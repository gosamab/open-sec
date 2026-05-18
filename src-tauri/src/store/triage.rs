//! `triage` table: per-(finding, root) user decisions (accept/dismiss/snooze)
//! that persist across re-scans.

use anyhow::Result;
use rusqlite::params;

use super::types::{now_ms, TriageRecord, TriageStatus};
use super::Store;

impl Store {
    /// Upsert a triage decision for (finding_id, root). Re-calling this with
    /// a different status overwrites the prior decision. `reason` is required
    /// for `dismissed`; `snooze_until` is required for `snoozed` but those
    /// constraints are policed at the IPC boundary, not here.
    pub fn set_triage(
        &self,
        finding_id: &str,
        root: &str,
        status: TriageStatus,
        reason: Option<&str>,
        snooze_until: Option<i64>,
    ) -> Result<()> {
        let conn = self.db();
        conn.execute(
            "INSERT INTO triage (finding_id, root, status, reason, snooze_until, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(finding_id, root) DO UPDATE SET
                 status = excluded.status,
                 reason = excluded.reason,
                 snooze_until = excluded.snooze_until,
                 updated_at = excluded.updated_at",
            params![
                finding_id,
                root,
                status.as_str(),
                reason,
                snooze_until,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    pub fn clear_triage(&self, finding_id: &str, root: &str) -> Result<()> {
        let conn = self.db();
        conn.execute(
            "DELETE FROM triage WHERE finding_id = ?1 AND root = ?2",
            params![finding_id, root],
        )?;
        Ok(())
    }

    /// Load every triage decision for a root. The returned list is suitable
    /// for the frontend to index by `finding_id` and overlay on findings as
    /// they're rendered.
    pub fn get_triage_for_root(&self, root: &str) -> Result<Vec<TriageRecord>> {
        let conn = self.db();
        let mut stmt = conn.prepare(
            "SELECT finding_id, status, reason, snooze_until, updated_at
             FROM triage WHERE root = ?1",
        )?;
        let rows = stmt
            .query_map(params![root], |row| {
                let status_str: String = row.get(1)?;
                Ok((
                    row.get::<_, String>(0)?,
                    status_str,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(finding_id, status, reason, snooze_until, updated_at)| {
                Ok(TriageRecord {
                    finding_id,
                    status: TriageStatus::from_str(&status)?,
                    reason,
                    snooze_until,
                    updated_at,
                })
            })
            .collect()
    }
}
