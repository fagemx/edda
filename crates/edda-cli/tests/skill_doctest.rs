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
//! and silently stops that block from running. What gives it away is the
//! assertions left behind — a block carrying `>` or `!` lines was checking
//! something, so `$ edda …` steps in an unmarked fence are an error rather than
//! prose. A transcript with nothing asserted is prose and passes, whichever
//! command it runs.
//!
//! The scan errs toward reading a fence-looking line as a fence, and toward
//! reporting. A false alarm is cheap; a skill that quietly stopped being
//! checked is the failure this file exists to prevent.

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

/// Every markdown file under `dir`, at any depth.
///
/// The scan used to be one directory deep, so a skill added in a subdirectory
/// would simply never be checked (GH-492).
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            found.extend(markdown_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            found.push(path);
        }
    }
    found.sort();
    found
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

/// The fence character and run length that opened a block, if this line opens
/// or closes one.
///
/// CommonMark allows both backticks and tildes, and closes a block only with
/// the same character at least as long as the opener. Recognising tildes keeps
/// a `~~~markdown` wrapper from leaking its quoted contents into the scan
/// (GH-494).
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == ch).count();
    (run >= 3).then_some((ch, run))
}

/// Walk the markdown once and return every fenced block.
///
/// Both the runner and the demotion guard read this, so they cannot drift
/// apart on what counts as a block — keeping two fence trackers in step was
/// itself a defect (GH-492).
///
/// Indentation is deliberately ignored. CommonMark treats a four-space line as
/// literal *relative to its container*, and it strips a list marker's width
/// first, so a fence indented under a `- ` bullet is a real fence — GitHub
/// renders it as code. Judging indentation against the document instead turned
/// such a block invisible: it stopped running, and a demoted one stopped being
/// reported, which is the silence this harness exists to prevent (GH-495).
///
/// Reading every fence-looking line as a fence errs toward running and
/// reporting. That is the safe direction here: the cost is a false alarm on an
/// indented literal example, against a silently unchecked skill.
fn fenced_blocks(markdown: &str) -> Vec<Fence<'_>> {
    let mut blocks = Vec::new();
    let mut open: Option<(char, usize, Fence)> = None;

    for line in markdown.lines() {
        let trimmed = line.trim();
        let marker = fence_marker(trimmed);

        match open.as_mut() {
            Some((ch, width, fence)) => {
                let closes = marker
                    .filter(|(c, run)| c == ch && run >= width)
                    .is_some_and(|(_, run)| trimmed[run..].trim().is_empty());
                if closes {
                    blocks.push(open.take().expect("open fence").2);
                } else {
                    fence.lines.push(trimmed);
                }
            }
            None => {
                if let Some((ch, run)) = marker {
                    open = Some((
                        ch,
                        run,
                        Fence {
                            is_doctest: trimmed == DOCTEST_FENCE,
                            lines: Vec::new(),
                        },
                    ));
                }
            }
        }
    }
    // An unclosed fence still carries its steps; report them rather than drop
    // them, or a stray backtick would hide a whole block.
    if let Some((_, _, fence)) = open {
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

/// Steps of an unmarked block, with any quoted illustration removed.
///
/// The exemption used to cover a whole fence, so one unbalanced fence could
/// merge an illustration with a later genuinely demoted block and excuse it.
/// Skipping only the span from a quoted marker to its matching closer keeps
/// the rest of the block under the guard (GH-494).
fn lines_outside_quoted_illustrations<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut quoting = false;
    for line in lines {
        if *line == DOCTEST_FENCE {
            quoting = true;
            continue;
        }
        if quoting {
            if fence_marker(line).is_some() {
                quoting = false;
            }
            continue;
        }
        out.push(*line);
    }
    out
}

/// Find doctest steps stranded in a block that is no longer marked as one.
///
/// Unmarked fences are ignored by design, so a demoted fence would otherwise
/// drop its assertions with the suite still green.
///
/// A block is only treated as demoted when it still carries assertions. A
/// transcript of `$ edda …` commands with no `>` or `!` lines is prose showing
/// usage — the style four docs under `docs/decision/` already use — and calling
/// it a lost marker would be a false alarm (GH-494).
fn find_demoted_steps(markdown: &str) -> Vec<String> {
    let mut stranded = Vec::new();

    for fence in fenced_blocks(markdown)
        .into_iter()
        .filter(|f| !f.is_doctest)
    {
        let lines = lines_outside_quoted_illustrations(&fence.lines);
        let has_assertions = lines
            .iter()
            .any(|l| l.starts_with("> ") || l.starts_with("! "));
        if !has_assertions {
            continue;
        }
        stranded.extend(
            lines
                .iter()
                .filter(|l| edda_step_args(l).is_some())
                .map(|l| (*l).to_string()),
        );
    }
    stranded
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

    for path in markdown_files(&dir) {
        let name = path
            .strip_prefix(&dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
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
fn a_transcript_without_assertions_is_prose() {
    // Was `a_shell_transcript_is_prose_not_a_lost_doctest`, which exempted a
    // block only when its commands were not edda. That missed the style four
    // docs under docs/decision/ actually use -- `$ edda …` transcripts showing
    // usage. What separates prose from a demoted doctest is not the command but
    // the absence of assertions, so the rule moved and this test moved with it
    // (GH-494).
    let other_commands = "```bash\n$ git status\n$ gh pr view 491\n```\n";
    assert!(
        find_demoted_steps(other_commands).is_empty(),
        "a transcript of other commands is prose"
    );

    let edda_transcript = "```bash\n$ edda claim \"auth\"\n$ edda peers --json\n```\n";
    assert!(
        find_demoted_steps(edda_transcript).is_empty(),
        "an edda transcript with nothing asserted is prose too"
    );

    let demoted = "```bash\n$ git status\n$ edda claim \"auth\"\n> session: cli-auth\n```\n";
    assert_eq!(
        find_demoted_steps(demoted),
        vec!["$ edda claim \"auth\"".to_string()],
        "assertions make it a doctest that lost its marker, and only the edda \
         step is reported"
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
    // A stray backtick must not be able to hide a block from the guard.
    let unclosed = "```bash\n$ edda claim \"auth\"\n> session: cli-auth\n";
    assert_eq!(
        find_demoted_steps(unclosed).len(),
        1,
        "an unterminated block still carries steps worth reporting"
    );
}

#[test]
fn an_unbalanced_illustration_cannot_launder_a_later_demotion() {
    // The exemption used to cover a whole fence, so an unbalanced illustration
    // could swallow a genuinely demoted block and excuse it. Skipping only the
    // quoted span keeps the rest under the guard (GH-494).
    // The outer fence has to be longer than the quoted one, or the inner
    // backticks close it and the two blocks never share a fence to begin with.
    let laundered = "````markdown\n```bash edda-doctest\n$ edda peers\n> ok\n```\n\
                     $ edda claim \"auth\"\n> session: cli-auth\n````\n";
    let stranded = find_demoted_steps(laundered);
    assert!(
        stranded.iter().any(|l| l.contains("claim")),
        "the demoted step outside the quoted span must still be reported: {stranded:?}"
    );
    assert!(
        !stranded.iter().any(|l| l.contains("peers")),
        "the quoted illustration must stay exempt: {stranded:?}"
    );
}

#[test]
fn a_tilde_wrapper_quotes_rather_than_leaks() {
    // CommonMark allows tilde fences; before GH-494 only backticks were
    // recognised, so a ~~~ wrapper left its quoted doctest visible to the
    // scanner and the illustration ran for real.
    let wrapped = "~~~markdown\n```bash edda-doctest\n$ edda claim \"auth\"\n> session: \
                   cli-auth\n```\n~~~\n";
    assert!(
        parse_blocks(wrapped).is_empty(),
        "a tilde wrapper quotes its contents"
    );
}

#[test]
fn an_indented_fence_is_still_a_fence() {
    // Was `an_indented_fence_is_literal_text`, which judged the four-space rule
    // against the document. CommonMark applies it relative to the container and
    // strips a list marker first, so a fence under a `- ` bullet is real and
    // GitHub renders it as code. Treating it as literal made such a block stop
    // running, and a demoted one stop being reported (GH-495).
    let indented = "    ```bash edda-doctest\n    $ edda claim \"auth\"\n    > session: \
                    cli-auth\n    ```\n";
    assert_eq!(
        parse_blocks(indented).len(),
        1,
        "an indented doctest still runs"
    );

    let demoted = "- step one:\n\n    ```bash\n    $ edda claim \"auth\"\n    > session: \
                   cli-auth\n    ```\n";
    assert_eq!(
        find_demoted_steps(demoted).len(),
        1,
        "and an indented demotion is still reported rather than silently dropped"
    );
}

#[test]
fn skills_in_subdirectories_are_scanned() {
    // The scan was one directory deep, so a skill in a subdirectory was never
    // checked (GH-492 item 2). Nothing in the shipped tree exercises this yet,
    // so the walk needs its own fixture.
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("group").join("deeper");
    std::fs::create_dir_all(&nested).expect("nested dirs");
    std::fs::write(dir.path().join("top.md"), "# top\n").expect("top");
    std::fs::write(nested.join("buried.md"), "# buried\n").expect("buried");
    std::fs::write(nested.join("ignored.txt"), "not markdown\n").expect("txt");

    let found: Vec<String> = markdown_files(dir.path())
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        found,
        vec!["buried.md", "top.md"],
        "sorted, recursive, .md only"
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
