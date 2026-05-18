use super::*;
use crate::scanner::ingest::WalkResult;
use crate::scanner::orchestrate::{FileFindings, ScanResult, StageUsage};
use crate::scanner::verify::{Verdict, VerifiedFinding};
use crate::scanner::{Finding, Severity};

fn mk_finding(id_seed: &str, sev: Severity) -> Finding {
    let mut f = Finding {
        id: String::new(),
        kind: crate::scanner::FindingKind::Vuln,
        severity: sev,
        cwe: "CWE-89".into(),
        owasp: Some("A03:2021".into()),
        title: format!("Test {id_seed}"),
        file: "/abs/src/foo.ts".into(),
        line_start: 10,
        line_end: 12,
        description: "desc".into(),
        data_flow: "src→sink".into(),
    };
    f.assign_id();
    f
}

fn mk_result() -> ScanResult {
    let finding = mk_finding("a", Severity::High);
    ScanResult {
        root: std::path::PathBuf::from("/tmp/proj"),
        ingest: WalkResult::default(),
        triaged: Vec::new(),
        findings_by_file: vec![FileFindings {
            path: std::path::PathBuf::from("/abs/src/foo.ts"),
            rel_path: "src/foo.ts".into(),
            findings: vec![finding.clone()],
        }],
        detect_errors: Vec::new(),
        verified: vec![VerifiedFinding {
            finding,
            verdict: Some(Verdict {
                is_reachable: true,
                source_is_untrusted: true,
                concrete_exploit: None,
                reasoning: "r".into(),
            }),
        }],
        patches: Vec::new(),
        usage: StageUsage::default(),
        durations: Default::default(),
        status: Default::default(),
    }
}

#[test]
fn round_trip_save_load() {
    let store = Store::open_in_memory().unwrap();
    let result = mk_result();
    let scan_id = store.save_scan(&result, "completed").unwrap();
    let loaded = store.load_scan(&scan_id).unwrap();
    assert_eq!(loaded.verified.len(), 1);
    assert_eq!(loaded.verified[0].finding.title, "Test a");
    assert!(loaded.verified[0].verdict.as_ref().unwrap().is_reachable);
    assert_eq!(loaded.findings_by_file.len(), 1);
    assert_eq!(loaded.findings_by_file[0].rel_path, "src/foo.ts");
    assert_eq!(loaded.root, std::path::PathBuf::from("/tmp/proj"));
}

#[test]
fn list_scan_groups_returns_latest_per_root() {
    let store = Store::open_in_memory().unwrap();
    let result = mk_result();
    let _id1 = store.save_scan(&result, "completed").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let id2 = store.save_scan(&result, "completed").unwrap();

    let groups = store.list_scan_groups(20).unwrap();
    assert_eq!(groups.len(), 1, "same root must collapse to one group");
    assert_eq!(groups[0].latest_scan_id, id2);
}

#[test]
fn migrations_are_idempotent() {
    let store = Store::open_in_memory().unwrap();
    // Second migrate call should be a no-op.
    store.migrate().unwrap();
    store.migrate().unwrap();
}

#[test]
fn triage_round_trip_and_upsert() {
    let store = Store::open_in_memory().unwrap();
    let root = "/tmp/proj";

    store
        .set_triage("abc", root, TriageStatus::Dismissed, Some("false positive"), None)
        .unwrap();
    // Same key, different status — must overwrite, not error.
    store
        .set_triage("abc", root, TriageStatus::Accepted, None, None)
        .unwrap();
    store
        .set_triage("def", root, TriageStatus::Snoozed, None, Some(9_999_999))
        .unwrap();
    // Different root: must not appear in the per-root query below.
    store
        .set_triage("abc", "/tmp/other", TriageStatus::Dismissed, Some("x"), None)
        .unwrap();

    let mut got = store.get_triage_for_root(root).unwrap();
    got.sort_by(|a, b| a.finding_id.cmp(&b.finding_id));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].finding_id, "abc");
    assert_eq!(got[0].status, TriageStatus::Accepted);
    assert_eq!(got[0].reason, None);
    assert_eq!(got[1].finding_id, "def");
    assert_eq!(got[1].status, TriageStatus::Snoozed);
    assert_eq!(got[1].snooze_until, Some(9_999_999));

    // Clear leaves the other row alone.
    store.clear_triage("abc", root).unwrap();
    let remaining = store.get_triage_for_root(root).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].finding_id, "def");
}
