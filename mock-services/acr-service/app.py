"""Mock Agent Config Registry — simulates the Go ACR for local testing.

Provides:
- GET /api/v1/tenants/:tid/profiles/:pid/configs/active — active agent config
- GET /api/v1/tool-registry — global tool registry with OpenAI function definitions
"""

import os
import json
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse

PORT = int(os.environ.get("PORT", "3002"))

AGENT_CONFIG = {
    "id": "880e8400-e29b-41d4-a716-446655440003",
    "agent_profile_id": "770e8400-e29b-41d4-a716-446655440002",
    "version": 1,
    "status": "active",
    "conversation_policy": {
        "max_turns": 20,
        "idle_timeout_minutes": 30,
        "greeting": "Hello! I'm the scheduling assistant for Test Hospital. How can I help you today?",
    },
    "escalation_rules": {
        "triggers": ["frustrated_3x", "explicit_request"],
        "fallback_message": "Let me connect you with a human agent.",
    },
    "tool_permissions": [
        {
            "tool_name": "list_doctors",
            "constraints": {
                "type": "object",
                "properties": {
                    "specialty": {
                        "type": "string",
                        "description": "Medical specialty to filter by",
                    },
                    "location": {
                        "type": "string",
                        "description": "Hospital location (north, south)",
                    },
                },
            },
        },
        {
            "tool_name": "list_appointments",
            "constraints": {
                "type": "object",
                "properties": {
                    "doctor_id": {"type": "string", "description": "Doctor UUID"},
                    "date": {
                        "type": "string",
                        "description": "Date in YYYY-MM-DD format",
                    },
                },
            },
        },
        {
            "tool_name": "book_appointment",
            "constraints": {
                "type": "object",
                "properties": {
                    "doctor_id": {"type": "string"},
                    "patient_name": {"type": "string"},
                    "date": {"type": "string"},
                    "time": {"type": "string"},
                },
                "required": ["doctor_id", "patient_name", "date", "time"],
            },
        },
        {
            "tool_name": "reschedule_appointment",
            "constraints": {},
        },
        {
            "tool_name": "cancel_appointment",
            "constraints": {},
        },
    ],
    "llm_params": {
        "model": "qwen2.5:7b",
        "temperature": 0.3,
        "max_tokens": 1024,
        "system_prompt": (
            "You are a friendly scheduling assistant for Test Hospital. "
            "Help patients find doctors and book appointments. "
            "Use the available tools to look up doctors and available slots. "
            "Be concise and helpful. If you don't know something, say so honestly."
        ),
    },
    "channel_format_rules": {
        "whatsapp": {"max_chars": 4096},
        "telegram": {"max_chars": 4096},
        "web_widget": {"max_chars": None},
    },
    "created_at": "2026-03-01T00:00:00Z",
    "activated_at": "2026-03-01T00:00:00Z",
}

# Global tool registry with full OpenAI function-calling definitions.
# These provide richer schemas than the constraints-only fallback.
TOOL_REGISTRY = [
    {
        "id": "00000000-0000-0000-0000-000000000001",
        "tool_name": "list_doctors",
        "description": "List available doctors, optionally filtered by specialty and location.",
        "openai_function_def": {
            "name": "list_doctors",
            "description": "List available doctors at the hospital. Can filter by medical specialty and location.",
            "parameters": {
                "type": "object",
                "properties": {
                    "specialty": {
                        "type": "string",
                        "description": "Medical specialty to filter by (e.g., cardiology, pediatrics, general)",
                    },
                    "location": {
                        "type": "string",
                        "description": "Hospital location to filter by (e.g., bogota-norte, medellin-centro)",
                    },
                },
            },
        },
        "is_active": True,
        "version": 1,
    },
    {
        "id": "00000000-0000-0000-0000-000000000002",
        "tool_name": "get_doctor_schedule",
        "description": "Get available appointment slots for a specific doctor.",
        "openai_function_def": {
            "name": "get_doctor_schedule",
            "description": "Get the available appointment slots for a specific doctor on a given date.",
            "parameters": {
                "type": "object",
                "properties": {
                    "doctor_id": {
                        "type": "string",
                        "description": "The UUID of the doctor",
                    },
                    "date": {
                        "type": "string",
                        "description": "Date to check availability (YYYY-MM-DD format)",
                    },
                },
                "required": ["doctor_id"],
            },
        },
        "is_active": True,
        "version": 1,
    },
    {
        "id": "00000000-0000-0000-0000-000000000003",
        "tool_name": "book_appointment",
        "description": "Book a new appointment with a doctor.",
        "openai_function_def": {
            "name": "book_appointment",
            "description": "Book a new appointment with a doctor at the hospital. Requires patient name, doctor, date and time.",
            "parameters": {
                "type": "object",
                "properties": {
                    "doctor_id": {
                        "type": "string",
                        "description": "The UUID of the doctor to book with",
                    },
                    "patient_name": {
                        "type": "string",
                        "description": "Full name of the patient",
                    },
                    "date": {
                        "type": "string",
                        "description": "Appointment date (YYYY-MM-DD format)",
                    },
                    "time": {
                        "type": "string",
                        "description": "Appointment time (HH:MM format, 24-hour)",
                    },
                },
                "required": ["doctor_id", "patient_name", "date", "time"],
            },
        },
        "is_active": True,
        "version": 1,
    },
    {
        "id": "00000000-0000-0000-0000-000000000004",
        "tool_name": "reschedule_appointment",
        "description": "Reschedule an existing appointment to a new date and time.",
        "openai_function_def": {
            "name": "reschedule_appointment",
            "description": "Reschedule an existing appointment to a new date and/or time.",
            "parameters": {
                "type": "object",
                "properties": {
                    "appointment_id": {
                        "type": "string",
                        "description": "The UUID of the appointment to reschedule",
                    },
                    "new_date": {
                        "type": "string",
                        "description": "New appointment date (YYYY-MM-DD format)",
                    },
                    "new_time": {
                        "type": "string",
                        "description": "New appointment time (HH:MM format, 24-hour)",
                    },
                },
                "required": ["appointment_id"],
            },
        },
        "is_active": True,
        "version": 1,
    },
    {
        "id": "00000000-0000-0000-0000-000000000005",
        "tool_name": "cancel_appointment",
        "description": "Cancel an existing appointment.",
        "openai_function_def": {
            "name": "cancel_appointment",
            "description": "Cancel an existing appointment. This action cannot be undone.",
            "parameters": {
                "type": "object",
                "properties": {
                    "appointment_id": {
                        "type": "string",
                        "description": "The UUID of the appointment to cancel",
                    },
                    "reason": {
                        "type": "string",
                        "description": "Optional reason for cancellation",
                    },
                },
                "required": ["appointment_id"],
            },
        },
        "is_active": True,
        "version": 1,
    },
]


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)

        if "/configs/active" in parsed.path:
            self._json(200, AGENT_CONFIG)
        elif parsed.path == "/api/v1/tool-registry":
            self._json(200, TOOL_REGISTRY)
        elif parsed.path == "/health":
            self._json(200, {"status": "ok"})
        else:
            self._json(404, {"error": "not found"})

    def _json(self, status, data):
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        print(f"[acr-mock] {args[0]}")


if __name__ == "__main__":
    print(f"Mock ACR Service listening on :{PORT}")
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
