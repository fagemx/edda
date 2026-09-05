//! Evidence is collected by the host; reviewed text never grants execution.
mod github;
mod probes;
mod process;
mod trust;

pub(crate) use github::gh_required_checks;
pub(crate) use probes::{extract_probe_verbs, run_probes, run_wiring_scan};
pub(crate) use trust::{extract_verify, spec_trust, SpecOrigin};

use super::brief::FrontMatter;
use edda_core::types::{ReviewGateRan, ReviewGateRead, ReviewProbe};
use edda_ledger::{paths::EddaPaths, Ledger};
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) struct GateSet {
    /// Original executable strings. Normalization is exclusively for READ.
    pub cmds: Vec<String>,
    pub declared_by: Vec<String>,
}

pub(crate) fn normalize_cmd(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn gate_set(fm: &FrontMatter, cli: &[String], verify: &[String]) -> GateSet {
    let mut gates = GateSet {
        cmds: vec![],
        declared_by: vec![],
    };
    for (source, commands) in [
        ("REVIEW.md", fm.gates.as_slice()),
        ("--gate", cli),
        ("spec.verify", verify),
    ] {
        if commands.iter().any(|s| !s.trim().is_empty()) {
            gates.declared_by.push(source.into());
        }
        for command in commands {
            if !command.trim().is_empty() && !gates.cmds.contains(command) {
                gates.cmds.push(command.clone());
            }
        }
    }
    gates
}

pub(crate) type GateRead = (String, Vec<ReviewGateRead>, Vec<String>);

pub(crate) fn read_gates(ledger: &Ledger, head: &str, gates: &GateSet) -> anyhow::Result<GateRead> {
    if gates.cmds.is_empty() {
        return Ok(("undeclared".into(), vec![], vec![]));
    }
    let events = ledger.iter_events_by_type("cmd")?;
    let mut read = Vec::new();
    let mut uncovered = Vec::new();
    for gate in &gates.cmds {
        let best = events.iter().rev().find(|event| {
            let payload = &event.payload;
            payload["git_sha"].as_str() == Some(head)
                && payload["tree_dirty"].as_bool() == Some(false)
                && payload["argv"]
                    .as_array()
                    .and_then(|args| args.iter().map(|a| a.as_str()).collect::<Option<Vec<_>>>())
                    .is_some_and(|args| normalize_cmd(&args.join(" ")) == normalize_cmd(gate))
        });
        if let Some(event) = best {
            read.push(ReviewGateRead {
                kind: "cmd-event".into(),
                r#ref: event.event_id.clone(),
                cmd: gate.clone(),
                result: if event.payload["exit_code"].as_i64() == Some(0) {
                    "green"
                } else {
                    "red"
                }
                .into(),
            });
        } else {
            uncovered.push(gate.clone());
        }
    }
    let status = if read.iter().any(|r| r.result == "red") {
        "red"
    } else if uncovered.is_empty() {
        "verified"
    } else {
        "unverified"
    };
    Ok((status.into(), read, uncovered))
}

/// One lattice for every independent evidence source; silence is neutral.
pub(crate) fn combine_gate_status(current: &str, incoming: Option<&str>) -> String {
    let incoming = incoming.unwrap_or("unverified");
    if current == "undeclared" || incoming == "undeclared" {
        "undeclared"
    } else if current == "red" || incoming == "red" {
        "red"
    } else if current == "verified" || incoming == "verified" {
        "verified"
    } else {
        "unverified"
    }
    .into()
}

pub(crate) fn read_ci(checks: &[(String, String)]) -> (Option<String>, Vec<ReviewGateRead>) {
    if checks.is_empty() {
        return (None, vec![]);
    }
    let read: Vec<_> = checks
        .iter()
        .map(|(name, bucket)| ReviewGateRead {
            kind: "ci".into(),
            r#ref: name.clone(),
            cmd: name.clone(),
            result: match bucket.as_str() {
                "pass" => "green",
                "fail" | "cancel" => "red",
                _ => "pending",
            }
            .into(),
        })
        .collect();
    let status = if read.iter().any(|r| r.result == "red") {
        "red"
    } else if read.iter().all(|r| r.result == "green") {
        "verified"
    } else {
        "unverified"
    };
    (Some(status.into()), read)
}

pub(crate) fn ran_status(gates: &[String], ran: &[ReviewGateRan]) -> Option<String> {
    // A real failure remains red even if another command exceeded the budget.
    if ran.iter().any(|r| !r.timed_out && r.exit != 0) {
        return Some("red".into());
    }
    if !gates.is_empty()
        && gates.iter().all(|gate| {
            ran.iter()
                .any(|r| &r.cmd == gate && r.exit == 0 && !r.timed_out && r.stdout_blob.is_some())
        })
    {
        Some("verified".into())
    } else {
        None
    }
}

pub(crate) fn ran_gates(
    cwd: &Path,
    gates: &[String],
    deadline_secs: u64,
    cargo_target_dir_set: bool,
    paths: &EddaPaths,
    _out_dir: &Path,
) -> (Vec<ReviewGateRan>, Vec<String>) {
    let mut ran = Vec::new();
    let mut notes = Vec::new();
    let Some(deadline) = Instant::now().checked_add(Duration::from_secs(deadline_secs)) else {
        return (ran, vec!["invalid --max-ran-sec deadline".into()]);
    };
    for gate in gates {
        if normalize_cmd(gate).starts_with("cargo ") && !cargo_target_dir_set {
            notes.push(format!(
                "skipped `{gate}`: set CARGO_TARGET_DIR (a build lane) to run cargo gates"
            ));
            continue;
        }
        if Instant::now() >= deadline {
            notes.push(format!(
                "not run `{gate}`: --max-ran-sec {deadline_secs} exhausted"
            ));
            continue;
        }
        let started = Instant::now();
        let outcome = process::shell(gate, cwd, deadline);
        match outcome {
            Err(error) => notes.push(format!("cannot execute `{gate}`: {error}")),
            Ok(output) => {
                let stdout_blob = match edda_ledger::blob_store::blob_put(paths, &output.stdout) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        notes.push(format!(
                            "stdout blob for `{gate}` not stored: {error}; RAN cannot verify"
                        ));
                        None
                    }
                };
                if output.timed_out {
                    notes.push(format!(
                        "killed `{gate}` process tree at --max-ran-sec {deadline_secs}"
                    ));
                }
                ran.push(ReviewGateRan {
                    cmd: gate.clone(),
                    exit: output.exit,
                    duration_ms: started.elapsed().as_millis() as u64,
                    stdout_blob,
                    timed_out: output.timed_out,
                });
            }
        }
    }
    (ran, notes)
}

pub(crate) fn evidence_text(
    read: &[ReviewGateRead],
    uncovered: &[String],
    ran: &[ReviewGateRan],
    probes: &[ReviewProbe],
    wiring_scan: Option<&str>,
) -> String {
    let mut text = String::from("### Gates READ (exact head, clean tree receipts / required CI)\n");
    for r in read {
        text.push_str(&format!(
            "- {:?}: {} ({} {:?})\n",
            r.cmd, r.result, r.kind, r.r#ref
        ));
    }
    for command in uncovered {
        text.push_str(&format!("- {command:?}: not covered\n"));
    }
    text.push_str("### Gates RAN (host-executed)\n");
    for r in ran {
        text.push_str(&format!(
            "- EVIDENCE RAN: {}: exit {}, {} ms, blob {}, timed_out={}\n",
            r.cmd,
            r.exit,
            r.duration_ms,
            r.stdout_blob.as_deref().unwrap_or("NOT STORED"),
            r.timed_out
        ));
    }
    text.push_str("### Probes (host-executed help only)\n");
    for p in probes {
        text.push_str(&format!("- EVIDENCE PROBE: {}: exit {}\n", p.cmd, p.exit));
    }
    if let Some(scan) = wiring_scan {
        text.push_str("### wiring-scan (base version)\n");
        text.push_str(scan);
        text.push('\n');
    }
    let measures = checklist_measures(ran, probes);
    text.push_str("### Checklist measure IDs (copy exactly for checklist result=ran)\n");
    if measures.is_empty() {
        text.push_str("- none; use na for source inspection or escalate for unresolved judgment\n");
    } else {
        for measure in measures {
            text.push_str(&format!("- {measure}\n"));
        }
    }
    text
}

pub(crate) fn checklist_measures(ran: &[ReviewGateRan], probes: &[ReviewProbe]) -> Vec<String> {
    let mut measures = ran
        .iter()
        .filter(|row| !row.timed_out && row.stdout_blob.is_some())
        .map(|row| format!("EVIDENCE RAN: {}", row.cmd))
        .collect::<Vec<_>>();
    measures.extend(
        probes
            .iter()
            .map(|probe| format!("EVIDENCE PROBE: {}", probe.cmd)),
    );
    measures
}

#[cfg(test)]
mod tests;
