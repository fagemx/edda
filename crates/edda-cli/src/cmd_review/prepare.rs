use super::{args::ReviewArgs, brief, evidence, git, github, identity, qualification, subject};
use anyhow::{bail, Result};
use edda_core::{ReviewRefs, ReviewSpec, ReviewVerdictPayload};
use edda_ledger::Ledger;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};

const CONTEXT_MAX_BYTES: usize = 32 * 1024;
const CONTEXT_READ_LIMIT: u64 = (CONTEXT_MAX_BYTES + 1) as u64;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;

#[derive(Debug, Clone)]
pub(crate) struct SupportingContext {
    pub digest: String,
    pub text: String,
}

#[derive(Debug)]
struct ContextOmission {
    reason: &'static str,
    path: PathBuf,
}

impl ContextOmission {
    fn note(&self) -> String {
        format!(
            "Review context omitted: reason={}; source=explicit --context-file {}",
            self.reason,
            display_path(&self.path)
        )
    }

    fn warning(&self) -> String {
        format!(
            "optional review context omitted: reason={}; path={}",
            self.reason,
            display_path(&self.path)
        )
    }
}

fn display_path(path: &Path) -> String {
    format!("{:?}", path.to_string_lossy())
}

fn omission(reason: &'static str, path: PathBuf) -> ContextOmission {
    ContextOmission { reason, path }
}

fn metadata_error_reason(path: &Path, error: &std::io::Error) -> &'static str {
    if error.kind() != std::io::ErrorKind::NotFound {
        return "unreadable";
    }
    let mut ancestor = path.parent();
    while let Some(parent) = ancestor {
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) => {
                return if metadata.file_type().is_dir() {
                    "missing"
                } else {
                    "unreadable"
                };
            }
            Err(parent_error) if parent_error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = parent.parent();
            }
            Err(_) => return "unreadable",
        }
    }
    "missing"
}

fn open_error_reason(error: &std::io::Error) -> &'static str {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return "symlink";
    }
    if error.kind() == std::io::ErrorKind::NotFound {
        "missing"
    } else {
        "unreadable"
    }
}

#[cfg(unix)]
fn open_context_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(windows)]
fn open_context_file(path: &Path) -> std::io::Result<File> {
    // OPEN_REPARSE_POINT opens the final component itself instead of following
    // it. The metadata checks below then reject every reparse point, not only
    // the symlink tags that std recognizes.
    OpenOptions::new()
        .read(true)
        // Deny write/delete sharing while the selected handle is open, so
        // pathname replacement cannot occur after the identity recheck.
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(unix)]
type ContextFileIdentity = (u64, u64);
#[cfg(windows)]
type ContextFileIdentity = same_file::Handle;

#[cfg(unix)]
fn path_file_identity(_: &Path, metadata: &Metadata) -> std::io::Result<ContextFileIdentity> {
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn path_file_identity(path: &Path, _: &Metadata) -> std::io::Result<ContextFileIdentity> {
    // same-file 1.0.6 safely asks Windows for volume serial + file index and
    // keeps its path handle open for the lifetime of the identity comparison.
    same_file::Handle::from_path(path)
}

#[cfg(unix)]
fn opened_file_identity(_: &File, metadata: &Metadata) -> std::io::Result<ContextFileIdentity> {
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn opened_file_identity(file: &File, _: &Metadata) -> std::io::Result<ContextFileIdentity> {
    same_file::Handle::from_file(file.try_clone()?)
}

#[cfg(unix)]
fn is_reparse_point(_: &Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn validate_regular_metadata(
    metadata: &Metadata,
    resolved: &Path,
) -> std::result::Result<(), ContextOmission> {
    if metadata.file_type().is_symlink() || is_reparse_point(metadata) {
        return Err(omission("symlink", resolved.to_path_buf()));
    }
    if !metadata.file_type().is_file() {
        return Err(omission("non-regular", resolved.to_path_buf()));
    }
    Ok(())
}

fn read_context_after_lstat(
    path: &Path,
    cwd: &Path,
    after_lstat: impl FnOnce(&Path),
) -> std::result::Result<SupportingContext, ContextOmission> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let before = std::fs::symlink_metadata(&resolved)
        .map_err(|error| omission(metadata_error_reason(&resolved, &error), resolved.clone()))?;
    validate_regular_metadata(&before, &resolved)?;
    let before_identity = path_file_identity(&resolved, &before)
        .map_err(|_| omission("unreadable", resolved.clone()))?;

    after_lstat(&resolved);

    let mut file = open_context_file(&resolved)
        .map_err(|error| omission(open_error_reason(&error), resolved.clone()))?;
    let opened = file
        .metadata()
        .map_err(|_| omission("unreadable", resolved.clone()))?;
    validate_regular_metadata(&opened, &resolved)?;
    let opened_identity = opened_file_identity(&file, &opened)
        .map_err(|_| omission("unreadable", resolved.clone()))?;

    // Re-lstat and identify the path while the safe handle denies write/delete
    // sharing. Comparing genuine identities catches regular-file replacement
    // between either pathname check and open; no bytes are read unless the
    // selected path still names the exact regular file inspected before open.
    let after = std::fs::symlink_metadata(&resolved)
        .map_err(|error| omission(metadata_error_reason(&resolved, &error), resolved.clone()))?;
    validate_regular_metadata(&after, &resolved)?;
    let after_identity = path_file_identity(&resolved, &after)
        .map_err(|_| omission("unreadable", resolved.clone()))?;
    if before_identity != opened_identity || after_identity != opened_identity {
        return Err(omission("changed", resolved));
    }

    let mut bytes = Vec::new();
    (&mut file)
        .take(CONTEXT_READ_LIMIT)
        .read_to_end(&mut bytes)
        .map_err(|_| omission("unreadable", resolved.clone()))?;
    if bytes.len() > CONTEXT_MAX_BYTES {
        return Err(omission("too-large", resolved));
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    let text = String::from_utf8(bytes).map_err(|_| omission("invalid-utf8", resolved))?;
    Ok(SupportingContext { digest, text })
}

fn read_context(
    path: &Path,
    cwd: &Path,
) -> std::result::Result<SupportingContext, ContextOmission> {
    read_context_after_lstat(path, cwd, |_| {})
}

pub(crate) struct Prepared {
    pub repo: PathBuf,
    pub ledger: Ledger,
    pub subject: subject::Subject,
    pub refs: ReviewRefs,
    pub spec: ReviewSpec,
    pub spec_text: String,
    pub review_md: String,
    pub has_review_md: bool,
    pub fm: brief::FrontMatter,
    pub authors: identity::Authors,
    pub session: String,
    pub notes: Vec<String>,
    pub context: Option<SupportingContext>,
    pub context_warning: Option<String>,
    pub prior: Option<ReviewVerdictPayload>,
}

pub(crate) fn prepare(args: &ReviewArgs, cwd: &Path) -> Result<Prepared> {
    let repo = git::repo_root_from(cwd)?;
    let pr = args.pr.map(|n| github::resolve_pr(cwd, n)).transpose()?;
    let subject = subject::resolve_subject(
        cwd,
        args.base
            .as_deref()
            .or_else(|| pr.as_ref().map(|p| p.base.as_str())),
        pr.as_ref().map(|p| p.head.as_str()).unwrap_or(&args.head),
    )?;
    let ledger = Ledger::open(&repo)?;
    let (mut refs, prior) = subject::history(&ledger, &repo, &subject, args.pr)?;
    let (spec, spec_text, issue) = github::load_spec(
        cwd,
        cwd,
        args.spec.as_deref(),
        pr.as_ref().and_then(|p| p.issue),
        args.trust_spec,
    )?;
    refs.issue = issue;
    let range = format!("{}..{}", subject.base_sha, subject.head_sha);
    let commits = git::git(cwd, &["rev-list", &range])?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let subjects = git::git(cwd, &["log", "--format=%s", &range])?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let trailers = git::git(cwd, &["log", "--format=%(trailers)", &range])?;
    let authors = identity::authors(&ledger, &commits, &subjects, &trailers)?;
    let session = if args.resume {
        let previous = prior.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--resume requires an existing review for this range or PR")
        })?;
        if previous.reviewer.agent != args.agent.as_str() {
            bail!("resume requires the same reviewer agent");
        }
        if args
            .session_id
            .as_ref()
            .is_some_and(|s| !identity::same_session(s, &previous.reviewer.session_id))
        {
            bail!("resume session differs from recorded reviewer session");
        }
        previous.reviewer.session_id.clone()
    } else {
        args.session_id
            .clone()
            .unwrap_or_else(crate::cmd_dispatch::generate_session_id)
    };
    // Backends expect a UUID, not an arbitrary human label.
    if session.len() != 36
        || session.char_indices().any(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c != '-'
            } else {
                !c.is_ascii_hexdigit()
            }
        })
    {
        bail!("reviewer --session-id must be a UUID");
    }
    identity::independence(&authors, &session, None)?;
    let review_blob = format!("{}:REVIEW.md", subject.base_sha);
    let has_review_md = git::git_ok(cwd, &["cat-file", "-e", &review_blob])?;
    let review_md = if has_review_md {
        git::git(cwd, &["show", &review_blob])?
    } else {
        String::new()
    };
    let (mut fm, _, note) = brief::parse_review_md(&review_md);
    if fm.classes.is_empty() {
        fm.classes = brief::default_classes();
    }
    if fm.ran_allowlist.is_empty() {
        fm.ran_allowlist.push("edda".into());
    }
    let mut notes = note.into_iter().collect::<Vec<_>>();
    notes.push("Structured conductor cost receipts currently lack author session/model/SHA; author identity uses linked session digests and git trailers.".into());
    let (context, context_warning) = match args.context_file.as_deref() {
        None => (None, None),
        Some(path) => {
            let source = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            match read_context(path, cwd) {
                Ok(context) => {
                    notes.push(format!(
                        "Review context: source=explicit --context-file {}; sha256={}; trust=untrusted supporting context",
                        display_path(&source),
                        context.digest
                    ));
                    (Some(context), None)
                }
                Err(omitted) => {
                    notes.push(omitted.note());
                    (None, Some(omitted.warning()))
                }
            }
        }
    };
    Ok(Prepared {
        repo,
        ledger,
        subject,
        refs,
        spec,
        spec_text,
        review_md,
        has_review_md,
        fm,
        authors,
        session,
        notes,
        context,
        context_warning,
        prior,
    })
}

pub(crate) fn collect_evidence(
    prepared: &mut Prepared,
    args: &ReviewArgs,
    worktree: &Path,
) -> Result<(edda_core::ReviewGates, Vec<edda_core::ReviewProbe>, String)> {
    let verify = if matches!(prepared.spec.trust.as_str(), "operator" | "maintainer") {
        evidence::extract_verify(&prepared.spec_text)
    } else {
        vec![]
    };
    let set = evidence::gate_set(&prepared.fm, &args.gates, &verify);
    let (_, mut read, mut uncovered) = evidence::read_gates(
        &prepared.ledger,
        &prepared.subject.head_sha,
        &set,
        &prepared.subject.files,
    )?;
    if let Some(pr) = args.pr {
        let checks = evidence::gh_required_checks(&prepared.repo, pr, &prepared.subject.head_sha)?;
        let (_, required_rows) = evidence::read_ci(&checks);
        read.extend(required_rows);
        let (_, mapped_rows, mapped_gates) =
            evidence::read_ci_job_map(&checks, &set, &prepared.fm.ci_gates);
        read.extend(mapped_rows);
        evidence::remove_mapped_uncovered(&mut uncovered, &mapped_gates);
    }
    let (ran, notes) = if args.run_gates {
        evidence::ran_gates(
            worktree,
            &set.cmds,
            args.max_ran_sec,
            std::env::var_os("CARGO_TARGET_DIR").is_some_and(|v| !v.is_empty()),
            &prepared.ledger.paths,
            &prepared.repo,
        )
    } else {
        (vec![], vec![])
    };
    prepared.notes.extend(notes);
    let status = evidence::gate_status(&set.cmds, &read, &ran);
    let diff = git::git(
        &prepared.repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            &format!(
                "{}..{}",
                prepared.subject.base_sha, prepared.subject.head_sha
            ),
            "--",
        ],
    )?;
    let verbs =
        evidence::extract_probe_verbs(&diff, Some(&prepared.spec_text), &prepared.fm.ran_allowlist);
    let probes = evidence::run_probes(worktree, &verbs);
    let wiring = evidence::run_wiring_scan(
        &prepared.repo,
        &prepared.subject.base_sha,
        &prepared.subject.head_sha,
    )?;
    let measures = evidence::checklist_measures(&ran, &probes);
    let text = evidence::evidence_text(&read, &uncovered, &ran, &probes, wiring.as_deref());
    Ok((
        edda_core::ReviewGates {
            status,
            declared_by: set.declared_by,
            read,
            ran,
        },
        probes,
        format!(
            "{text}\n### Checklist `ran` measure IDs (exact strings only)\n{}",
            measures.join("\n")
        ),
    ))
}

pub(crate) fn assemble(
    prepared: &Prepared,
    args: &ReviewArgs,
    evidence: &str,
) -> Result<(brief::Brief, Vec<String>, qualification::Qualification)> {
    let classes = brief::route_classes(&prepared.subject.files, &prepared.fm.classes);
    // REVIEW.md §6.1 reads brief silence as "checklist-type engine", so the
    // brief must name the engine's R22 authority for this PR's surface.
    let qualified = qualification::assess(
        &prepared.subject.files,
        &super::model_requested(args),
        super::transport(args.agent),
        args.require_model_diversity,
    )?;
    let paths = prepared
        .subject
        .files
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let decisions = prepared.ledger.query_by_paths(&paths, None, Some(100))?;
    let ratified = prepared.ledger.ratified_decision_events()?;
    let pack = serde_json::to_string(
        &serde_json::json!({"decisions":decisions,"ratified_event_ids":ratified}),
    )?;
    let range = format!(
        "{}..{}",
        prepared.subject.base_sha, prepared.subject.head_sha
    );
    let chunks = prepared
        .subject
        .files
        .iter()
        .map(|path| {
            Ok((
                path.clone(),
                git::git(
                    &prepared.repo,
                    &[
                        "--literal-pathspecs",
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-renames",
                        &range,
                        "--",
                        path,
                    ],
                )?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let budget = std::env::var("EDDA_REVIEW_DIFF_BUDGET_CHARS")
        .ok()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(200_000);
    let qualification_text = qualified.brief_section();
    let inputs = brief::BriefInputs {
        review_md: &prepared.review_md,
        classes: &classes,
        qualification: &qualification_text,
        spec: &prepared.spec_text,
        spec_trust: &prepared.spec.trust,
        ledger_pack: &pack,
        evidence,
        head_sha: &prepared.subject.head_sha,
        context: prepared
            .context
            .as_ref()
            .map(|context| brief::SupportingContext {
                digest: &context.digest,
                text: &context.text,
            }),
    };
    Ok((
        brief::assemble(&inputs, chunks, &prepared.fm.classes, budget)?,
        classes,
        qualified,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_review::git::testrepo;
    use edda_core::event::{new_cmd_event_with_git_context_and_dirty_paths, CmdEventParams};

    fn omitted(path: &Path, cwd: &Path) -> &'static str {
        read_context(path, cwd).expect_err("context omitted").reason
    }

    fn omitted_after_lstat(path: &Path, cwd: &Path, hook: impl FnOnce(&Path)) -> &'static str {
        read_context_after_lstat(path, cwd, hook)
            .expect_err("context omitted")
            .reason
    }

    #[test]
    fn bounded_context_preserves_exact_utf8_bytes_and_accepts_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cwd = temp.path().join("invocation");
        std::fs::create_dir(&cwd).expect("invocation cwd");
        let relative = Path::new("facts.md");

        std::fs::write(cwd.join(relative), []).expect("empty context");
        let empty = read_context(relative, &cwd).expect("empty context accepted");
        assert!(empty.text.is_empty());
        assert_eq!(
            empty.digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let exact = format!("{}é", "界".repeat(10_922));
        assert_eq!(exact.len(), CONTEXT_MAX_BYTES);
        std::fs::write(cwd.join(relative), exact.as_bytes()).expect("exact context");
        let loaded = read_context(relative, &cwd).expect("32 KiB accepted");
        assert_eq!(loaded.text.as_bytes(), exact.as_bytes());
        assert_eq!(loaded.digest, hex::encode(Sha256::digest(exact.as_bytes())));

        let preserved = "\u{feff}  first\r\nsecond\n\t";
        let absolute = cwd.join("absolute.md");
        std::fs::write(&absolute, preserved.as_bytes()).expect("preserved context");
        assert_eq!(
            read_context(&absolute, temp.path())
                .expect("absolute context")
                .text,
            preserved
        );
    }

    #[test]
    fn invalid_context_is_wholly_omitted_with_stable_reasons() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        assert_eq!(omitted(Path::new("missing"), root), "missing");

        let oversized = root.join("oversized");
        std::fs::write(&oversized, vec![b'x'; CONTEXT_MAX_BYTES + 1]).expect("oversized");
        assert_eq!(omitted(&oversized, root), "too-large");

        let invalid = root.join("invalid");
        std::fs::write(&invalid, [0xff]).expect("invalid utf8");
        assert_eq!(omitted(&invalid, root), "invalid-utf8");

        let directory = root.join("directory");
        std::fs::create_dir(&directory).expect("directory");
        assert_eq!(omitted(&directory, root), "non-regular");

        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("blocker");
        assert_eq!(omitted(&blocker.join("child"), root), "unreadable");
    }

    #[test]
    fn regular_file_replaced_after_lstat_is_rejected_as_changed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let selected = temp.path().join("selected");
        let original = temp.path().join("original");
        std::fs::write(&selected, b"approved facts").expect("selected");
        assert_eq!(
            omitted_after_lstat(&selected, temp.path(), |path| {
                std::fs::rename(path, &original).expect("retain original identity");
                std::fs::write(path, b"replacement facts").expect("replacement");
            }),
            "changed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_nonregular_context_are_rejected_without_following_or_blocking() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::write(&target, b"facts").expect("target");
        symlink(&target, &link).expect("symlink");
        assert_eq!(omitted(&link, temp.path()), "symlink");

        let socket = temp.path().join("socket");
        let _listener = UnixListener::bind(&socket).expect("unix socket");
        assert_eq!(omitted(&socket, temp.path()), "non-regular");
        assert_eq!(omitted(Path::new("/dev/null"), temp.path()), "non-regular");

        let selected = temp.path().join("selected");
        let original = temp.path().join("original");
        let secret = temp.path().join("secret");
        std::fs::write(&selected, b"approved facts").expect("selected");
        std::fs::write(&secret, [0xff]).expect("secret");
        assert_eq!(
            omitted_after_lstat(&selected, temp.path(), |path| {
                std::fs::rename(path, &original).expect("retain original");
                symlink(&secret, path).expect("replacement symlink");
            }),
            "symlink",
            "the replacement target must be rejected before its secret bytes are read"
        );

        let fifo = temp.path().join("fifo");
        let fifo_original = temp.path().join("fifo-original");
        std::fs::write(&fifo, b"approved facts").expect("fifo placeholder");
        assert_eq!(
            omitted_after_lstat(&fifo, temp.path(), |path| {
                std::fs::rename(path, &fifo_original).expect("retain fifo placeholder");
                let status = std::process::Command::new("mkfifo")
                    .arg(path)
                    .status()
                    .expect("run mkfifo");
                assert!(status.success(), "mkfifo failed: {status}");
            }),
            "non-regular",
            "O_NONBLOCK must make a FIFO replacement return instead of waiting for a writer"
        );
    }

    #[cfg(windows)]
    #[test]
    fn distinct_files_with_identical_legacy_metadata_are_rejected_as_changed() {
        use std::os::windows::fs::{FileTimesExt as _, MetadataExt as _};
        use std::time::{Duration, UNIX_EPOCH};

        fn set_identical_metadata(path: &Path) {
            std::fs::write(path, b"same-size-facts").expect("write context");
            let fixed = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
            let times = std::fs::FileTimes::new()
                .set_accessed(fixed)
                .set_modified(fixed)
                .set_created(fixed);
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .expect("open for timestamps")
                .set_times(times)
                .expect("set identical timestamps");
        }

        fn legacy_metadata_tuple(metadata: &Metadata) -> (u32, u64, u64, u64, u64) {
            (
                metadata.file_attributes(),
                metadata.creation_time(),
                metadata.last_access_time(),
                metadata.last_write_time(),
                metadata.file_size(),
            )
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let selected = temp.path().join("selected");
        let replacement = temp.path().join("replacement");
        let retained = temp.path().join("retained");
        set_identical_metadata(&selected);
        set_identical_metadata(&replacement);
        assert_eq!(
            legacy_metadata_tuple(&std::fs::metadata(&selected).expect("selected metadata")),
            legacy_metadata_tuple(&std::fs::metadata(&replacement).expect("replacement metadata")),
            "the adversarial files must defeat the retired metadata tuple"
        );
        assert_ne!(
            same_file::Handle::from_path(&selected).expect("selected identity"),
            same_file::Handle::from_path(&replacement).expect("replacement identity"),
            "distinct files must have distinct genuine Windows identities"
        );

        assert_eq!(
            omitted_after_lstat(&selected, temp.path(), |path| {
                std::fs::rename(path, &retained).expect("retain original identity");
                std::fs::rename(&replacement, path).expect("install metadata twin");
            }),
            "changed"
        );
    }

    #[cfg(windows)]
    #[test]
    fn final_component_replacement_symlink_is_rejected_when_privilege_is_available() {
        use std::os::windows::fs::symlink_file;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::write(&target, b"facts").expect("target");
        if let Err(error) = symlink_file(&target, &link) {
            assert!(
                error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(1314),
                "unexpected symlink failure: {error}"
            );
            return;
        }
        assert_eq!(omitted(&link, temp.path()), "symlink");

        let selected = temp.path().join("selected");
        let original = temp.path().join("original");
        let secret = temp.path().join("secret");
        std::fs::write(&selected, b"approved facts").expect("selected");
        std::fs::write(&secret, [0xff]).expect("secret");
        assert_eq!(
            omitted_after_lstat(&selected, temp.path(), |path| {
                std::fs::rename(path, &original).expect("retain original");
                symlink_file(&secret, path).expect("replacement symlink");
            }),
            "symlink",
            "OPEN_REPARSE_POINT must reject the link before reading secret bytes"
        );
    }

    #[test]
    fn evidence_selection_receives_the_reviewed_subject_files() {
        let (_temp, root) = testrepo::init();
        testrepo::run(&root, &["checkout", "-qb", "feature"]);
        std::fs::create_dir_all(root.join("docs/archive")).expect("archive dir");
        let subject_path = "docs/archive/probe.txt";
        let head = testrepo::commit_file(&root, subject_path, "subject\n", "feature change");
        let ledger = Ledger::open_or_init(&root).expect("ledger");
        let argv = vec!["gate".to_owned()];
        let untracked = vec![subject_path.to_owned()];
        let event = new_cmd_event_with_git_context_and_dirty_paths(
            &CmdEventParams {
                branch: "main",
                parent_hash: ledger.last_event_hash().expect("parent").as_deref(),
                argv: &argv,
                cwd: root.to_str().expect("utf8 root"),
                exit_code: 0,
                duration_ms: 1,
                stdout_blob: "",
                stderr_blob: "",
            },
            Some(&head),
            Some(true),
            Some((&[], &untracked)),
        )
        .expect("receipt");
        ledger.append_event(&event).expect("append receipt");
        let args = ReviewArgs {
            gates: vec!["gate".into()],
            ..Default::default()
        };
        let mut prepared = prepare(&args, &root).expect("prepare");
        let (gates, _, _) = collect_evidence(&mut prepared, &args, &root).expect("evidence");
        assert_eq!(gates.status, "unverified");
        assert!(gates.read.is_empty());
    }
}
