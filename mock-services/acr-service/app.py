"""Mock Agent Config Registry — simulates the Go ACR for local testing.

Provides:
- GET /api/v1/tenants/:tid/profiles/:pid/configs/active — active agent config
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
    ],
    "llm_params": {
        "model": "gpt-4o",
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


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)

        if "/configs/active" in parsed.path:
            self._json(200, AGENT_CONFIG)
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
