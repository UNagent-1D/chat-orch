"""Mock Graph API — simulates WhatsApp Cloud API + Telegram Bot API for local testing.

Provides:
- POST /v18.0/:phone_number_id/messages — WhatsApp send (accepts, logs, returns mock)
- POST /bot:token/sendMessage — Telegram send
- POST /bot:token/sendPhoto — Telegram send photo
- POST /bot:token/answerCallbackQuery — Telegram callback answer

All endpoints accept the request, log it, and return a success response.
"""

import os
import json
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse

PORT = int(os.environ.get("PORT", "3003"))


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length) if content_length > 0 else b""

        parsed = urlparse(self.path)
        path = parsed.path

        try:
            data = json.loads(body) if body else {}
        except json.JSONDecodeError:
            data = {"raw": body.decode("utf-8", errors="replace")}

        print(f"[graph-mock] {self.command} {path}")
        print(f"  Body: {json.dumps(data, indent=2)[:500]}")

        # WhatsApp messages endpoint
        if "/messages" in path and "v18.0" in path:
            self._json(
                200,
                {
                    "messaging_product": "whatsapp",
                    "contacts": [
                        {"input": data.get("to", ""), "wa_id": data.get("to", "")}
                    ],
                    "messages": [{"id": "wamid.mock_" + os.urandom(8).hex()}],
                },
            )

        # Telegram endpoints
        elif path.startswith("/bot") and "/send" in path:
            self._json(
                200,
                {
                    "ok": True,
                    "result": {
                        "message_id": 12345,
                        "chat": {"id": data.get("chat_id", 0)},
                        "text": data.get("text", ""),
                    },
                },
            )

        elif path.startswith("/bot") and "answerCallbackQuery" in path:
            self._json(200, {"ok": True})

        # Hospital mock API endpoints (for tool execution)
        elif path == "/doctors":
            self._json(
                200,
                [
                    {
                        "id": "doc-001",
                        "name": "Dr. Garcia",
                        "specialty": "cardiology",
                        "location": "north",
                        "available_slots": ["2026-03-05 09:00", "2026-03-05 14:00"],
                    },
                    {
                        "id": "doc-002",
                        "name": "Dr. Rodriguez",
                        "specialty": "pediatrics",
                        "location": "south",
                        "available_slots": ["2026-03-05 10:00", "2026-03-06 11:00"],
                    },
                ],
            )

        elif path == "/appointments":
            if self.command == "POST":
                self._json(
                    201,
                    {
                        "id": "apt-" + os.urandom(4).hex(),
                        "status": "confirmed",
                        "doctor_id": data.get("doctor_id"),
                        "patient_name": data.get("patient_name"),
                        "date": data.get("date"),
                        "time": data.get("time"),
                    },
                )
            else:
                self._json(200, [])

        elif path == "/health":
            self._json(200, {"status": "ok"})

        else:
            self._json(200, {"ok": True, "mock": True})

    def do_GET(self):
        parsed = urlparse(self.path)

        if parsed.path == "/doctors":
            self._json(
                200,
                [
                    {"id": "doc-001", "name": "Dr. Garcia", "specialty": "cardiology"},
                    {
                        "id": "doc-002",
                        "name": "Dr. Rodriguez",
                        "specialty": "pediatrics",
                    },
                ],
            )
        elif parsed.path == "/health":
            self._json(200, {"status": "ok"})
        else:
            self._json(200, {"ok": True})

    def _json(self, status, data):
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass  # Suppress default logging, we log in do_POST/do_GET


if __name__ == "__main__":
    print(f"Mock Graph API + Hospital API listening on :{PORT}")
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
