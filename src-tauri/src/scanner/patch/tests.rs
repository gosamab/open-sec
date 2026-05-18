use super::*;
use crate::error::ProviderResult;
use crate::providers::{ContentBlock, GenerationRequest, Response, StopReason, Usage};
use crate::scanner::verify::Verdict;
use crate::scanner::Severity;
use async_trait::async_trait;
use std::sync::Mutex;
use tempfile::TempDir;

fn mk_finding(file: &str, kind: FindingKind) -> Finding {
    let mut f = Finding {
        id: String::new(),
        kind,
        severity: Severity::High,
        cwe: "CWE-89".into(),
        owasp: None,
        title: "T".into(),
        file: file.into(),
        line_start: 2,
        line_end: 2,
        description: "d".into(),
        data_flow: "src→sink".into(),
    };
    f.assign_id();
    f
}

// --- locate ---------------------------------------------------------

#[test]
fn locate_exact_match_single_line() {
    let src = "line a\nline b\nline c\n";
    match locate(src, "line b\n") {
        Located::Exact {
            byte_offset,
            line_start,
            line_end,
        } => {
            assert_eq!(byte_offset, 7);
            assert_eq!(line_start, 2);
            assert_eq!(line_end, 2);
        }
        other => panic!("expected Exact, got {other:?}"),
    }
}

#[test]
fn locate_exact_match_multi_line() {
    let src = "a\nb\nc\nd\n";
    match locate(src, "b\nc\n") {
        Located::Exact {
            line_start,
            line_end,
            ..
        } => {
            assert_eq!(line_start, 2);
            assert_eq!(line_end, 3);
        }
        other => panic!("expected Exact, got {other:?}"),
    }
}

#[test]
fn locate_fuzzy_on_indent_drift() {
    // Source has 4-space indent; model emitted 2-space indent.
    let src = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
    let needle = "  let x = 1;\n  let y = 2;\n";
    match locate(src, needle) {
        Located::Fuzzy {
            line_start,
            line_end,
            matched_text,
            ..
        } => {
            assert_eq!(line_start, 2);
            assert_eq!(line_end, 3);
            assert_eq!(matched_text, "    let x = 1;\n    let y = 2;\n");
        }
        other => panic!("expected Fuzzy, got {other:?}"),
    }
}

#[test]
fn locate_fuzzy_on_trailing_whitespace() {
    let src = "a   \nb\nc\n";
    // needle has no trailing whitespace on the first line
    match locate(src, "a\nb\n") {
        Located::Fuzzy { .. } => (),
        other => panic!("expected Fuzzy, got {other:?}"),
    }
}

#[test]
fn locate_not_found() {
    let src = "hello\nworld\n";
    assert!(matches!(locate(src, "goodbye"), Located::NotFound));
}

// --- diff -----------------------------------------------------------

#[test]
fn diff_round_trip_single_line() {
    let src = "fn main() {\n    let x = 1;\n}\n";
    let new_block = "    let x = 2;\n";
    let off = src.find("    let x = 1;\n").unwrap();
    let diff = synth_diff("src/lib.rs", src, off, "    let x = 1;\n".len(), new_block);
    assert!(diff.starts_with("--- src/lib.rs\n+++ src/lib.rs\n"));
    // Default diffy header must not leak through.
    assert!(!diff.contains("--- original"));
    assert!(diff.contains("-    let x = 1;"));
    assert!(diff.contains("+    let x = 2;"));
}

#[test]
fn finalize_produces_diff_for_exact_match() {
    let src = "a\nb\nc\n";
    let finding = mk_finding("focus.ts", FindingKind::Vuln);
    let proposal = PatchProposal {
        file: "focus.ts".into(),
        anchor_line: 2,
        old_block: "b\n".into(),
        new_block: "B\n".into(),
        explanation: "x".into(),
    };
    let patch = finalize(&finding, proposal, src);
    assert!(matches!(patch.located, Located::Exact { .. }));
    let diff = patch.diff.unwrap();
    assert!(diff.contains("-b"));
    assert!(diff.contains("+B"));
}

#[test]
fn finalize_produces_no_diff_on_not_found() {
    let src = "a\nb\nc\n";
    let finding = mk_finding("focus.ts", FindingKind::Vuln);
    let proposal = PatchProposal {
        file: "focus.ts".into(),
        anchor_line: 1,
        old_block: "nope".into(),
        new_block: "still nope".into(),
        explanation: "x".into(),
    };
    let patch = finalize(&finding, proposal, src);
    assert!(matches!(patch.located, Located::NotFound));
    assert!(patch.diff.is_none());
}

// --- should_patch ---------------------------------------------------

fn mk_vf(kind: FindingKind, verify_keep: Option<bool>) -> VerifiedFinding {
    let finding = mk_finding("focus.ts", kind);
    let verdict = verify_keep.map(|keep| Verdict {
        is_reachable: keep,
        source_is_untrusted: true,
        concrete_exploit: if keep {
            Some(crate::scanner::verify::Exploit {
                kind: crate::scanner::verify::ExploitKind::Other,
                request: None,
                payload: "x".into(),
                expected_effect: "y".into(),
            })
        } else {
            None
        },
        reasoning: "r".into(),
    });
    VerifiedFinding { finding, verdict }
}

#[test]
fn should_patch_decisions() {
    assert!(should_patch(&mk_vf(FindingKind::Vuln, Some(true))));
    assert!(!should_patch(&mk_vf(FindingKind::Vuln, Some(false))));
    assert!(!should_patch(&mk_vf(FindingKind::Vuln, None)));
    assert!(should_patch(&mk_vf(FindingKind::Hardening, None)));
}

// --- propose_many end-to-end ---------------------------------------

struct OneShotProvider {
    body: Mutex<Option<String>>,
}

impl OneShotProvider {
    fn new(body: &str) -> Self {
        Self {
            body: Mutex::new(Some(body.into())),
        }
    }
}

#[async_trait]
impl Provider for OneShotProvider {
    fn name(&self) -> &'static str {
        "oneshot"
    }
    async fn generate(&self, _req: GenerationRequest) -> ProviderResult<Response> {
        let body = self
            .body
            .lock()
            .unwrap()
            .clone()
            .expect("provider hit unexpectedly more than once");
        Ok(Response {
            id: "msg".into(),
            model: "oneshot".into(),
            content: vec![ContentBlock::Text { text: body }],
            stop_reason: Some(StopReason::EndTurn),
            stop_sequence: None,
            usage: Usage::default(),
        })
    }
}

#[tokio::test]
async fn propose_many_filters_and_patches() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::fs::write(root.join("focus.ts"), "a\nb\nc\n").unwrap();

    // Only the KEEP'd vuln triggers a provider call; the dropped vuln is
    // skipped without hitting the model.
    let provider: Arc<dyn Provider> = Arc::new(OneShotProvider::new(
        r#"{"file":"focus.ts","anchor_line":2,"old_block":"b\n","new_block":"B\n","explanation":"x"}"#,
    ));

    let verified = vec![
        mk_vf(FindingKind::Vuln, Some(false)),
        mk_vf(FindingKind::Vuln, Some(true)),
    ];
    let out = propose_many(verified, root, provider, "oneshot", 2).await;
    assert_eq!(out.len(), 1);
    assert!(out[0].diff.is_some());
}
