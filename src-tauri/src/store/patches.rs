//! `applied_patches` table: tracks which patches the user has actually
//! written back to disk, so the "applied" badge survives reloads.

use anyhow::Result;
use rusqlite::params;

use super::types::{now_ms, AppliedPatchRecord};
use super::Store;

impl Store {
    pub fn record_patch_applied(&self, finding_id: &str, root: &str, file: &str) -> Result<()> {
        let conn = self.db();
        conn.execute(
            "INSERT INTO applied_patches (finding_id, root, file, applied_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(finding_id, root) DO UPDATE SET
                 file = excluded.file,
                 applied_at = excluded.applied_at",
            params![finding_id, root, file, now_ms()],
        )?;
        Ok(())
    }

    pub fn get_applied_for_root(&self, root: &str) -> Result<Vec<AppliedPatchRecord>> {
        let conn = self.db();
        let mut stmt = conn.prepare(
            "SELECT finding_id, file, applied_at FROM applied_patches WHERE root = ?1",
        )?;
        let rows = stmt
            .query_map(params![root], |row| {
                Ok(AppliedPatchRecord {
                    finding_id: row.get(0)?,
                    file: row.get(1)?,
                    applied_at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
