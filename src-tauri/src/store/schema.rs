//! Forward-only schema definition for the SQLite store. Each `SCHEMA_V*`
//! constant is run exactly once via the `PRAGMA user_version` ladder in
//! `super::Store::migrate`. Never edit a shipped migration — add a new one.

pub(super) const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS scans (
    id                     TEXT PRIMARY KEY,
    root                   TEXT NOT NULL,
    started_at             INTEGER NOT NULL,
    finished_at            INTEGER,
    status                 TEXT NOT NULL,
    total_findings         INTEGER NOT NULL DEFAULT 0,
    kept_findings          INTEGER NOT NULL DEFAULT 0,
    hardening_findings     INTEGER NOT NULL DEFAULT 0,
    walk_json              TEXT NOT NULL DEFAULT '{"candidates":[],"skipped":[]}',
    triaged_json           TEXT NOT NULL DEFAULT '[]',
    usage_json             TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS scans_root_started_idx ON scans(root, started_at DESC);

CREATE TABLE IF NOT EXISTS findings (
    scan_id        TEXT NOT NULL,
    finding_id     TEXT NOT NULL,
    rel_path       TEXT NOT NULL,
    kind           TEXT NOT NULL,
    severity       TEXT NOT NULL,
    cwe            TEXT NOT NULL,
    owasp          TEXT,
    title          TEXT NOT NULL,
    file           TEXT NOT NULL,
    line_start     INTEGER NOT NULL,
    line_end       INTEGER NOT NULL,
    description    TEXT NOT NULL,
    data_flow      TEXT NOT NULL,
    verdict_json   TEXT,
    patch_json     TEXT,
    PRIMARY KEY (scan_id, finding_id),
    FOREIGN KEY (scan_id) REFERENCES scans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS findings_finding_id_idx ON findings(finding_id);

CREATE TABLE IF NOT EXISTS triage (
    finding_id     TEXT NOT NULL,
    root           TEXT NOT NULL,
    status         TEXT NOT NULL,
    reason         TEXT,
    snooze_until   INTEGER,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (finding_id, root)
);
"#;

pub(super) const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS applied_patches (
    finding_id  TEXT NOT NULL,
    root        TEXT NOT NULL,
    file        TEXT NOT NULL,
    applied_at  INTEGER NOT NULL,
    PRIMARY KEY (finding_id, root)
);
"#;

pub(super) const SCHEMA_V3: &str = r#"
ALTER TABLE scans ADD COLUMN detect_errors_json TEXT NOT NULL DEFAULT '[]';
"#;

pub(super) const SCHEMA_V4: &str = r#"
ALTER TABLE scans ADD COLUMN durations_json TEXT NOT NULL DEFAULT '{}';
"#;

// Drop columns + index that were written but never read.
pub(super) const SCHEMA_V5: &str = r#"
DROP INDEX IF EXISTS findings_finding_id_idx;
ALTER TABLE scans DROP COLUMN finished_at;
ALTER TABLE scans DROP COLUMN hardening_findings;
"#;
