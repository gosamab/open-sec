//! SQLite-backed persistence for scan history. Lives at
//! `<app_data_dir>/open-sec.db`. Schema is migrated forward via PRAGMA
//! user_version so future shape changes don't require manual DB resets.
//!
//! Complex per-finding payloads (`Verdict`, `Patch`) are stored as serialized
//! JSON in columns rather than fully normalised tables — we always read them
//! together with their finding, and there's no querying into their internals.
//!
//! Layout:
//!   - `schema`  — versioned SQL constants (read by `migrate`)
//!   - `types`   — public types returned across the IPC boundary, plus the
//!                 small `kind`/`severity` <-> TEXT converters used by both
//!                 `scans.rs` and `tests.rs`
//!   - `scans`   — `scans` + `findings` tables (the bulk of the SQL)
//!   - `triage`  — per-(finding, root) user decisions
//!   - `patches` — applied-patch ledger
//!
//! The `Store` struct itself lives here; each table module adds an
//! `impl Store` block in its own file.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::Connection;

mod patches;
mod scans;
mod schema;
mod triage;
mod types;

#[cfg(test)]
mod tests;

pub use types::{
    new_scan_id, now_ms, AppliedPatchRecord, ScanGroup, TriageRecord, TriageStatus,
};

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Acquire the connection mutex, recovering from a previous thread's
    /// panic. SQLite operations are transactional so a poisoned lock only
    /// means a holder panicked, not that data is corrupt.
    pub(super) fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Open (or create) the SQLite database at `path` and run any pending
    /// schema migrations. Any scan rows still in `running` from a previous
    /// crash / kill are flipped to `cancelled` so the recents list doesn't
    /// keep displaying them as in-progress forever.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating DB dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite at {}", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        store.finalize_stale_running_scans()?;
        Ok(store)
    }

    /// Flip any leftover `running` rows to `cancelled`. Runs once per
    /// `open()` — by the time we reach this code, no scan can possibly be
    /// running yet, so anything still tagged `running` is from a previous
    /// process that exited without finalizing.
    fn finalize_stale_running_scans(&self) -> Result<()> {
        let conn = self.db();
        conn.execute(
            "UPDATE scans SET status = 'cancelled' WHERE status = 'running'",
            [],
        )?;
        Ok(())
    }

    /// In-memory store, for tests.
    #[cfg(test)]
    fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.db();
        let current: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current < 1 {
            let tx = conn.transaction()?;
            tx.execute_batch(schema::SCHEMA_V1)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        if current < 2 {
            let tx = conn.transaction()?;
            tx.execute_batch(schema::SCHEMA_V2)?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        if current < 3 {
            let tx = conn.transaction()?;
            tx.execute_batch(schema::SCHEMA_V3)?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        if current < 4 {
            let tx = conn.transaction()?;
            tx.execute_batch(schema::SCHEMA_V4)?;
            tx.pragma_update(None, "user_version", 4)?;
            tx.commit()?;
        }
        if current < 5 {
            let tx = conn.transaction()?;
            tx.execute_batch(schema::SCHEMA_V5)?;
            tx.pragma_update(None, "user_version", 5)?;
            tx.commit()?;
        }
        Ok(())
    }
}
