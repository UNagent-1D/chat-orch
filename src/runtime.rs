use serde_json::Value;

use crate::hospital::{tool_definitions, HospitalClient};
use crate::llm::{ChatMessage, ChatResponse, LlmClient, ToolCall};
use crate::session::SessionStore;

const MAX_TOOL_ROUNDS: usize = 5;

const SYSTEM_PROMPT: &str = r#"Eres el asistente de agendamiento de la Clínica San Ignacio (red privada en Bogotá y Medellín).

Tu rol: ayudar a pacientes a consultar médicos, ver horarios disponibles, agendar citas, cancelar citas y consultar sus citas existentes.

Reglas:
- Responde siempre en español neutro, cálido y profesional.
- Usa las herramientas disponibles antes de inventar información. Si no sabes el doctor_id exacto, primero lista médicos con `list_doctors`.
- Antes de agendar (`book_appointment`), verifica disponibilidad con `get_doctor_schedule`.
- Si el paciente no te ha dado su `patient_ref` o nombre completo para agendar, pídelos.
- Las fechas se manejan en formato ISO 8601 (2026-03-15T09:00:00). El horario de atención es lunes a viernes de 9:00 a 11:30 y de 14:00 a 16:30.
- Al cancelar una cita pide confirmación antes de llamar la herramienta.
- Si una herramienta devuelve `error: true`, explícale al paciente lo ocurrido en sus términos, sin filtrar errores técnicos.

Herramientas disponibles:
- list_doctors(area?, place?) — catálogo de médicos.
- get_doctor_schedule(doctor_id, days_ahead?) — slots libres de 30 min.
- book_appointment(doctor_id, patient_ref, patient_name, slot_start, specialty?) — crea cita.
- cancel_appointment(appointment_id, reason?) — cancela una cita existente.
- get_patient_appointments(patient_ref, status?) — consulta citas del paciente.
"#;

const FALLBACK_REPLY: &str =
    "Lo siento, tuve un problema atendiendo tu solicitud. Intenta de nuevo en un momento.";

pub async fn run_turn(
    llm: &LlmClient,
    hospital: &HospitalClient,
    sessions: &SessionStore,
    sid: &str,
    user_text: &str,
) -> (String, bool) {
    let user_msg = ChatMessage {
        role: "user".into(),
        content: Some(user_text.to_string()),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    };
    sessions.append(sid, user_msg).await;

    let tools = tool_definitions();
    let mut resolved = false;

    for _round in 0..MAX_TOOL_ROUNDS {
        let mut messages = vec![ChatMessage {
            role: "system".into(),
            content: Some(SYSTEM_PROMPT.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        messages.extend(sessions.history(sid).await);

        let resp = match llm.complete(&messages, &tools).await {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(error=%err, "llm completion failed");
                return (FALLBACK_REPLY.into(), false);
            }
        };

        match resp {
            ChatResponse::Content(text) => {
                let text = if text.trim().is_empty() {
                    FALLBACK_REPLY.to_string()
                } else {
                    text
                };
                sessions
                    .append(
                        sid,
                        ChatMessage {
                            role: "assistant".into(),
                            content: Some(text.clone()),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                        },
                    )
                    .await;
                return (text, resolved);
            }
            ChatResponse::ToolCalls(calls) => {
                // Record the assistant's tool-call message so the next round
                // includes it as context per OpenAI protocol.
                sessions
                    .append(
                        sid,
                        ChatMessage {
                            role: "assistant".into(),
                            content: None,
                            tool_calls: Some(calls.clone()),
                            tool_call_id: None,
                            name: None,
                        },
                    )
                    .await;

                for call in calls {
                    let (result, booked) = execute_tool(hospital, &call).await;
                    resolved = resolved || booked;
                    sessions
                        .append(
                            sid,
                            ChatMessage {
                                role: "tool".into(),
                                content: Some(result),
                                tool_calls: None,
                                tool_call_id: Some(call.id.clone()),
                                name: Some(call.function.name.clone()),
                            },
                        )
                        .await;
                }
            }
        }
    }

    tracing::warn!(sid=%sid, "tool loop hit round cap");
    (FALLBACK_REPLY.into(), false)
}

async fn execute_tool(hospital: &HospitalClient, call: &ToolCall) -> (String, bool) {
    let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| Value::Null);
    match hospital.call_tool(&call.function.name, &args).await {
        Ok(val) => {
            let booked = call.function.name == "book_appointment"
                && val.get("error") != Some(&Value::Bool(true));
            (val.to_string(), booked)
        }
        Err(err) => (
            serde_json::json!({
                "error": true,
                "detail": err.to_string(),
            })
            .to_string(),
            false,
        ),
    }
}
