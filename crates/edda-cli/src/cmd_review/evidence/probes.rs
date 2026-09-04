use super::process;
use anyhow::{Context, Result};
use edda_core::types::ReviewProbe;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn verb_ok(verb: &str) -> bool {
    let mut chars = verb.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub(crate) fn extract_probe_verbs(
    diff: &str,
    spec: Option<&str>,
    bins: &[String],
) -> Vec<(String, String)> {
    let sources = diff
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .chain(spec.unwrap_or("").lines());
    let mut out = Vec::new();
    for line in sources {
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else {
                break;
            };
            let mut words = after[..end].split_whitespace();
            if let (Some(bin), Some(verb)) = (words.next(), words.next()) {
                if verb_ok(bin) && verb_ok(verb) && bins.iter().any(|b| b.trim() == bin) {
                    let pair = (bin.to_owned(), verb.to_owned());
                    if !out.contains(&pair) {
                        out.push(pair);
                    }
                }
            }
            rest = &after[end + 1..];
        }
    }
    out
}

pub(crate) fn run_probes(cwd: &Path, verbs: &[(String, String)]) -> Vec<ReviewProbe> {
    let deadline = Instant::now() + Duration::from_secs(30);
    verbs
        .iter()
        .map(|(bin, verb)| {
            // `git <verb> --help` can launch a browser; -h is terminal-only.
            let display = if bin == "git" {
                format!("git --no-pager {verb} -h")
            } else {
                format!("{bin} {verb} --help")
            };
            let result = (|| -> Result<process::Output> {
                anyhow::ensure!(verb_ok(bin) && verb_ok(verb), "invalid probe token");
                let executable = if bin == "edda" {
                    std::env::current_exe()?
                } else {
                    process::executable(bin)?
                };
                let mut command = Command::new(executable);
                if bin == "git" {
                    command.args(["--no-pager", verb, "-h"]);
                } else {
                    command.args([verb, "--help"]);
                }
                process::run(
                    &mut command,
                    cwd,
                    deadline.min(Instant::now() + Duration::from_secs(5)),
                    4000,
                )
            })();
            ReviewProbe {
                cmd: display,
                exit: result.map(|output| output.exit).unwrap_or(-1),
            }
        })
        .collect()
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        // Only a newly-created, unique directory owned by this invocation.
        let _ = std::fs::remove_file(self.0.join("wiring-scan.sh"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

pub(crate) fn run_wiring_scan(repo: &Path, base: &str, head: &str) -> Result<Option<String>> {
    for sha in [base, head] {
        anyhow::ensure!(
            sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "wiring scan requires full SHAs"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let git = process::executable("git")?;
    let object = format!("{base}:scripts/wiring-scan.sh");
    let exists = process::run(
        Command::new(&git).args(["cat-file", "-e", &object]),
        repo,
        deadline,
        4000,
    )?;
    anyhow::ensure!(!exists.timed_out, "wiring scan base lookup timed out");
    if exists.exit != 0 {
        return Ok(None);
    }
    let source = process::run(
        Command::new(&git).args(["show", &object]),
        repo,
        deadline,
        1024 * 1024,
    )?;
    anyhow::ensure!(
        source.exit == 0 && !source.timed_out && !source.truncated,
        "cannot read bounded base wiring-scan source"
    );
    let scratch = std::env::temp_dir().join(format!("edda-review-wiring-{}", ulid::Ulid::new()));
    std::fs::create_dir(&scratch).context("create private wiring scan directory")?;
    let scratch = Scratch(scratch);
    let script = scratch.0.join("wiring-scan.sh");
    std::fs::write(&script, source.stdout).context("write base wiring scan")?;
    let output = process::run(
        Command::new(process::executable("sh")?)
            .arg(&script)
            .args([base, head]),
        repo,
        deadline,
        64000,
    )?;
    Ok(Some(format!(
        "exit={} timed_out={} truncated={}\n{}",
        output.exit,
        output.timed_out,
        output.truncated,
        String::from_utf8_lossy(&output.stdout)
    )))
}
