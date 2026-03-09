#!/usr/bin/env bash
# Clone Go microservice repos into .dev/services/ for local Docker builds.
# These directories are gitignored — they are NOT submodules.
#
# Usage:
#   ./scripts/clone-services.sh          # clone (skip if already present)
#   ./scripts/clone-services.sh --pull   # clone + pull latest on each repo

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
SERVICES_DIR="$ROOT_DIR/.dev/services"

REPOS=(
  "git@github.com:UNagent-1D/Tenant.git|tenant"
  "git@github.com:UNagent-1D/conversation-chat.git|conversation-chat"
)

mkdir -p "$SERVICES_DIR"

for entry in "${REPOS[@]}"; do
  IFS='|' read -r repo_url dir_name <<< "$entry"
  target="$SERVICES_DIR/$dir_name"

  if [ -d "$target/.git" ]; then
    echo "[ok] $dir_name already cloned at $target"
    if [ "${1:-}" = "--pull" ]; then
      echo "     pulling latest..."
      git -C "$target" pull --ff-only
    fi
  else
    echo "[clone] $repo_url → $target"
    git clone "$repo_url" "$target"
  fi
done

echo ""
echo "Done. Services available at:"
echo "  $SERVICES_DIR/tenant"
echo "  $SERVICES_DIR/conversation-chat"
