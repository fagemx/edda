use super::evidence::{spec_trust, SpecOrigin};
use super::git::{commit, git};
use anyhow::{bail, Context, Result};
use edda_core::ReviewSpec;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

/// A validated GitHub repository selector. Controlled calls construct this
/// from the same canonical remote identity whose digest the manifest binds;
/// ordinary review calls retain the explicit `EDDA_REPO=owner/repo` door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubRepository {
    owner: String,
    name: String,
}

impl GitHubRepository {
    pub(crate) fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub(crate) fn for_control_compile(checkout: &Path, expected_portable: &str) -> Result<Self> {
        let paths = edda_ledger::EddaPaths::discover(checkout);
        let identity = edda_store::continuity::derive_portable_repository_identity(
            checkout,
            &paths.config_json,
        )?;
        anyhow::ensure!(
            identity.portable_repo_id.as_deref() == Some(expected_portable),
            "portable repository binding changed"
        );
        let hint = edda_store::continuity::derive_portable_repository_remote_hint(checkout)
            .context("portable repository does not have a GitHub remote selector")?;
        let name = hint
            .strip_prefix("github.com/")
            .context("portable repository is not hosted on github.com")?;
        Self::parse(name)
    }

    pub(crate) fn for_control(
        checkout: &Path,
        expected_portable: &str,
        expected_repository: &str,
    ) -> Result<Self> {
        let observed = Self::for_control_compile(checkout, expected_portable)?;
        anyhow::ensure!(
            observed.full_name() == expected_repository,
            "canonical GitHub repository binding changed"
        );
        Ok(observed)
    }

    pub(crate) fn from_checkout(checkout: &Path) -> Result<Self> {
        let hint = edda_store::continuity::derive_portable_repository_remote_hint(checkout)
            .context("cannot derive an explicit GitHub repository selector")?;
        let name = hint
            .strip_prefix("github.com/")
            .context("repository remote is not hosted on github.com")?;
        Self::parse(name)
    }

    fn from_context(checkout: &Path) -> Result<Self> {
        if let Some(value) = std::env::var_os("EDDA_REPO").filter(|value| !value.is_empty()) {
            return Self::parse(&value.to_string_lossy());
        }
        Self::from_checkout(checkout)
    }

    fn parse(value: &str) -> Result<Self> {
        let normalized = value.to_ascii_lowercase();
        edda_core::guided_execution::validate_canonical_github_repository(
            &normalized,
            "GitHub repository selector",
        )?;
        let (owner, name) = normalized
            .split_once('/')
            .context("GitHub repository selector must contain owner/repo")?;
        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    fn concrete_api_arg(&self, value: &str) -> Result<String> {
        if !value.starts_with("repos/") {
            return Ok(value.to_owned());
        }
        let expanded = value
            .replace("{owner}", &self.owner)
            .replace("{repo}", &self.name);
        anyhow::ensure!(
            expanded.starts_with(&format!("repos/{}/{}/", self.owner, self.name)),
            "GitHub API path is outside the validated repository"
        );
        Ok(expanded)
    }
}

/// Production controlled GitHub transport. Its executable and repository
/// selectors cannot be redirected by the environment. Tests inject an
/// explicit binary into this value instead of weakening this constructor.
#[derive(Debug, Clone)]
pub(crate) struct ControlledGitHubTransport {
    binary: std::path::PathBuf,
}

impl ControlledGitHubTransport {
    pub(crate) fn production() -> Result<Self> {
        for variable in ["EDDA_GH_BIN", "GH_REPO", "EDDA_REPO", "GH_HOST"] {
            anyhow::ensure!(
                std::env::var_os(variable).is_none(),
                "controlled GitHub transport refuses environment override {variable}"
            );
        }
        Ok(Self {
            binary: "gh".into(),
        })
    }

    #[cfg(test)]
    pub(crate) fn injected(binary: impl Into<std::path::PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    fn command(&self, repo: &Path, selected: &GitHubRepository, args: &[&str]) -> Result<Command> {
        command_for_binary(&self.binary, repo, selected, args)
    }
}

/// The `gh` this verb runs: `EDDA_GH_BIN` when set, otherwise `gh` from PATH.
/// An empty value names no binary, so it counts as unset.
fn gh_bin() -> std::path::PathBuf {
    std::env::var_os("EDDA_GH_BIN")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("gh"))
}

fn command_for_binary(
    binary: &Path,
    repo: &Path,
    selected: &GitHubRepository,
    args: &[&str],
) -> Result<Command> {
    let mut command = Command::new(binary);
    command
        .current_dir(repo)
        .env_remove("EDDA_GH_BIN")
        .env_remove("GH_REPO")
        .env_remove("EDDA_REPO")
        .env_remove("GH_HOST");
    if args.first() == Some(&"api") {
        let expanded = args
            .iter()
            .map(|value| selected.concrete_api_arg(value))
            .collect::<Result<Vec<_>>>()?;
        command.args(expanded);
    } else {
        command.args(args).args(["--repo", &selected.full_name()]);
    }
    Ok(command)
}

fn command_for(repo: &Path, selected: &GitHubRepository, args: &[&str]) -> Result<Command> {
    command_for_binary(&gh_bin(), repo, selected, args)
}

fn command(repo: &Path, args: &[&str]) -> Result<Command> {
    command_for(repo, &GitHubRepository::from_context(repo)?, args)
}

/// Capture one `gh` invocation through the shared binary/repository seam.
pub(crate) fn gh_capture(repo: &Path, args: &[&str]) -> Result<Output> {
    command(repo, args)?.output().context("run gh")
}

/// Capture a call bound to an already validated repository. Inherited
/// repository/host selectors are always removed from the child environment.
pub(crate) fn gh_capture_for(
    repo: &Path,
    selected: &GitHubRepository,
    args: &[&str],
) -> Result<Output> {
    command_for(repo, selected, args)?
        .output()
        .context("run gh")
}

/// One row from the structured reread used only to diagnose a refused plain
/// required-check read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequiredCheckRow {
    name: String,
    state: String,
    bucket: String,
}

/// Only `Green` satisfies the merge gate. Every diagnostic-only variant is a
/// typed refusal, including a reread whose rows raced to passing/skipping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequiredChecks {
    Green,
    Absent,
    Blocked(Vec<RequiredCheckRow>),
    RacedNowGreen(Vec<RequiredCheckRow>),
    Indeterminate(String),
}

impl RequiredChecks {
    pub(crate) fn refusal(self, pr: u64) -> Option<(i32, String)> {
        let rows = |rows: &[RequiredCheckRow]| {
            rows.iter()
                .map(|row| format!("{} (bucket={}, state={})", row.name, row.bucket, row.state))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Self::Green => None,
            Self::Absent => Some((
                1,
                format!(
                    "no required-check rows are currently reported for PR #{pr}; wait for CI, or \
                     check whether the PR base is covered by a ruleset; refusing"
                ),
            )),
            Self::Blocked(checks) => Some((
                1,
                format!(
                    "required checks are not green for PR #{pr}: {}; refusing",
                    rows(&checks)
                ),
            )),
            Self::RacedNowGreen(checks) => Some((
                1,
                format!(
                    "required-check state changed during the read for PR #{pr}: diagnostic rows \
                     are now all passing/skipping ({}); run `edda review merge` again; refusing",
                    rows(&checks)
                ),
            )),
            Self::Indeterminate(error) => Some((
                2,
                format!("cannot determine required checks for PR #{pr}: {error}; refusing"),
            )),
        }
    }
}

fn required_checks_argv(pr: u64) -> Vec<String> {
    vec![
        "pr".into(),
        "checks".into(),
        pr.to_string(),
        "--required".into(),
    ]
}

fn required_checks_diagnostic_argv(pr: u64) -> Vec<String> {
    let mut argv = required_checks_argv(pr);
    argv.extend(["--json".into(), "name,state,bucket".into()]);
    argv
}

/// Classify by exit status and structured stdout only — never gh's human
/// stderr wording.
fn classify_required_checks(exit: Option<i32>, stdout: &[u8]) -> RequiredChecks {
    if stdout.is_empty() {
        return RequiredChecks::Indeterminate(format!(
            "diagnostic required-check read returned no structured report: stdout was empty \
             (exit {exit:?}); retry after waiting for CI, and check GitHub connectivity and \
             repository access"
        ));
    }
    if !matches!(exit, Some(0 | 1 | 8)) {
        return RequiredChecks::Indeterminate(format!(
            "diagnostic `gh pr checks --required --json name,state,bucket` exited unexpectedly \
             ({exit:?})"
        ));
    }
    let rows: Vec<RequiredCheckRow> = match serde_json::from_slice(stdout) {
        Ok(rows) => rows,
        Err(error) => {
            return RequiredChecks::Indeterminate(format!(
                "diagnostic required-check JSON is malformed or has an unknown row shape: {error}"
            ));
        }
    };
    if rows.is_empty() {
        return RequiredChecks::Absent;
    }
    if let Some(row) = rows.iter().find(|row| {
        row.name.is_empty()
            || row.state.is_empty()
            || !matches!(
                row.bucket.as_str(),
                "pass" | "fail" | "pending" | "skipping" | "cancel"
            )
    }) {
        return RequiredChecks::Indeterminate(format!(
            "diagnostic required-check row has an unknown shape: name={:?}, state={:?}, \
             bucket={:?}",
            row.name, row.state, row.bucket
        ));
    }
    if rows
        .iter()
        .all(|row| matches!(row.bucket.as_str(), "pass" | "skipping"))
    {
        RequiredChecks::RacedNowGreen(rows)
    } else {
        RequiredChecks::Blocked(rows)
    }
}

/// Run the unchanged plain command as the sole Green authority. A nonzero
/// result gets exactly one structured diagnostic reread; that reread can
/// explain a refusal but never approve it.
pub(crate) fn required_checks(repo: &Path, pr: u64) -> RequiredChecks {
    let argv = required_checks_argv(pr);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let plain = match gh_capture(repo, &args) {
        Ok(output) => output,
        Err(error) => {
            return RequiredChecks::Indeterminate(format!(
                "authoritative plain required-check read failed to start: {error:#}"
            ));
        }
    };
    if plain.status.success() {
        return RequiredChecks::Green;
    }
    let argv = required_checks_diagnostic_argv(pr);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    match gh_capture(repo, &args) {
        Ok(output) => classify_required_checks(output.status.code(), &output.stdout),
        Err(error) => RequiredChecks::Indeterminate(format!(
            "diagnostic required-check read failed to start: {error:#}"
        )),
    }
}

pub(crate) fn gh(repo: &Path, args: &[&str]) -> Result<Value> {
    let selected = GitHubRepository::from_context(repo)?;
    gh_for(repo, &selected, args)
}

pub(crate) fn gh_for(repo: &Path, selected: &GitHubRepository, args: &[&str]) -> Result<Value> {
    let output = gh_capture_for(repo, selected, args)?;
    parse_gh_json(output)
}

pub(crate) fn controlled_gh_capture_for(
    transport: &ControlledGitHubTransport,
    repo: &Path,
    selected: &GitHubRepository,
    args: &[&str],
) -> Result<Output> {
    transport
        .command(repo, selected, args)?
        .output()
        .context("run controlled gh")
}

pub(crate) fn controlled_gh_for(
    transport: &ControlledGitHubTransport,
    repo: &Path,
    selected: &GitHubRepository,
    args: &[&str],
) -> Result<Value> {
    parse_gh_json(controlled_gh_capture_for(transport, repo, selected, args)?)
}

fn parse_gh_json(output: Output) -> Result<Value> {
    if !output.status.success() {
        bail!("GitHub request failed with exit {:?}", output.status.code());
    }
    serde_json::from_slice(&output.stdout).context("GitHub response is not one JSON value")
}

pub(crate) fn authenticated_login(
    transport: &ControlledGitHubTransport,
    repo: &Path,
    selected: &GitHubRepository,
) -> Result<String> {
    let user = controlled_gh_for(transport, repo, selected, &["api", "user"])?;
    let login = user["login"]
        .as_str()
        .context("authenticated GitHub user omits canonical login")?;
    edda_core::guided_execution::validate_canonical_github_login(
        login,
        "authenticated GitHub user.login",
    )?;
    Ok(login.to_owned())
}

/// Run `gh` for a write whose stdout is not JSON — a label edit, a comment
/// post, and a status post each print a bare URL or nothing on success, so
/// none of them can go through [`gh`], which parses stdout as JSON. Only
/// the exit status is meaningful here; stdout is discarded.
pub(crate) fn gh_write(repo: &Path, args: &[&str]) -> Result<()> {
    let output = gh_capture(repo, args)?;
    if !output.status.success() {
        bail!("GitHub write failed with exit {:?}", output.status.code());
    }
    Ok(())
}

/// Run `gh` for a write whose stdout is not JSON, feeding `stdin` to it —
/// `--body-file -` reads the merge body from stdin (GH-1105), so no
/// temporary file is staged and no cleanup can race the call.
pub(crate) fn gh_write_stdin(repo: &Path, args: &[&str], stdin: &str) -> Result<()> {
    use std::io::Write;
    let mut child = command(repo, args)?
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("run gh")?;
    child
        .stdin
        .take()
        .context("gh stdin")?
        .write_all(stdin.as_bytes())
        .context("write gh stdin")?;
    let output = child.wait_with_output().context("run gh")?;
    if !output.status.success() {
        bail!("GitHub write failed with exit {:?}", output.status.code());
    }
    Ok(())
}

/// The PR's current head SHA — a lighter read than [`resolve_pr`], which
/// also fetches the head and base commits into private refs for `edda
/// review`'s own diffing. `edda review deliver`'s moved-head check only
/// ever compares this string; it has no need to make the commit locally
/// resolvable.
#[derive(Debug, Clone)]
pub(crate) struct PrDeliverySubject {
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub base_sha: String,
    pub state: String,
}

pub(crate) fn pr_delivery_subject_for(
    transport: &ControlledGitHubTransport,
    repo: &Path,
    selected: &GitHubRepository,
    number: u64,
) -> Result<PrDeliverySubject> {
    let value = controlled_gh_for(
        transport,
        repo,
        selected,
        &[
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "headRefOid,headRefName,baseRefName,baseRefOid,state",
        ],
    )?;
    let subject = PrDeliverySubject {
        head_sha: value["headRefOid"]
            .as_str()
            .context("PR head missing")?
            .to_owned(),
        head_ref: value["headRefName"]
            .as_str()
            .context("PR head ref missing")?
            .to_owned(),
        base_ref: value["baseRefName"]
            .as_str()
            .context("PR base ref missing")?
            .to_owned(),
        base_sha: value["baseRefOid"]
            .as_str()
            .context("PR base SHA missing")?
            .to_owned(),
        state: value["state"]
            .as_str()
            .context("PR state missing")?
            .to_owned(),
    };
    for (sha, field) in [
        (&subject.head_sha, "PR head"),
        (&subject.base_sha, "PR base"),
    ] {
        anyhow::ensure!(
            sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid {field} SHA"
        );
    }
    Ok(subject)
}

pub(crate) fn pr_head(repo: &Path, number: u64) -> Result<String> {
    let value = gh(
        repo,
        &["pr", "view", &number.to_string(), "--json", "headRefOid"],
    )?;
    let head = value["headRefOid"].as_str().context("PR head missing")?;
    anyhow::ensure!(
        head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid PR head SHA"
    );
    Ok(head.to_owned())
}

pub(crate) struct PrSubject {
    pub head: String,
    pub base: String,
    pub issue: Option<u64>,
}

/// Fetch one remote ref into a review-private ref, and resolve that.
///
/// `FETCH_HEAD` lives in the **common** git directory, so the main checkout
/// and every linked worktree share one cell. Two `edda review` processes
/// fetching at the same time overwrite each other's between one process's
/// fetch and its `rev-parse`, and the loser resolves whatever the other left
/// behind — a foreign commit, or the multi-line value git reports as
/// `fatal: Needed a single revision` (GH-997). The fleet launches per-PR
/// review lanes into one checkout and serialises none of them.
///
/// Naming a private destination removes the shared cell rather than guarding
/// it: concurrent reviews of different PRs never name the same ref, and two
/// reviews of the same PR fetch the same objects to it. `--force` is required
/// because a PR head that was rebased or force-pushed moves the ref
/// non-fast-forward. The ref is left in place: it costs one file, and it
/// keeps the reviewed objects resolvable for a SHA-pinned verdict.
fn fetch_private_ref(repo: &Path, number: u64, role: &str, remote_ref: &str) -> Result<String> {
    let private = format!("refs/edda/review/pr{number}/{role}");
    git(
        repo,
        &[
            "fetch",
            "--no-tags",
            "--force",
            "origin",
            &format!("{remote_ref}:{private}"),
        ],
    )?;
    commit(repo, &private)
}

pub(crate) fn resolve_pr(repo: &Path, number: u64) -> Result<PrSubject> {
    for _ in 0..2 {
        let value = gh(
            repo,
            &[
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "headRefOid,baseRefName,body",
            ],
        )?;
        let expected = value["headRefOid"].as_str().context("PR head missing")?;
        if expected.len() != 40 || !expected.bytes().all(|v| v.is_ascii_hexdigit()) {
            bail!("invalid PR head SHA");
        }
        if fetch_private_ref(repo, number, "head", &format!("pull/{number}/head"))? != expected {
            continue;
        }
        let base = value["baseRefName"].as_str().context("PR base missing")?;
        git(repo, &["check-ref-format", &format!("refs/heads/{base}")])?;
        let base_sha = fetch_private_ref(repo, number, "base", &format!("refs/heads/{base}"))?;
        return Ok(PrSubject {
            head: expected.into(),
            base: base_sha,
            issue: closing_issue(value["body"].as_str().unwrap_or("")),
        });
    }
    bail!("PR changed during fetch; retry review")
}

pub(crate) fn closing_issue(body: &str) -> Option<u64> {
    let words = body.split_whitespace().collect::<Vec<_>>();
    words.windows(2).find_map(|pair| {
        let keyword = pair[0]
            .trim_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_lowercase();
        if ![
            "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
        ]
        .contains(&keyword.as_str())
        {
            return None;
        }
        let issue = pair[1]
            .trim_end_matches(|c: char| c.is_ascii_punctuation() && c != '#')
            .strip_prefix('#')?;
        issue.parse().ok()
    })
}

pub(crate) fn load_spec(
    repo: &Path,
    cwd: &Path,
    explicit: Option<&str>,
    inferred: Option<u64>,
    trust: bool,
) -> Result<(ReviewSpec, String, Option<u64>)> {
    if let Some(path) = explicit.filter(|s| !s.starts_with('#')) {
        let text =
            std::fs::read_to_string(cwd.join(path)).with_context(|| format!("read spec {path}"))?;
        return Ok((
            ReviewSpec {
                mode: "spec-backed".into(),
                source: path.into(),
                trust: spec_trust(&SpecOrigin::Path, false).into(),
            },
            text,
            None,
        ));
    }
    let issue = match explicit {
        Some(value) => Some(
            value
                .strip_prefix('#')
                .context("expected #issue")?
                .parse::<u64>()?,
        ),
        None => inferred,
    };
    let Some(number) = issue else {
        return Ok((
            ReviewSpec {
                mode: "convention-only".into(),
                source: "none".into(),
                trust: spec_trust(&SpecOrigin::None, false).into(),
            },
            String::new(),
            None,
        ));
    };
    let value = gh(
        repo,
        &[
            "issue",
            "view",
            &number.to_string(),
            "--json",
            "body,author",
        ],
    )?;
    let body = value["body"]
        .as_str()
        .context("issue body missing")?
        .to_owned();
    let origin = if explicit.is_some() {
        SpecOrigin::ExplicitIssue
    } else {
        let login = value["author"]["login"]
            .as_str()
            .context("issue author missing")?;
        let permission = gh(
            repo,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/collaborators/{login}/permission"),
            ],
        );
        SpecOrigin::PrDerived {
            author_perm: permission
                .ok()
                .and_then(|v| v["permission"].as_str().map(str::to_owned)),
        }
    };
    Ok((
        ReviewSpec {
            mode: "spec-backed".into(),
            source: format!("issue#{number}"),
            trust: spec_trust(&origin, trust).into(),
        },
        body,
        Some(number),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_review::git::testrepo;

    #[test]
    fn every_command_uses_the_validated_repo_and_removes_ambient_selectors() {
        let selected = GitHubRepository::parse("owner/repo").unwrap();
        let command = command_for(
            Path::new("."),
            &selected,
            &["pr", "view", "7", "--json", "headRefOid"],
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["--repo", "owner/repo"]));
        let env = command
            .get_envs()
            .map(|(key, value)| (key.to_string_lossy().into_owned(), value))
            .collect::<Vec<_>>();
        for key in ["GH_REPO", "EDDA_REPO", "GH_HOST"] {
            assert!(env
                .iter()
                .any(|(actual, value)| actual == key && value.is_none()));
        }

        let api = command_for(
            Path::new("."),
            &selected,
            &["api", "repos/{owner}/{repo}/issues/7"],
        )
        .unwrap();
        let args = api
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["api", "repos/owner/repo/issues/7"]);

        let fields = command_for(
            Path::new("."),
            &selected,
            &[
                "api",
                "--method",
                "POST",
                "repos/{owner}/{repo}/git/tags",
                "-f",
                "message={owner}/{repo}",
            ],
        )
        .unwrap();
        let args = fields
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.contains(&"repos/owner/repo/git/tags".to_string()));
        assert!(args.contains(&"message={owner}/{repo}".to_string()));
    }

    #[test]
    fn required_check_argv_pins_the_pr_and_diagnostic_fields() {
        assert_eq!(
            required_checks_argv(4242),
            ["pr", "checks", "4242", "--required"]
        );
        assert_eq!(
            required_checks_diagnostic_argv(4242),
            [
                "pr",
                "checks",
                "4242",
                "--required",
                "--json",
                "name,state,bucket"
            ]
        );
    }

    #[test]
    fn required_check_output_is_classified_without_human_stderr() {
        for exit in [Some(0), Some(1), Some(8), Some(9), None] {
            let RequiredChecks::Indeterminate(message) = classify_required_checks(exit, b"") else {
                panic!("empty output with {exit:?} must be indeterminate");
            };
            for expected in [
                "no structured report",
                "retry",
                "waiting for CI",
                "GitHub connectivity",
                "repository access",
            ] {
                assert!(
                    message.contains(expected),
                    "missing {expected:?}: {message}"
                );
            }
            assert!(!message.contains("no required-check rows"), "{message}");
        }
        assert_eq!(
            classify_required_checks(Some(0), b"[]"),
            RequiredChecks::Absent
        );
        let blocked = br#"[{"name":"build","state":"FAILURE","bucket":"fail"},
            {"name":"test","state":"PENDING","bucket":"pending"}]"#;
        assert!(matches!(
            classify_required_checks(Some(0), blocked),
            RequiredChecks::Blocked(rows)
                if rows.iter().map(|row| row.bucket.as_str()).collect::<Vec<_>>()
                    == ["fail", "pending"]
        ));
        let raced = br#"[{"name":"build","state":"SUCCESS","bucket":"pass"},
            {"name":"docs","state":"SKIPPED","bucket":"skipping"}]"#;
        assert!(matches!(
            classify_required_checks(Some(1), raced),
            RequiredChecks::RacedNowGreen(rows) if rows.len() == 2
        ));
        for result in [
            classify_required_checks(Some(0), b"{"),
            classify_required_checks(Some(9), b"[]"),
            classify_required_checks(
                Some(0),
                br#"[{"name":"CI Gate","state":"SUCCESS","bucket":"mystery"}]"#,
            ),
        ] {
            assert!(matches!(result, RequiredChecks::Indeterminate(_)));
        }
    }

    #[test]
    fn controlled_transport_refuses_every_environment_override() {
        let _injected_test_transport = ControlledGitHubTransport::injected("gh");
        let _lock = crate::claim_guard::GH_BIN_ENV_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for variable in ["EDDA_GH_BIN", "GH_REPO", "EDDA_REPO", "GH_HOST"] {
            let previous = std::env::var_os(variable);
            std::env::set_var(variable, "attacker/override");
            let error = ControlledGitHubTransport::production().unwrap_err();
            assert!(error.to_string().contains(variable), "{error}");
            match previous {
                Some(value) => std::env::set_var(variable, value),
                None => std::env::remove_var(variable),
            }
        }
    }

    #[test]
    fn configured_portable_key_cannot_hide_a_changed_control_remote() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success());
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(root.path())
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ]);
        std::fs::create_dir_all(root.path().join(".edda")).unwrap();
        std::fs::write(
            root.path().join(".edda/config.json"),
            r#"{"portable_repo_key":"stable/key"}"#,
        )
        .unwrap();
        let paths = edda_ledger::EddaPaths::discover(root.path());
        let portable = edda_store::continuity::derive_portable_repository_identity(
            root.path(),
            &paths.config_json,
        )
        .unwrap()
        .portable_repo_id
        .unwrap();
        assert_eq!(
            GitHubRepository::for_control(root.path(), &portable, "owner/repo")
                .unwrap()
                .full_name(),
            "owner/repo"
        );
        git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/attacker/foreign.git",
        ]);
        let error = GitHubRepository::for_control(root.path(), &portable, "owner/repo")
            .unwrap_err()
            .to_string();
        assert!(error.contains("binding changed"), "{error}");
    }

    #[test]
    fn closing_keywords_ignore_mentions_and_use_first_closing_issue() {
        assert_eq!(closing_issue("Issue: #1\nCloses #652. Fixes #3"), Some(652));
        assert_eq!(closing_issue("encloses #9; related #652"), None);
        assert_eq!(closing_issue("resolved #7"), Some(7));
    }

    /// Two review lanes in one checkout, interleaved at the point that used
    /// to lose the subject: lane 1 fetches, lane 2 fetches, lane 1 resolves.
    /// Driving the interleaving directly rather than racing two threads makes
    /// the regression deterministic — the old code loses on every run, not on
    /// an unlucky one.
    #[test]
    fn a_concurrent_fetch_cannot_redirect_a_resolved_subject() {
        let (_origin_temp, origin) = testrepo::init();
        let pr1 = testrepo::commit_file(&origin, "pr1.txt", "one\n", "pr 1");
        testrepo::run(&origin, &["update-ref", "refs/pull/1/head", &pr1]);
        let pr2 = testrepo::commit_file(&origin, "pr2.txt", "two\n", "pr 2");
        testrepo::run(&origin, &["update-ref", "refs/pull/2/head", &pr2]);
        assert_ne!(pr1, pr2);

        let (_work_temp, work) = testrepo::init();
        testrepo::run(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );

        // Lane 1 fetches its PR.
        let resolved = fetch_private_ref(&work, 1, "head", "pull/1/head").expect("lane 1 fetch");
        assert_eq!(resolved, pr1);

        // Lane 2 fetches its own, moving the cell both lanes used to read.
        fetch_private_ref(&work, 2, "head", "pull/2/head").expect("lane 2 fetch");
        assert_eq!(
            commit(&work, "FETCH_HEAD").expect("shared cell"),
            pr2,
            "precondition: the shared FETCH_HEAD really did move under lane 1"
        );

        // Lane 1 resolves its subject. Anything reading the shared cell now
        // answers pr2; lane 1's own ref still answers pr1.
        assert_eq!(
            commit(&work, "refs/edda/review/pr1/head").expect("lane 1 subject"),
            pr1,
            "lane 1 must resolve its own PR, not whatever lane 2 last fetched"
        );
    }

    /// A rebased or force-pushed head moves the private ref non-fast-forward;
    /// a second review of the same PR must follow it rather than fail.
    #[test]
    fn a_force_pushed_head_moves_the_private_ref() {
        let (_origin_temp, origin) = testrepo::init();
        let first = testrepo::commit_file(&origin, "pr.txt", "first\n", "round 1");
        testrepo::run(&origin, &["update-ref", "refs/pull/7/head", &first]);

        let (_work_temp, work) = testrepo::init();
        testrepo::run(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        assert_eq!(
            fetch_private_ref(&work, 7, "head", "pull/7/head").expect("round 1"),
            first
        );

        testrepo::run(&origin, &["reset", "-q", "--hard", "HEAD~1"]);
        let rewritten = testrepo::commit_file(&origin, "pr.txt", "rewritten\n", "round 2");
        testrepo::run(&origin, &["update-ref", "refs/pull/7/head", &rewritten]);
        assert_eq!(
            fetch_private_ref(&work, 7, "head", "pull/7/head").expect("round 2"),
            rewritten
        );
    }
}
