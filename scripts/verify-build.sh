#!/usr/bin/env bash
# Rebuilds the contract and compares its hash with a release or a deployed
# contract.
#
#   ./scripts/verify-build.sh                         # just print the hash
#   ./scripts/verify-build.sh <sha256>                # compare with a release
#   NETWORK=testnet ./scripts/verify-build.sh C...    # compare with on-chain code
set -euo pipefail

WASM=target/wasm32v1-none/release/stellarsafe_treasury.wasm
stellar contract build >/dev/null
LOCAL=$(sha256sum "$WASM" | cut -d' ' -f1)
echo "local build: $LOCAL"

TARGET="${1:-}"
[[ -z "$TARGET" ]] && exit 0

if [[ "$TARGET" == C* ]]; then
  TMP=$(mktemp)
  stellar contract fetch --id "$TARGET" --network "${NETWORK:-testnet}" --out-file "$TMP"
  TARGET=$(sha256sum "$TMP" | cut -d' ' -f1)
  rm -f "$TMP"
  echo "on-chain:    $TARGET"
else
  echo "expected:    $TARGET"
fi

if [[ "$LOCAL" == "$TARGET" ]]; then
  echo "MATCH"
else
  echo "MISMATCH" >&2
  exit 1
fi
