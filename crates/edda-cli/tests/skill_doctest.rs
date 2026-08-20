//! Executable examples for the coordination skills shipped by `edda init`.
//!
//! `crates/edda-cli/src/skills/*.md` are compiled into the binary with
//! `include_str!` and written into every project. Nothing used to check that
//! the commands they teach are real: over one review loop this file set shipped
//! four separate false CLI claims — a verb declared not to exist while it did,
//! an invocation whose bare form silently released nothing, and twice a command
//! named as the way to read a value it does not print. Every one passed fmt,
//! clippy and the whole test suite.
//!
//! A skill can now carry its own proof. Mark a fenced block `bash edda-doctest`
//! and this harness runs it against the built binary in a throwaway project:
//!
//! ```text
//! ```bash edda-doctest
//! $ edda claim "auth" --paths "src/auth/*"
//! > session: cli-auth
//! $ edda unclaim
//! ! cannot tell which claim is yours
//! ```
//! ```
//!
//! `$` runs a command, `>` asserts it succeeded and printed the text, and `!`
//! asserts it failed with the text. Unmarked blocks are prose and are ignored,
//! so existing examples do not have to be converted at once.
//!
//! Ignoring unmarked blocks is also how coverage could disappear: demoting a
//! fence back to ```` ```bash ```` is a one-token edit that reads as cosmetic
//! and silently stops that block from running. The steps it leaves behind are
//! what give it away, so a `$ edda …` line inside an unmarked block is an
//! error rather than prose. A shell transcript that runs something else
//! (`$ git status`) is prose and passes.

use std::path::{Path, PathBuf};
use std::process::Command;

const EDDA: &str = env!("CARGO_BIN_EXE_edda");

/// One `$`/`>`/`!` line from a doctest block.
#[derive(Debug)]
enum Step {
    Run(Vec<String>),
    ExpectOk(String),
    ExpectErr(String),
}

fn skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/skills")
}

/// Split a command line on spaces, keeping double-quoted runs together.
///
/// Enough for the shapes skills actually use (`--paths "src/a/*"`); anything
/// needing more is a sign the example belongs in a test rather than a doc.
fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The info string that makes a fenced block executable.
const DOCTEST_FENCE: &str = "```bash edda-doctest";

/// One fenced block, kept with the marker that decides whether it runs.
struct Fence<'a> {
    is_doctest: bool,
    lines: Vec<&'a str>,
}

/// Walk the markdown once and return every fenced block.
///
/// Both the runner and the demotion guard read this, so they cannot drift
/// apart on what counts as a block — keeping two fence trackers in step was
/// itself a defect (GH-492).
///
/// Closing needs a fence at least as long as the opening one, per CommonMark,
/// so a ```` wrapper can quote a ``` block. That is what lets a skill *show*
/// the doctest format without the harness executing the illustration.
fn fenced_blocks(markdown: &str) -> Vec<Fence<'_>> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, Fence)> = None;

    for line in markdown.lines() {
        let trimmed = line.trim();
        let ticks = trimmed.chars().take_while(|c| *c == '`').count();

        match open.as_mut() {
            Some((width, fence)) => {
                if ticks >= *width && trimmed[ticks..].trim().is_empty() {
                    blocks.push(open.take().expect("open fence").1);
                } else {
                    fence.lines.push(trimmed);
                }
            }
            None if ticks >= 3 => {
                open = Some((
                    ticks,
                    Fence {
                        is_doctest: trimmed == DOCTEST_FENCE,
                        lines: Vec::new(),
                    },
                ));
            }
            None => {}
        }
    }
    // An unclosed fence still carries its steps; report them rather than drop
    // them, or a stray backtick would hide a whole block.
    if let Some((_, fence)) = open {
        blocks.push(fence);
    }
    blocks
}

/// Does this line invoke edda as a doctest step?
///
/// The guard and the runner agree on this, so a line that would have run is
/// exactly the line whose absence from a doctest fence is worth reporting.
/// Narrow to `edda` on purpose: a prose block written as a shell transcript
/// (`$ git status`) is not a lost doctest, and claiming it was would be a
/// false alarm in a message asserting a marker went missing (GH-492).
fn edda_step_args(trimmed: &str) -> Option<Vec<String>> {
    let rest = trimmed.strip_prefix("$ ")?;
    let args = split_args(rest);
    match args.first().map(String::as_str) {
        Some("edda") => Some(args[1..].to_vec()),
        _ => None,
    }
}

/// Pull every `bash edda-doctest` block out of one skill.
fn parse_blocks(markdown: &str) -> Vec<Vec<Step>> {
    let mut blocks = Vec::new();

    for fence in fenced_blocks(markdown).into_iter().filter(|f| f.is_doctest) {
        let mut steps = Vec::new();
        for trimmed in fence.lines {
            if let Some(args) = edda_step_args(trimmed) {
                steps.push(Step::Run(args));
            } else if let Some(rest) = trimmed.strip_prefix("> ") {
                steps.push(Step::ExpectOk(rest.to_string()));
            } else if let Some(rest) = trimmed.strip_prefix("! ") {
                steps.push(Step::ExpectErr(rest.to_string()));
            } else if trimmed.starts_with("$ ") {
                panic!("a doctest step must invoke edda: {trimmed}");
            } else if !trimmed.is_empty() {
                panic!("unrecognized doctest line (want `$ `, `> ` or `! `): {trimmed}");
            }
        }
        blocks.push(steps);
    }
    blocks
}

/// Find doctest steps stranded in a block that is no longer marked as one.
///
/// Unmarked fences are ignored by design, so a demoted fence would otherwise
/// drop its assertions with the suite still green.
fn find_demoted_steps(markdown: &str) -> Vec<String> {
    fenced_blocks(markdown)
        .into_iter()
        // A block that quotes the marker as content is illustrating the
        // format, not a doctest that lost its own. Its steps are meant to be
        // read, so reporting them would be the false alarm this guard was
        // narrowed to avoid.
        .filter(|f| !f.is_doctest && !f.lines.contains(&DOCTEST_FENCE))
        .flat_map(|f| f.lines)
        .filter(|line| edda_step_args(line).is_some())
        .map(str::to_string)
        .collect()
}

struct Outcome {
    ok: bool,
    text: String,
}

fn run(args: &[String], repo: &Path, store: &Path) -> Outcome {
    let out = Command::new(EDDA)
        .args(args)
        .current_dir(repo)
        .env("EDDA_STORE_ROOT", store)
        .env_remove("EDDA_SESSION_ID")
        .env_remove("EDDA_SESSION_LABEL")
        .output()
        .unwrap_or_else(|e| panic!("could not run edda {args:?}: {e}"));
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Outcome {
        ok: out.status.success(),
        text,
    }
}

/// A fresh project for one block, so blocks cannot leak state into each other.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    let store = dir.path().join("store");
    std::fs::create_dir_all(&repo).expect("repo dir");
    std::fs::create_dir_all(&store).expect("store dir");

    let git = Command::new("git")
        .args(["init", "-q", "."])
        .current_dir(&repo)
        .output()
        .expect("git init");
    assert!(git.status.success(), "git init failed");

    let init = run(
        &["init".to_string(), "--no-hooks".to_string()],
        &repo,
        &store,
    );
    assert!(init.ok, "edda init failed in the fixture:\n{}", init.text);

    (dir, repo, store)
}

#[test]
fn shipped_skill_examples_still_do_what_they_say() {
    let dir = skills_dir();
    let mut checked = 0usize;
    let mut files_with_blocks = Vec::new();

    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let markdown = std::fs::read_to_string(&path).expect("read skill");

        // Checked before the emptiness test: a file whose only doctest fence
        // was demoted parses to zero blocks, which is precisely the silent
        // loss this guard exists to make loud.
        let stranded = find_demoted_steps(&markdown);
        assert!(
            stranded.is_empty(),
            "{name}: doctest steps sit in a block that is not marked \
             ```bash edda-doctest, so they never run: {stranded:?}"
        );

        let blocks = parse_blocks(&markdown);
        if blocks.is_empty() {
            continue;
        }
        files_with_blocks.push(name.clone());

        for (b, block) in blocks.iter().enumerate() {
            let (_guard, repo, store) = fixture();
            let mut last: Option<Outcome> = None;

            for step in block {
                match step {
                    Step::Run(args) => last = Some(run(args, &repo, &store)),
                    Step::ExpectOk(want) => {
                        let got = last
                            .as_ref()
                            .unwrap_or_else(|| panic!("{name} block {b}: `> ` before any command"));
                        assert!(
                            got.ok,
                            "{name} block {b}: expected success containing {want:?}, but the \
                             command failed:\n{}",
                            got.text
                        );
                        assert!(
                            got.text.contains(want),
                            "{name} block {b}: output did not contain {want:?}:\n{}",
                            got.text
                        );
                        checked += 1;
                    }
                    Step::ExpectErr(want) => {
                        let got = last
                            .as_ref()
                            .unwrap_or_else(|| panic!("{name} block {b}: `! ` before any command"));
                        assert!(
                            !got.ok,
                            "{name} block {b}: expected failure containing {want:?}, but the \
                             command succeeded:\n{}",
                            got.text
                        );
                        // A misspelled or removed verb also exits non-zero, and
                        // would otherwise satisfy any `!` expectation. That is
                        // exactly the class this harness exists to catch.
                        assert!(
                            !got.text.contains("unrecognized subcommand"),
                            "{name} block {b}: the command does not exist:\n{}",
                            got.text
                        );
                        assert!(
                            got.text.contains(want),
                            "{name} block {b}: error did not contain {want:?}:\n{}",
                            got.text
                        );
                        checked += 1;
                    }
                }
            }
        }
    }

    assert!(
        checked > 0,
        "no skill doctest assertions ran; the harness is not reaching {}",
        dir.display()
    );
    println!("skill doctests: {checked} assertions across {files_with_blocks:?}");
}

#[test]
fn a_demoted_fence_is_an_error_rather_than_silence() {
    let demoted = "```bash\n$ edda claim \"auth\"\n> session: cli-auth\n```\n";
    assert_eq!(
        find_demoted_steps(demoted).len(),
        1,
        "a doctest block whose marker was removed must be reported"
    );
    assert!(
        parse_blocks(demoted).is_empty(),
        "and it must indeed have stopped running, which is why reporting matters"
    );

    let marked = "```bash edda-doctest\n$ edda claim \"auth\"\n> session: cli-auth\n```\n";
    assert!(
        find_demoted_steps(marked).is_empty(),
        "a properly marked block is not stranded"
    );

    let prose = "```bash\nedda claim \"auth\" --paths \"src/a/*\"\n```\n";
    assert!(
        find_demoted_steps(prose).is_empty(),
        "an ordinary example without doctest steps stays prose"
    );
}

#[test]
fn a_shell_transcript_is_prose_not_a_lost_doctest() {
    // The guard used to fire on any `$ ` line, so a prose block written as a
    // transcript would be reported as a doctest whose marker went missing --
    // a false alarm asserting something that never happened (GH-492).
    let transcript = "```bash\n$ git status\n$ gh pr view 491\n```\n";
    assert!(
        find_demoted_steps(transcript).is_empty(),
        "a transcript of other commands is prose"
    );

    let mixed = "```bash\n$ git status\n$ edda claim \"auth\"\n```\n";
    assert_eq!(
        find_demoted_steps(mixed),
        vec!["$ edda claim \"auth\"".to_string()],
        "only the edda step is a stranded doctest step"
    );
}

#[test]
fn an_illustrated_doctest_inside_a_longer_fence_does_not_run() {
    // A skill may want to show the format. CommonMark closes a fence only with
    // one at least as long, so a ```` wrapper quotes the ``` block inside it;
    // the harness must read it the same way or it would execute an example
    // that was never meant to run (GH-492).
    let illustrated = "````markdown\n```bash edda-doctest\n$ edda claim \"auth\"\n> session: \
                       cli-auth\n```\n````\n";
    assert!(
        parse_blocks(illustrated).is_empty(),
        "the illustration is quoted, not executed"
    );
    assert!(
        find_demoted_steps(illustrated).is_empty(),
        "and quoting it is not a demotion either"
    );
}

#[test]
fn an_unclosed_fence_still_reports_its_steps() {
    // A stray backtick must not be able to hide a whole block from the guard.
    let unclosed = "```bash\n$ edda claim \"auth\"\n";
    assert_eq!(
        find_demoted_steps(unclosed).len(),
        1,
        "an unterminated block still carries steps worth reporting"
    );
}

#[test]
fn a_doctest_block_is_parsed_into_its_steps() {
    let blocks = parse_blocks(
        "prose\n\n```bash edda-doctest\n$ edda claim \"auth\" --paths \"src/a/*\"\n> session: \
         cli-auth\n! nope\n```\n\n```bash\n$ edda not-a-doctest\n```\n",
    );
    assert_eq!(blocks.len(), 1, "only the marked block is a doctest");
    assert_eq!(blocks[0].len(), 3);
    match &blocks[0][0] {
        Step::Run(args) => assert_eq!(args, &["claim", "auth", "--paths", "src/a/*"]),
        other => panic!("expected a command, got {other:?}"),
    }
}
