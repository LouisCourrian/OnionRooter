#!/usr/bin/env bash
# Dev-time registration of the OnionRouter Native Messaging host on Linux.
#
# Usage:  ./installer/linux/register-dev.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EXE_RELEASE="$REPO_ROOT/companion/target/release/onionrouter-companion"
EXE_DEBUG="$REPO_ROOT/companion/target/debug/onionrouter-companion"
TEMPLATE="$REPO_ROOT/installer/com.onionrouter.companion.json.template"

if [[ -x "$EXE_RELEASE" ]]; then
  EXE="$EXE_RELEASE"
elif [[ -x "$EXE_DEBUG" ]]; then
  EXE="$EXE_DEBUG"
  echo "Using debug build at $EXE"
else
  echo "Companion binary not found. Run 'cargo build' in ./companion first." >&2
  exit 1
fi

HOST_DIR="$HOME/.mozilla/native-messaging-hosts"
mkdir -p "$HOST_DIR"
MANIFEST="$HOST_DIR/com.onionrouter.companion.json"

sed "s|{{COMPANION_PATH}}|$EXE|g" "$TEMPLATE" > "$MANIFEST"
chmod 644 "$MANIFEST"

echo "Registered OnionRouter companion:"
echo "  Manifest: $MANIFEST"
echo "  Binary  : $EXE"
echo
echo "Next: load the extension via about:debugging in Firefox."
