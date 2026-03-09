"""Mock Tenant Service — simulates the Go Tenant Service for local testing.

Provides:
- GET /internal/resolve-channel — channel → tenant resolution
- GET /api/v1/tenants/:id — tenant details
- GET /api/v1/tenants/:id/profiles — agent profiles
- GET /api/v1/tenants/:id/data-sources — data sources for tool execution
"""

import os
import json
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs

PORT = int(os.environ.get("PORT", "3001"))

# ── Test fixtures ─────────────────────────────────────────────────────

TENANT_ID = "550e8400-e29b-41d4-a716-446655440000"
PROFILE_ID = "770e8400-e29b-41d4-a716-446655440002"

CHANNEL_MAP = {
    "telegram:test-hospital": {
        "tenant_id": TENANT_ID,
        "tenant_slug": "test-hospital",
        "agent_profile_id": PROFILE_ID,
        "webhook_secret_ref": "test-secret",
        "is_active": True,
    },
    "whatsapp:123456789012345": {
        "tenant_id": TENANT_ID,
        "tenant_slug": "test-hospital",
        "agent_profile_id": PROFILE_ID,
        "webhook_secret_ref": "test-secret",
        "is_active": True,
    },
}

TENANT = {
    "id": TENANT_ID,
    "slug": "test-hospital",
    "name": "Test Hospital",
    "plan": "pro",
    "status": "active",
    "branding_logo_url": None,
    "branding_primary_color": "#2E75B6",
}

PROFILES = [
    {
        "id": PROFILE_ID,
        "name": "Scheduling Bot",
        "description": "Handles appointment scheduling",
        "scheduling_flow_rules": {
            "steps": ["greet", "collect_specialty", "find_doctor", "book"]
        },
        "escalation_rules": {"triggers": ["frustrated_3x", "explicit_request"]},
        "allowed_specialties": ["cardiology", "pediatrics", "general"],
        "allowed_locations": ["north", "south"],
        "agent_config_id": "880e8400-e29b-41d4-a716-446655440003",
    }
]

DATA_SOURCES = [
    {
        "id": "990e8400-e29b-41d4-a716-446655440004",
        "name": "Hospital Mock API",
        "source_type": "rest_api",
        "base_url": "http://graph-mock:3003",
        "credential_ref": None,
        "route_configs": {
            "list_doctors": {
                "method": "GET",
                "path": "/doctors",
                "query_params": ["specialty", "location"],
            },
            "list_appointments": {
                "method": "GET",
                "path": "/appointments",
                "query_params": ["doctor_id", "date"],
            },
            "book_appointment": {
                "method": "POST",
                "path": "/appointments",
            },
        },
        "is_active": True,
    }
]


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)
        qs = parse_qs(parsed.query)

        if parsed.path == "/internal/resolve-channel":
            channel_type = qs.get("channel_type", [""])[0]
            channel_key = qs.get("channel_key", [""])[0]
            lookup = f"{channel_type}:{channel_key}"

            if lookup in CHANNEL_MAP:
                self._json(200, CHANNEL_MAP[lookup])
            else:
                self._json(404, {"error": "channel not found"})

        elif parsed.path.startswith("/api/v1/tenants/") and parsed.path.endswith(
            "/profiles"
        ):
            self._json(200, {"data": PROFILES})

        elif parsed.path.startswith("/api/v1/tenants/") and parsed.path.endswith(
            "/data-sources"
        ):
            self._json(200, {"data": DATA_SOURCES})

        elif (
            parsed.path.startswith("/api/v1/tenants/")
            and "/end-users/lookup/phone/" in parsed.path
        ):
            # End-user lookup by phone — return "not found" for unknown numbers
            self._json(200, {"exists": False})

        elif parsed.path.startswith("/api/v1/tenants/"):
            self._json(200, TENANT)

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
        print(f"[tenant-mock] {args[0]}")


if __name__ == "__main__":
    print(f"Mock Tenant Service listening on :{PORT}")
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
