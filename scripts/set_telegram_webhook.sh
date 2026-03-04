#!/usr/bin/env bash
# Set the Telegram webhook URL for a bot.
#
# Usage:
#   ./scripts/set_telegram_webhook.sh <webhook_url> <tenant_slug>
#
# Example:
#   ./scripts/set_telegram_webhook.sh https://your-domain.com test-hospital
#
# Prerequisites:
#   - TELEGRAM_BOT_TOKEN env var must be set
#   - TELEGRAM_WEBHOOK_SECRET env var must be set

set -euo pipefail

WEBHOOK_URL="${1:?Usage: $0 <webhook_url> <tenant_slug>}"
TENANT_SLUG="${2:?Usage: $0 <webhook_url> <tenant_slug>}"

if [ -z "${TELEGRAM_BOT_TOKEN:-}" ]; then
    echo "Error: TELEGRAM_BOT_TOKEN is not set"
    exit 1
fi

FULL_URL="${WEBHOOK_URL}/webhook/telegram/${TENANT_SLUG}"

echo "Setting Telegram webhook to: ${FULL_URL}"

curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/setWebhook" \
    -d "url=${FULL_URL}" \
    -d "secret_token=${TELEGRAM_WEBHOOK_SECRET:-}" \
    | python3 -m json.tool

echo ""
echo "Done. Verify with:"
echo "  curl https://api.telegram.org/bot\${TELEGRAM_BOT_TOKEN}/getWebhookInfo | python3 -m json.tool"
