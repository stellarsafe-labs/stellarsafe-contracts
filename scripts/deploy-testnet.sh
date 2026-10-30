#!/usr/bin/env bash
# Deploys a StellarSafe treasury to Stellar testnet.
#
# Usage:
#   SOURCE=alice ./scripts/deploy-testnet.sh init.json
#
# SOURCE is a stellar-cli identity (`stellar keys generate alice --fund
# --network testnet`). init.json holds the constructor argument; see
# scripts/init.example.json. TESTNET ONLY: the contract has not been audited.
set -euo pipefail

NETWORK="${NETWORK:-testnet}"
SOURCE="${SOURCE:?set SOURCE to a stellar-cli identity}"
INIT_FILE="${1:?usage: deploy-testnet.sh <init.json>}"
WASM=target/wasm32v1-none/release/stellarsafe_treasury.wasm

if [[ "$NETWORK" == "mainnet" || "$NETWORK" == "pubnet" ]]; then
  echo "Refusing to deploy unaudited contracts to $NETWORK." >&2
  exit 1
fi

stellar contract build

echo "Uploading wasm..."
WASM_HASH=$(stellar contract upload --wasm "$WASM" --source "$SOURCE" --network "$NETWORK")
echo "wasm hash: $WASM_HASH"

echo "Deploying treasury..."
CONTRACT_ID=$(stellar contract deploy \
  --wasm-hash "$WASM_HASH" \
  --source "$SOURCE" \
  --network "$NETWORK" \
  -- \
  --init-file-path "$INIT_FILE")

echo "treasury contract id: $CONTRACT_ID"
stellar contract invoke --id "$CONTRACT_ID" --source "$SOURCE" --network "$NETWORK" -- get_config
