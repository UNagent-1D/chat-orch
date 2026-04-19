use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;

#[derive(Clone)]
pub struct HospitalClient {
    http: Client,
    base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl HospitalClient {
    pub fn new(http: Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Dispatch a tool call to the right hospital-mock endpoint. Returns the
    /// raw JSON body so the LLM sees exactly what the service returned.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Value, AppError> {
        match name {
            "list_doctors" => self.list_doctors(args).await,
            "get_doctor_schedule" => self.get_doctor_schedule(args).await,
            "book_appointment" => self.book_appointment(args).await,
            "cancel_appointment" => self.cancel_appointment(args).await,
            "get_patient_appointments" => self.get_patient_appointments(args).await,
            other => Err(AppError::BadRequest(format!("unknown tool: {other}"))),
        }
    }

    async fn list_doctors(&self, args: &Value) -> Result<Value, AppError> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(v) = args.get("area").and_then(|x| x.as_str()) {
            q.push(("area", v.to_string()));
        }
        if let Some(v) = args.get("place").and_then(|x| x.as_str()) {
            q.push(("place", v.to_string()));
        }
        self.send_get(&self.url("/doctors"), &q).await
    }

    async fn get_doctor_schedule(&self, args: &Value) -> Result<Value, AppError> {
        let doctor_id = args
            .get("doctor_id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::BadRequest("doctor_id is required".into()))?;
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(v) = args.get("days_ahead").and_then(|x| x.as_i64()) {
            q.push(("days_ahead", v.to_string()));
        }
        self.send_get(&self.url(&format!("/doctors/{doctor_id}/schedule")), &q)
            .await
    }

    async fn book_appointment(&self, args: &Value) -> Result<Value, AppError> {
        self.send_post(&self.url("/appointments"), args).await
    }

    async fn cancel_appointment(&self, args: &Value) -> Result<Value, AppError> {
        let appt_id = args
            .get("appointment_id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::BadRequest("appointment_id is required".into()))?;
        let body = json!({
            "reason": args.get("reason").and_then(|x| x.as_str()).unwrap_or("not specified"),
        });
        self.send_post(
            &self.url(&format!("/appointments/{appt_id}/cancel")),
            &body,
        )
        .await
    }

    async fn get_patient_appointments(&self, args: &Value) -> Result<Value, AppError> {
        let patient_ref = args
            .get("patient_ref")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::BadRequest("patient_ref is required".into()))?;
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(v) = args.get("status").and_then(|x| x.as_str()) {
            q.push(("status", v.to_string()));
        }
        self.send_get(
            &self.url(&format!("/patients/{patient_ref}/appointments")),
            &q,
        )
        .await
    }

    async fn send_get(&self, url: &str, query: &[(&str, String)]) -> Result<Value, AppError> {
        let resp = self.http.get(url).query(query).send().await?;
        Self::decode(resp, url).await
    }

    async fn send_post(&self, url: &str, body: &Value) -> Result<Value, AppError> {
        let resp = self.http.post(url).json(body).send().await?;
        Self::decode(resp, url).await
    }

    async fn decode(resp: reqwest::Response, url: &str) -> Result<Value, AppError> {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_else(|_| json!({}));
        if !status.is_success() {
            // Surface the error body to the LLM — hospital-mock returns
            // {"error": "..."} shapes that the model can reason about.
            return Ok(json!({
                "error": true,
                "status": status.as_u16(),
                "body": body,
                "url": url,
            }));
        }
        Ok(body)
    }
}

/// OpenAI-style tool definitions for the five hospital-mock operations.
/// Fed to the LLM as the `tools` array on each chat.completions request.
pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_doctors".into(),
            description:
                "Lista médicos disponibles. Filtros opcionales por área (especialidad) y lugar (ubicación). Ejemplos de áreas: Cardiologist, Pediatrician, General Practitioner, Neurologist."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "area":  {"type": "string", "description": "Especialidad, coincidencia parcial case-insensitive, p.ej. 'cardio'."},
                    "place": {"type": "string", "description": "Palabra clave de ubicación, p.ej. 'Bogota'."}
                }
            }),
        },
        ToolDef {
            name: "get_doctor_schedule".into(),
            description:
                "Devuelve los espacios de 30 min disponibles para un médico. Usa este tool antes de agendar una cita."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "doctor_id":  {"type": "string", "description": "ID del médico, p.ej. 'doc-001'."},
                    "days_ahead": {"type": "integer", "description": "Días hacia adelante (default 7, máx 30)."}
                },
                "required": ["doctor_id"]
            }),
        },
        ToolDef {
            name: "book_appointment".into(),
            description: "Crea una nueva cita confirmada para un paciente con un médico específico.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "doctor_id":    {"type": "string"},
                    "patient_ref":  {"type": "string", "description": "Identificador del paciente, p.ej. 'HOSP-PAT-00492'."},
                    "patient_name": {"type": "string", "description": "Nombre completo del paciente."},
                    "slot_start":   {"type": "string", "description": "Hora de inicio en ISO 8601, p.ej. '2026-03-15T09:00:00'."},
                    "specialty":    {"type": "string", "description": "Especialidad; por defecto la del médico."}
                },
                "required": ["doctor_id", "patient_ref", "patient_name", "slot_start"]
            }),
        },
        ToolDef {
            name: "cancel_appointment".into(),
            description: "Cancela una cita existente (no la elimina, cambia su estado a cancelada).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "appointment_id": {"type": "string", "description": "ID de la cita, p.ej. 'appt-seed-001'."},
                    "reason":         {"type": "string"}
                },
                "required": ["appointment_id"]
            }),
        },
        ToolDef {
            name: "get_patient_appointments".into(),
            description: "Lista todas las citas de un paciente.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patient_ref": {"type": "string"},
                    "status":      {"type": "string", "enum": ["confirmed", "cancelled", "all"]}
                },
                "required": ["patient_ref"]
            }),
        },
    ]
}
