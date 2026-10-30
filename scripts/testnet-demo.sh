#!/usr/bin/env bash
# End-to-end StellarSafe demo on Stellar testnet.
#
# Creates fresh funded accounts and a test asset, deploys a treasury, then runs
# payments, a rejected payment, a timelocked governance change, a guardian veto
# and a pause. Every transaction is written to docs/TESTNET.md with explorer
# links.
#
#   ./scripts/testnet-demo.sh            # needs stellar-cli >= 25.2
set -euo pipefail

NET=testnet
OUT=docs/TESTNET.md
EXPLORER=https://stellar.expert/explorer/testnet
WASM=target/wasm32v1-none/release/stellarsafe_treasury.wasm
DELAY=120        # governance timelock (seconds), short for the demo
UNIT=10000000    # 7 decimals
P="ssdemo"       # key-name prefix

log() { printf '\n\033[1m== %s\033[0m\n' "$*" >&2; }
addr() { stellar keys address "$P-$1"; }
rows=()

# Run a contract call and record its tx hash. The return value is left in
# $RESULT (not printed) so the row survives: no subshell.
# Usage: call "<description>" <source> <contract> -- fn args...
RESULT=""
call() {
  local desc=$1 src=$2 id=$3; shift 3
  local err; err=$(mktemp)
  if RESULT=$(stellar contract invoke --id "$id" --source "$P-$src" --network $NET --send yes "$@" 2>"$err"); then
    local hash shown
    # Last 64-hex string in the CLI log that isn't one of our own arguments
    # (e.g. a wasm hash passed to Upgrade).
    hash=$(grep -oE '[0-9a-f]{64}' "$err" | grep -vxF -f <(printf '%s\n' "$@" | grep -oE '[0-9a-f]{64}' || true) | tail -1 || true)
    shown=$RESULT; [[ "$shown" == "null" || -z "$shown" ]] && shown="ok"
    echo "  ok: $desc → $shown ${hash:+($hash)}" >&2
    rows+=("| $desc | $shown | [\`${hash:0:12}…\`]($EXPLORER/tx/$hash) |")
    rm -f "$err"
  else
    cat "$err" >&2; rm -f "$err"; return 1
  fi
}

# Simulate a call that must fail with a given contract error code.
expect_err() {
  local desc=$1 code=$2 src=$3 id=$4; shift 4
  local res
  if res=$(stellar contract invoke --id "$id" --source "$P-$src" --network $NET --send no "$@" 2>&1); then
    echo "expected error #$code but call succeeded: $desc" >&2; exit 1
  fi
  if ! grep -q "#$code" <<<"$res"; then
    echo "expected error #$code, got:" >&2; echo "$res" >&2; exit 1
  fi
  echo "  rejected as expected: $desc (Error #$code)" >&2
  rows+=("| $desc | rejected with \`Error(Contract, #$code)\` (simulation) | – |")
}

log "Build"
stellar contract build >/dev/null
WASM_HASH=$(sha256sum "$WASM" | cut -d' ' -f1)

log "Accounts (funded by friendbot)"
for k in alice bob carol guardian vendor stranger issuer; do
  stellar keys generate "$P-$k" --network $NET --fund --overwrite >/dev/null 2>&1
  echo "  $k: $(addr $k)" >&2
done

log "Test asset SSUSD + Stellar Asset Contract"
ASSET="SSUSD:$(addr issuer)"
TOKEN=$(stellar contract asset deploy --asset "$ASSET" --source "$P-issuer" --network $NET 2>/dev/null \
  || stellar contract id asset --asset "$ASSET" --network $NET)
for k in vendor stranger; do
  stellar tx new change-trust --source "$P-$k" --line "$ASSET" --network $NET >/dev/null
done
echo "  token: $TOKEN" >&2

log "Deploy treasury"
INIT=$(cat <<JSON
{
  "members": {
    "$(addr alice)": 7,
    "$(addr bob)": 3,
    "$(addr carol)": 2,
    "$(addr guardian)": 8
  },
  "governance_threshold": 2,
  "governance_delay": $DELAY,
  "max_proposal_ttl": 86400,
  "allowlist_enabled": true,
  "allowlist": ["$(addr vendor)"],
  "asset_policies": {
    "$TOKEN": {
      "max_amount": "$((10000 * UNIT))",
      "tiers": [
        { "min_amount": "0", "approvals": 1 },
        { "min_amount": "$((500 * UNIT))", "approvals": 2 }
      ]
    }
  },
  "budgets": [
    { "department": 1, "token": "$TOKEN",
      "budget": { "limit": "$((5000 * UNIT))", "period": "CalendarMonth" } }
  ]
}
JSON
)
TREASURY=$(stellar contract deploy --wasm "$WASM" --source "$P-alice" --network $NET -- --init "$INIT")
echo "  treasury: $TREASURY" >&2

log "Fund treasury"
call "Issuer mints 20,000 SSUSD to the treasury" issuer "$TOKEN" -- mint --to "$TREASURY" --amount "$((20000 * UNIT))"

log "Payment below 500: one approval"
call "Bob proposes 100 SSUSD to vendor (dept 1)" bob "$TREASURY" -- propose_payment \
  --proposer "$(addr bob)" --token "$TOKEN" --to "$(addr vendor)" --amount "$((100 * UNIT))" --department 1 --ttl 86400
ID=$RESULT
call "Alice approves #$ID" alice "$TREASURY" -- approve --approver "$(addr alice)" --id "$ID"
call "Alice executes #$ID: 100 SSUSD paid" alice "$TREASURY" -- execute --executor "$(addr alice)" --id "$ID"

log "Payment of 750: two approvals"
call "Bob proposes 750 SSUSD to vendor" bob "$TREASURY" -- propose_payment \
  --proposer "$(addr bob)" --token "$TOKEN" --to "$(addr vendor)" --amount "$((750 * UNIT))" --department 1 --ttl 86400
ID=$RESULT
call "Alice approves #$ID" alice "$TREASURY" -- approve --approver "$(addr alice)" --id "$ID"
expect_err "Executing #$ID with 1 of 2 approvals" 305 alice "$TREASURY" -- execute --executor "$(addr alice)" --id "$ID"
call "Carol approves #$ID" carol "$TREASURY" -- approve --approver "$(addr carol)" --id "$ID"
call "Alice executes #$ID: 750 SSUSD paid" alice "$TREASURY" -- execute --executor "$(addr alice)" --id "$ID"

log "Policy rejections"
expect_err "Paying a recipient not on the allowlist" 403 bob "$TREASURY" -- propose_payment \
  --proposer "$(addr bob)" --token "$TOKEN" --to "$(addr stranger)" --amount "$UNIT" --department 1 --ttl 86400
expect_err "Paying 5,001 SSUSD from a 5,000/month budget" 405 bob "$TREASURY" -- propose_payment \
  --proposer "$(addr bob)" --token "$TOKEN" --to "$(addr vendor)" --amount "$((5001 * UNIT))" --department 1 --ttl 86400

log "Timelocked governance: allowlist a new recipient"
call "Alice proposes SetRecipientAllowed(stranger, true)" alice "$TREASURY" -- propose \
  --proposer "$(addr alice)" --action "{\"SetRecipientAllowed\":[\"$(addr stranger)\",true]}" --ttl 3600
ID=$RESULT
call "Alice approves #$ID" alice "$TREASURY" -- approve --approver "$(addr alice)" --id "$ID"
call "Bob approves #$ID (threshold reached, timelock starts)" bob "$TREASURY" -- approve --approver "$(addr bob)" --id "$ID"
expect_err "Executing #$ID before the ${DELAY}s timelock ends" 307 alice "$TREASURY" -- execute --executor "$(addr alice)" --id "$ID"
echo "  waiting $((DELAY + 10))s for the timelock..." >&2; sleep $((DELAY + 10))
call "Alice executes #$ID after the timelock" alice "$TREASURY" -- execute --executor "$(addr alice)" --id "$ID"

log "Guardian veto"
call "Alice proposes Upgrade(unknown wasm)" alice "$TREASURY" -- propose \
  --proposer "$(addr alice)" --action "{\"Upgrade\":\"$(printf 'ab%.0s' {1..32})\"}" --ttl 3600
ID=$RESULT
call "Alice approves #$ID" alice "$TREASURY" -- approve --approver "$(addr alice)" --id "$ID"
call "Bob approves #$ID" bob "$TREASURY" -- approve --approver "$(addr bob)" --id "$ID"
call "Guardian vetoes #$ID during the timelock" guardian "$TREASURY" -- cancel --caller "$(addr guardian)" --id "$ID"

log "Emergency pause"
call "Bob proposes 10 SSUSD to vendor" bob "$TREASURY" -- propose_payment \
  --proposer "$(addr bob)" --token "$TOKEN" --to "$(addr vendor)" --amount "$((10 * UNIT))" --department 1 --ttl 86400
ID=$RESULT
call "Alice approves #$ID" alice "$TREASURY" -- approve --approver "$(addr alice)" --id "$ID"
call "Guardian pauses payments" guardian "$TREASURY" -- pause --guardian "$(addr guardian)"
expect_err "Executing #$ID while paused" 406 alice "$TREASURY" -- execute --executor "$(addr alice)" --id "$ID"

log "Final state"
VENDOR_BAL=$(stellar contract invoke --id "$TOKEN" --source "$P-alice" --network $NET --send no -- balance --id "$(addr vendor)")
TREASURY_BAL=$(stellar contract invoke --id "$TOKEN" --source "$P-alice" --network $NET --send no -- balance --id "$TREASURY")
BUDGET=$(stellar contract invoke --id "$TREASURY" --source "$P-alice" --network $NET --send no -- get_budget --department 1 --token "$TOKEN")

{
  echo "# Testnet deployment"
  echo
  echo "Generated by \`scripts/testnet-demo.sh\` on $(date -u +%Y-%m-%dT%H:%M:%SZ). Testnet is reset periodically, so these links may expire; re-run the script to reproduce."
  echo
  echo "| | |"
  echo "|---|---|"
  echo "| Treasury contract | [\`$TREASURY\`]($EXPLORER/contract/$TREASURY) |"
  echo "| Wasm SHA-256 (= on-chain wasm hash) | \`$WASM_HASH\` |"
  echo "| Commit | \`${DEMO_COMMIT:-$(git rev-parse HEAD)}\` |"
  echo "| Test token (SSUSD SAC) | [\`$TOKEN\`]($EXPLORER/contract/$TOKEN) |"
  echo "| Members | alice (proposer, approver, executor), bob (proposer, approver), carol (approver), guardian |"
  echo "| Policy | 1 approval below 500 SSUSD, 2 from 500; cap 10,000; dept 1 budget 5,000/month; allowlist on; ${DELAY}s governance timelock |"
  echo
  echo "## Transactions"
  echo
  echo "| Step | Result | Transaction |"
  echo "|---|---|---|"
  printf '%s\n' "${rows[@]}"
  echo
  echo "## Final state"
  echo
  echo "- Vendor balance: \`$VENDOR_BAL\` (850 SSUSD; 7 decimals)"
  echo "- Treasury balance: \`$TREASURY_BAL\`"
  echo "- Department 1 budget: \`$BUDGET\`"
  echo
  echo "Verify the deployed code matches this repository:"
  echo
  echo '```sh'
  echo "NETWORK=testnet ./scripts/verify-build.sh $TREASURY"
  echo '```'
} > "$OUT"

log "Done — wrote $OUT"
