//! Export a `ScanResult` to markdown or SARIF v2.1.0. Both formats reflect
//! the post-verify state — dropped vulns are excluded, hardening items
//! appear as `note`-level. Patches are inlined for markdown and surfaced
//! under SARIF `fixes`.

use serde_json::json;

use crate::scanner::orchestrate::ScanResult;
use crate::scanner::patch::Patch;
use crate::scanner::verify::VerifiedFinding;
use crate::scanner::{FindingKind, Severity};

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "critical",
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
        Severity::Info => "info",
    }
}

/// SARIF only knows `error / warning / note / none`. Map our finer-grained
/// severities to those buckets.
fn sarif_level(s: Severity) -> &'static str {
    match s {
        Severity::Critical | Severity::High => "error",
        Severity::Medium => "warning",
        Severity::Low | Severity::Info => "note",
    }
}

fn is_displayed(v: &VerifiedFinding) -> bool {
    matches!(v.finding.kind, FindingKind::Hardening)
        || v.verdict.as_ref().map(|x| x.keep()).unwrap_or(false)
}

pub fn export_markdown(result: &ScanResult) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    writeln!(out, "# Security scan report\n").ok();
    writeln!(out, "- **Root:** `{}`", result.root.display()).ok();
    writeln!(
        out,
        "- **Files scanned:** {}",
        result.findings_by_file.len()
    )
    .ok();

    let displayed: Vec<&VerifiedFinding> =
        result.verified.iter().filter(|v| is_displayed(v)).collect();
    let kept = displayed
        .iter()
        .filter(|v| matches!(v.finding.kind, FindingKind::Vuln))
        .count();
    let hardening = displayed
        .iter()
        .filter(|v| matches!(v.finding.kind, FindingKind::Hardening))
        .count();
    let dropped = result.verified.len() - displayed.len();
    writeln!(
        out,
        "- **Findings:** {} kept · {} hardening · {} dropped by verifier",
        kept, hardening, dropped
    )
    .ok();
    let u = &result.usage.total;
    writeln!(
        out,
        "- **Tokens:** input={}, output={}, cache_read={}",
        u.input_tokens, u.output_tokens, u.cache_read_input_tokens
    )
    .ok();
    writeln!(out).ok();

    if displayed.is_empty() {
        writeln!(out, "_No findings to report._").ok();
        return out;
    }

    let patches: std::collections::HashMap<&str, &Patch> = result
        .patches
        .iter()
        .map(|p| (p.finding_id.as_str(), p))
        .collect();

    for v in &displayed {
        let f = &v.finding;
        let sev = severity_label(f.severity);
        let kind = match f.kind {
            FindingKind::Vuln => "VULN",
            FindingKind::Hardening => "HARDENING",
        };
        writeln!(out, "---\n").ok();
        writeln!(out, "## [{}] {} · {}", sev.to_uppercase(), f.cwe, f.title).ok();
        writeln!(
            out,
            "_{}_ · `{}:{}-{}`",
            kind, f.file, f.line_start, f.line_end
        )
        .ok();
        if let Some(o) = &f.owasp {
            writeln!(out, "_OWASP {}_", o).ok();
        }
        writeln!(out).ok();
        writeln!(out, "{}\n", f.description).ok();
        writeln!(out, "**Data flow:** {}\n", f.data_flow).ok();

        if let Some(verdict) = &v.verdict {
            writeln!(out, "### Verifier").ok();
            writeln!(
                out,
                "- reachable: `{}` · untrusted source: `{}`",
                verdict.is_reachable, verdict.source_is_untrusted
            )
            .ok();
            writeln!(out, "\n{}\n", verdict.reasoning).ok();
            if let Some(ex) = &verdict.concrete_exploit {
                writeln!(out, "### Exploit").ok();
                writeln!(
                    out,
                    "- kind: `{:?}` · effect: {}",
                    ex.kind, ex.expected_effect
                )
                .ok();
                if let Some(req) = &ex.request {
                    writeln!(out, "- request: `{} {}`", req.method, req.path).ok();
                }
                writeln!(out, "- payload: `{}`\n", ex.payload).ok();
            }
        }

        if let Some(patch) = patches.get(f.id.as_str()) {
            writeln!(out, "### Suggested patch").ok();
            writeln!(out, "{}\n", patch.proposal.explanation).ok();
            if let Some(diff) = &patch.diff {
                writeln!(out, "```diff\n{}\n```\n", diff.trim_end()).ok();
            } else {
                writeln!(out, "_old_block not located in current file._\n").ok();
                writeln!(out, "```\n- {}\n+ {}\n```\n", patch.proposal.old_block, patch.proposal.new_block).ok();
            }
        }
    }

    out
}

/// Minimal SARIF v2.1.0 envelope. Targets GitHub code-scanning compatibility.
pub fn export_sarif(result: &ScanResult) -> String {
    let displayed: Vec<&VerifiedFinding> =
        result.verified.iter().filter(|v| is_displayed(v)).collect();
    let patches: std::collections::HashMap<&str, &Patch> = result
        .patches
        .iter()
        .map(|p| (p.finding_id.as_str(), p))
        .collect();

    let rules: Vec<_> = {
        let mut seen = std::collections::BTreeMap::new();
        for v in &displayed {
            seen.entry(v.finding.cwe.clone()).or_insert_with(|| {
                json!({
                    "id": v.finding.cwe,
                    "name": v.finding.cwe,
                    "shortDescription": { "text": v.finding.cwe },
                    "helpUri": format!("https://cwe.mitre.org/data/definitions/{}.html",
                        v.finding.cwe.trim_start_matches("CWE-")),
                })
            });
        }
        seen.into_values().collect()
    };

    let results_json: Vec<_> = displayed
        .iter()
        .map(|v| {
            let f = &v.finding;
            let mut body = json!({
                "ruleId": f.cwe,
                "level": sarif_level(f.severity),
                "message": { "text": format!("{}\n\n{}\n\nData flow: {}", f.title, f.description, f.data_flow) },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": f.file },
                        "region": { "startLine": f.line_start, "endLine": f.line_end }
                    }
                }],
                "properties": {
                    "severity": severity_label(f.severity),
                    "kind": match f.kind { FindingKind::Vuln => "vuln", FindingKind::Hardening => "hardening" },
                    "owasp": f.owasp,
                    "finding_id": f.id,
                }
            });
            // Attach fix if we have a patch with a located block.
            if let Some(patch) = patches.get(f.id.as_str()) {
                if patch.diff.is_some() {
                    body["fixes"] = json!([{
                        "description": { "text": patch.proposal.explanation },
                        "artifactChanges": [{
                            "artifactLocation": { "uri": patch.proposal.file },
                            "replacements": [{
                                "deletedRegion": {
                                    "startLine": patch.proposal.anchor_line,
                                },
                                "insertedContent": { "text": patch.proposal.new_block }
                            }]
                        }]
                    }]);
                }
            }
            body
        })
        .collect();

    let doc = json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "open-sec",
                    "informationUri": "https://github.com/anthropics/open-sec",
                    "rules": rules,
                }
            },
            "results": results_json,
            "properties": {
                "root": result.root.display().to_string(),
                "tokens": {
                    "input": result.usage.total.input_tokens,
                    "output": result.usage.total.output_tokens,
                    "cache_read": result.usage.total.cache_read_input_tokens,
                }
            }
        }]
    });

    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::ingest::WalkResult;
    use crate::scanner::orchestrate::{FileFindings, StageUsage};
    use crate::scanner::verify::{Exploit, ExploitKind, Verdict};
    use crate::scanner::Finding;

    fn mk_result() -> ScanResult {
        let mut f = Finding {
            id: String::new(),
            kind: FindingKind::Vuln,
            severity: Severity::High,
            cwe: "CWE-89".into(),
            owasp: Some("A03:2021".into()),
            title: "SQL Injection".into(),
            file: "/p/src/app.ts".into(),
            line_start: 10,
            line_end: 14,
            description: "interpolated `id` into raw SQL".into(),
            data_flow: "req.params.id → format! → db.query".into(),
        };
        f.assign_id();
        ScanResult {
            root: std::path::PathBuf::from("/p"),
            ingest: WalkResult::default(),
            triaged: Vec::new(),
            triage_errors: Vec::new(),
            findings_by_file: vec![FileFindings {
                path: std::path::PathBuf::from("/p/src/app.ts"),
                rel_path: "src/app.ts".into(),
                findings: vec![f.clone()],
            }],
            detect_errors: Vec::new(),
            verified: vec![VerifiedFinding {
                finding: f,
                verdict: Some(Verdict {
                    is_reachable: true,
                    source_is_untrusted: true,
                    concrete_exploit: Some(Exploit {
                        kind: ExploitKind::Http,
                        request: None,
                        payload: "1 OR 1=1--".into(),
                        expected_effect: "auth bypass".into(),
                    }),
                    reasoning: "reachable from /users/:id".into(),
                }),
            }],
            patches: Vec::new(),
            usage: StageUsage::default(),
            durations: Default::default(),
            status: Default::default(),
        }
    }

    #[test]
    fn markdown_export_renders_header_and_finding() {
        let md = export_markdown(&mk_result());
        assert!(md.contains("# Security scan report"));
        assert!(md.contains("[HIGH] CWE-89"));
        assert!(md.contains("SQL Injection"));
        assert!(md.contains("`/p/src/app.ts:10-14`"));
        assert!(md.contains("### Verifier"));
    }

    #[test]
    fn sarif_export_is_valid_json_with_expected_keys() {
        let sarif = export_sarif(&mk_result());
        let v: serde_json::Value = serde_json::from_str(&sarif).unwrap();
        assert_eq!(v["version"], "2.1.0");
        let runs = v["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["tool"]["driver"]["name"], "open-sec");
        let results = runs[0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ruleId"], "CWE-89");
        assert_eq!(results[0]["level"], "error");
        assert!(results[0]["message"]["text"]
            .as_str()
            .unwrap()
            .contains("SQL Injection"));
    }
}
