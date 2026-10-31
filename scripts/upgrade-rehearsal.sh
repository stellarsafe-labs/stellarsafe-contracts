#!/usr/bin/env bash
# Rehearses an upgrade before it is proposed: snapshots a live treasury,
# then upgrades the snapshot to the code in this checkout through governance
# and runs `migrate`, metered against mainnet limits. Read-only: nothing is
# sent to the network.
#
# Usage:
#   ./scripts/upgrade-rehearsal.sh C...CONTRACT_ID          # testnet
#   NETWORK=mainnet ./scripts/upgrade-rehearsal.sh C...ID
set -euo pipefail

NETWORK="${NETWORK:-testnet}"
CONTRACT="${1:?usage: upgrade-rehearsal.sh <contract id>}"
SNAPSHOT="$PWD/target/upgrade-rehearsal/snapshot.json"

stellar contract build
mkdir -p "$(dirname "$SNAPSHOT")"
echo "Snapshotting $CONTRACT on $NETWORK..." >&2
stellar snapshot create --address "$CONTRACT" --network "$NETWORK" \
  --output json --out "$SNAPSHOT"

STELLARSAFE_SNAPSHOT="$SNAPSHOT" STELLARSAFE_CONTRACT="$CONTRACT" \
  cargo test -p stellarsafe-treasury --features upgrade-rehearsal \
  --test upgrade_rehearsal -- --nocapture
