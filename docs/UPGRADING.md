# Upgrading a treasury

A treasury has no admin key, so its code changes only through an `Upgrade(wasm_hash)` governance proposal. That proposal needs `governance_threshold` approvals, waits out `governance_delay`, and guardians can veto it. Storage is kept across the upgrade.

## How storage versions work

- `STORAGE_VERSION` (in `types.rs`) is the storage layout the code reads and writes. `storage_version()` returns the layout of the stored data. Treasuries deployed before versions were recorded report `1`.
- If an upgrade raises `STORAGE_VERSION`, every write fails with `MigrationRequired` (600) until someone calls `migrate()`. Views keep working.
- `migrate()` can be called by anyone and is safe to repeat. It runs each conversion step from the stored version up to the current one, in order, so no version is skipped. Once storage is current, calling it again does nothing.
- If stored data is *newer* than the code (after a downgrade), `migrate()` fails with `UnsupportedStorageVersion` (601). Writes are still allowed in that state, so governance can upgrade forward again.

## Changing the storage layout

Bump `STORAGE_VERSION` whenever a stored type or key changes shape: `Config`, `Proposal`, `AssetPolicy`, `Budget`, `BudgetUsage` or `DataKey`. New `Action` variants and new `DataKey` variants appended at the end don't need a bump. Neither do new instance keys with a default.

Then add one step to `migration::migrate`, for example:

```rust
if from < 2 {
    v1_to_v2(env)?;
}
```

A step must fit in one transaction. It may rewrite instance data and other bounded entries (config, members, the asset list, asset policies). Unbounded data, such as proposals and allowlist entries, must instead be converted lazily when it is read.

## Procedure

1. **Build and publish the release.** Tag it and let the release workflow publish the Wasm and its SHA-256. Check it locally with `./scripts/verify-build.sh <sha256>`.
2. **Rehearse against the live state.** Take the snapshot shortly before proposing:

   ```sh
   NETWORK=mainnet ./scripts/upgrade-rehearsal.sh C...TREASURY
   ```

   The rehearsal upgrades a snapshot of the real treasury through governance, then runs `migrate()` within mainnet instruction limits. It checks that config, members, policies and every proposal survive and that writes work again. Do not propose an upgrade whose rehearsal fails.
3. **Dry-run on testnet.** Run the same rehearsal against a testnet treasury, or do the real upgrade there first:
   - deploy the old release,
   - run `./scripts/testnet-demo.sh`-style traffic against it,
   - then upgrade and migrate it.
4. **Upload the Wasm:** `stellar contract upload --wasm stellarsafe_treasury.wasm --network mainnet --source <account>`. The printed hash must equal the release SHA-256.
5. **Propose** `Upgrade(<hash>)` with a TTL longer than `governance_delay`. Put the release link in the off-chain description and pass its hash as `metadata_hash`.
6. **Approve and wait.** When the threshold is reached, `proposal_ready` is emitted. Guardians and members should verify the hash during the timelock, and veto if anything is wrong.
7. **Execute, then migrate right away.** Call `migrate()` in the next transaction, and confirm that `storage_version()` equals the new code's `STORAGE_VERSION`. If you skip this, every write fails with `MigrationRequired` until someone calls it.
8. **Verify** with `NETWORK=mainnet ./scripts/verify-build.sh C...TREASURY`, then check `contract_version()`.

## If something goes wrong

- **The upgrade proposal is vetoed or expires:** nothing changed. Fix it and propose again.
- **`migrate()` fails on-chain:** writes stay blocked, and governance cannot run either. That is why step 2 is required. Recovery would need code that the treasury can no longer adopt, so treat a failed rehearsal as a hard stop.
- **The new code misbehaves but storage is fine:** propose `Upgrade` back to the previous release. That only works if the previous release can still read the current layout (see the downgrade note above).
