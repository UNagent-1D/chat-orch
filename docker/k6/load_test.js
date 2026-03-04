// k6 load test for the Chat Orchestrator.
//
// Usage:
//   k6 run docker/k6/load_test.js
//   k6 run --vus 100 --duration 60s docker/k6/load_test.js
//
// Prerequisites:
//   docker compose up -d  (Redis + mocks must be running)
//   cargo run              (orchestrator must be running on :3000)

import http from "k6/http";
import { check, sleep } from "k6";
import { Rate, Trend } from "k6/metrics";

// Custom metrics
const errorRate = new Rate("error_rate");
const webhookDuration = new Trend("webhook_duration");

// Test configuration
export const options = {
  stages: [
    { duration: "10s", target: 10 }, // Ramp up to 10 VUs
    { duration: "30s", target: 50 }, // Ramp up to 50 VUs
    { duration: "30s", target: 100 }, // Ramp up to 100 VUs
    { duration: "20s", target: 100 }, // Stay at 100 VUs
    { duration: "10s", target: 0 }, // Ramp down
  ],
  thresholds: {
    http_req_duration: ["p(95)<500"], // 95% of requests under 500ms
    error_rate: ["rate<0.05"], // Error rate under 5%
  },
};

const BASE_URL = __ENV.BASE_URL || "http://localhost:3000";

// ── Telegram webhook simulation ───────────────────────────────────────

function telegramWebhook() {
  const updateId = Math.floor(Math.random() * 1000000000);
  const userId = Math.floor(Math.random() * 10000) + 1;

  const payload = JSON.stringify({
    update_id: updateId,
    message: {
      message_id: updateId,
      from: {
        id: userId,
        first_name: `User${userId}`,
        is_bot: false,
      },
      chat: {
        id: userId,
        type: "private",
      },
      date: Math.floor(Date.now() / 1000),
      text: "I need to schedule a cardiology appointment",
    },
  });

  const res = http.post(
    `${BASE_URL}/webhook/telegram/test-hospital`,
    payload,
    {
      headers: {
        "Content-Type": "application/json",
        "X-Telegram-Bot-Api-Secret-Token": "test-secret",
      },
    }
  );

  webhookDuration.add(res.timings.duration);

  const success = check(res, {
    "status is 200 or 503": (r) => r.status === 200 || r.status === 503,
    "status is 200": (r) => r.status === 200,
  });

  errorRate.add(!success);
}

// ── Health check ──────────────────────────────────────────────────────

function healthCheck() {
  const res = http.get(`${BASE_URL}/health`);

  check(res, {
    "health returns 200": (r) => r.status === 200,
    "health returns ok": (r) => r.body === "ok",
  });
}

// ── Main test function ────────────────────────────────────────────────

export default function () {
  // 90% webhook traffic, 10% health checks
  if (Math.random() < 0.9) {
    telegramWebhook();
  } else {
    healthCheck();
  }

  sleep(0.01); // 10ms between requests per VU
}
