//! Storage layout versioning for upgrades.
//!
//! `STORAGE_VERSION` names the layout this code reads and writes, and the
//! layout of the stored data is recorded next to it. After an `Upgrade` to
//! code with a newer layout, every write is refused with `MigrationRequired`
//! until someone calls `migrate`. New code therefore never acts on data it
//! hasn't converted.
//!
//! `migrate` is permissionless and idempotent. It only runs the conversion
//! steps built into the code, which governance already approved by
//! approving the upgrade.

use soroban_sdk::Env;

use crate::errors::Error;
use crate::storage as st;
use crate::types::STORAGE_VERSION;

/// Run at the start of every write entry point.
///
/// Data in a *newer* layout than this code (after a downgrade) is not
/// refused: blocking it would also block the governance upgrade needed to
/// move forward again. A downgrade must only target code that can still
/// read the newer layout.
pub fn enter(env: &Env) -> Result<(), Error> {
    st::bump_instance(env);
    if st::get_storage_version(env) < STORAGE_VERSION {
        return Err(Error::MigrationRequired);
    }
    Ok(())
}

/// Brings stored data up to `STORAGE_VERSION`. Returns the version the data
/// had before, or `None` if it was already current.
pub fn migrate(env: &Env) -> Result<Option<u32>, Error> {
    st::bump_instance(env);
    let from = st::get_storage_version(env);
    if from > STORAGE_VERSION {
        return Err(Error::UnsupportedStorageVersion);
    }
    if from == STORAGE_VERSION {
        return Ok(None);
    }
    // Add one step per layout change, oldest first, for example:
    //     if from < 2 { v1_to_v2(env)?; }
    // A step must fit in one transaction, so it can only rewrite instance
    // data and other bounded entries. Convert unbounded persistent data
    // (proposals, allowlist entries) lazily when it is read instead.
    st::set_storage_version(env, STORAGE_VERSION);
    Ok(Some(from))
}
