//! Owner-bound return continuity.
//!
//! A delegated job's completion must survive the replacement of the assistant
//! session that delegated it. `edda-pi send` addresses a *session*, so a stale
//! return address either reaches a dead session, is lost, or is presented twice.
//! This module stores returns against a stable **owner reference** with an
//! explicit holder binding and an atomic exactly-once claim, so the current
//! responsible session consumes each completion once and a superseded session
//! cannot re-present it.
//!
//! It is a durable local mailbox, not a scheduler, second task system, role lock
//! or wake adapter: nothing runs on its own. A responsible session consumes
//! pending returns when it next takes a turn.
//!
//! The mailbox root is the workspace containing the caller's cwd, or the
//! absolute path in `EDDA_RETURN_ROOT` when a managed launcher pins one shared
//! root for an assistant and its controllers across sibling project directories.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use edda_core::hash::sha256_hex;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Subcommand, Debug)]
pub enum ReturnCmd {
    /// Register (or explicitly replace) the current responsible holder of an owner reference
    Bind(BindArgs),
    /// Post a controller's done/failed return against an owner reference
    Post(PostArgs),
    /// Read-only: unclaimed returns for an owner reference
    Pending(OwnerArgs),
    /// Atomically claim unclaimed returns — only the current holder may claim
    Claim(ClaimArgs),
    /// Read one return by id
    Show(ShowArgs),
    /// Read-only: the current holder and return counts
    Status(OwnerArgs),
}

#[derive(Args, Debug)]
pub struct BindArgs {
    /// Stable owner reference, e.g. `assistant/coord-delegate-fresh`
    #[arg(long)]
    pub owner: String,
    /// Session id that is now responsible for this owner reference
    #[arg(long)]
    pub session: String,
    /// Explicit identity replacement: the session currently bound, which this bind supersedes
    #[arg(long)]
    pub replaces_session: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct PostArgs {
    #[arg(long)]
    pub owner: String,
    /// Work / job identifier within the owner reference
    #[arg(long)]
    pub work: String,
    /// done | failed
    #[arg(long)]
    pub status: String,
    /// One-line result
    #[arg(long)]
    pub result: Option<String>,
    /// Deliverable path
    #[arg(long)]
    pub deliverable: Option<String>,
    /// Full report; `-` reads stdin
    #[arg(long)]
    pub message_file: Option<String>,
    /// Posting controller session id
    #[arg(long)]
    pub session: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct OwnerArgs {
    #[arg(long)]
    pub owner: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ClaimArgs {
    #[arg(long)]
    pub owner: String,
    /// Session claiming; refused unless it is the current holder
    #[arg(long)]
    pub session: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct OwnerRecord {
    version: u32,
    owner: String,
    holder_session: String,
    holder_since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replaced_session: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct MessageRecord {
    version: u32,
    id: String,
    owner: String,
    work: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deliverable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    posted_by_session: String,
    posted_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ClaimRecord {
    version: u32,
    message_id: String,
    claimed_by_session: String,
    claimed_at: String,
}

fn returns_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".edda").join("returns")
}

/// The mailbox is rooted at the Edda workspace containing the caller's cwd by
/// default. `EDDA_RETURN_ROOT` lets a managed launcher pin one explicit mailbox
/// root shared by an assistant and its delegated controllers, whose project
/// directories need not share an `.edda`/`.git` workspace root. The override is
/// additive: with it unset the resolved workspace root is used unchanged, so
/// existing per-project callers keep their mailbox.
fn mailbox_root(repo_root: &Path, override_value: Option<std::ffi::OsString>) -> Result<PathBuf> {
    match override_value {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(&value);
            if !path.is_absolute() {
                bail!("EDDA_RETURN_ROOT must be an absolute path");
            }
            Ok(path)
        }
        _ => Ok(repo_root.to_path_buf()),
    }
}

fn owner_file(dir: &Path, owner: &str) -> PathBuf {
    dir.join("owners")
        .join(format!("{}.json", sha256_hex(owner.as_bytes())))
}

fn message_file(dir: &Path, id: &str) -> PathBuf {
    dir.join("messages").join(format!("{id}.json"))
}

fn claim_file(dir: &Path, id: &str) -> PathBuf {
    dir.join("claims").join(format!("{id}.json"))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn ensure_layout(dir: &Path) -> Result<()> {
    for sub in ["owners", "messages", "claims"] {
        fs::create_dir_all(dir.join(sub))
            .with_context(|| format!("create {}", dir.join(sub).display()))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, record: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    if let Err(error) = write_temp(&temp, &bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("rename into {}", path.display()));
    }
    Ok(())
}

/// Write `bytes` to `temp` and flush them to disk, so the later rename into the
/// final path can never expose a partially written file.
fn write_temp(temp: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::File::create(temp).with_context(|| format!("create {}", temp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn list_json<T: for<'de> Deserialize<'de>>(dir: &Path) -> Result<Vec<T>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(value) = read_json::<T>(&path)? {
            out.push(value);
        }
    }
    Ok(out)
}

fn holder(dir: &Path, owner: &str) -> Result<Option<OwnerRecord>> {
    read_json(&owner_file(dir, owner))
}

/// Serialize mutations with a process lock so a bind cannot race a claim.
fn lock(dir: &Path) -> Result<fs::File> {
    let path = dir.join(".lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("lock {}", path.display()))?;
    Ok(file)
}

fn read_message(dir: &Path, id: &str) -> Result<Option<MessageRecord>> {
    read_json(&message_file(dir, id))
}

/// A claim marker counts only when it holds a complete record. A zero-length
/// marker is a crash-incomplete write and must not hide the return forever.
fn is_claimed(dir: &Path, id: &str) -> Result<bool> {
    let path = claim_file(dir, id);
    match fs::metadata(&path) {
        Ok(meta) => Ok(meta.len() > 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("stat {}", path.display())),
    }
}

fn pending_messages(dir: &Path, owner: &str) -> Result<Vec<MessageRecord>> {
    let mut pending = Vec::new();
    for message in list_json::<MessageRecord>(&dir.join("messages"))? {
        if message.owner == owner && !is_claimed(dir, &message.id)? {
            pending.push(message);
        }
    }
    pending.sort_by(|a, b| a.posted_at.cmp(&b.posted_at).then_with(|| a.id.cmp(&b.id)));
    Ok(pending)
}

pub fn execute(cmd: ReturnCmd, repo_root: &Path) -> Result<()> {
    let root = mailbox_root(repo_root, std::env::var_os("EDDA_RETURN_ROOT"))?;
    let dir = returns_dir(&root);
    match cmd {
        ReturnCmd::Bind(args) => bind(&dir, args),
        ReturnCmd::Post(args) => post(&dir, args),
        ReturnCmd::Pending(args) => pending(&dir, args),
        ReturnCmd::Claim(args) => claim(&dir, args),
        ReturnCmd::Show(args) => show(&dir, args),
        ReturnCmd::Status(args) => status(&dir, args),
    }
}

fn bind(dir: &Path, args: BindArgs) -> Result<()> {
    ensure_layout(dir)?;
    let _guard = lock(dir)?;
    let existing = holder(dir, &args.owner)?;
    let replaced = existing
        .as_ref()
        .map(|record| record.holder_session.clone());
    if let Some(current) = &replaced {
        if *current != args.session && args.replaces_session.as_deref() != Some(current.as_str()) {
            bail!(
                "owner '{}' is held by session '{}'; pass --replaces-session '{}' to replace it explicitly",
                args.owner,
                current,
                current
            );
        }
    }
    let record = OwnerRecord {
        version: 1,
        owner: args.owner.clone(),
        holder_session: args.session.clone(),
        holder_since: now(),
        replaced_session: replaced.clone().filter(|old| old != &args.session),
    };
    write_atomic(&owner_file(dir, &args.owner), &record)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "bound",
                "owner": record.owner,
                "holderSession": record.holder_session,
                "replacedSession": record.replaced_session,
                "holderSince": record.holder_since,
            }))?
        );
    } else {
        println!(
            "owner {} -> session {}",
            record.owner, record.holder_session
        );
    }
    Ok(())
}

fn post(dir: &Path, args: PostArgs) -> Result<()> {
    if args.status != "done" && args.status != "failed" {
        bail!("--status must be 'done' or 'failed'");
    }
    ensure_layout(dir)?;
    let message = match args.message_file.as_deref() {
        None => None,
        Some("-") => {
            use std::io::Read;
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Some(text)
        }
        Some(path) => Some(fs::read_to_string(path).with_context(|| format!("read {path}"))?),
    };
    let _guard = lock(dir)?;
    // Content hash over the whole record: a corrected --result or
    // --deliverable is a new return, while an identical re-post keeps the same
    // id and stays idempotent.
    let id = sha256_hex(
        format!(
            "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
            args.owner,
            args.work,
            args.status,
            args.session,
            message.as_deref().unwrap_or(""),
            args.result.as_deref().unwrap_or(""),
            args.deliverable.as_deref().unwrap_or("")
        )
        .as_bytes(),
    );
    let record = MessageRecord {
        version: 1,
        id: id.clone(),
        owner: args.owner.clone(),
        work: args.work.clone(),
        status: args.status.clone(),
        result: args.result.clone(),
        deliverable: args.deliverable.clone(),
        message,
        posted_by_session: args.session.clone(),
        posted_at: now(),
    };
    if read_message(dir, &id)?.is_none() {
        write_atomic(&message_file(dir, &id), &record)?;
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "posted",
                "messageId": record.id,
                "owner": record.owner,
                "work": record.work,
                "state": record.status,
            }))?
        );
    } else {
        println!(
            "posted {} ({}) for owner {}",
            record.work, record.status, record.owner
        );
    }
    Ok(())
}

fn pending(dir: &Path, args: OwnerArgs) -> Result<()> {
    let messages = pending_messages(dir, &args.owner)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "owner": args.owner,
                "pending": messages,
                "count": messages.len(),
            }))?
        );
    } else if messages.is_empty() {
        println!("no pending returns for owner {}", args.owner);
    } else {
        for message in &messages {
            println!(
                "{}\t{}\t{}\t{}",
                message.id,
                message.work,
                message.status,
                message.result.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn claim(dir: &Path, args: ClaimArgs) -> Result<()> {
    ensure_layout(dir)?;
    let _guard = lock(dir)?;
    let current = holder(dir, &args.owner)?;
    let Some(record) = current else {
        bail!(
            "no owner binding for '{}'; bind before claiming",
            args.owner
        );
    };
    if record.holder_session != args.session {
        bail!(
            "owner '{}' is held by session '{}', not '{}'; a superseded session cannot claim",
            args.owner,
            record.holder_session,
            args.session
        );
    }
    let mut claimed = Vec::new();
    for message in pending_messages(dir, &args.owner)? {
        let claim = ClaimRecord {
            version: 1,
            message_id: message.id.clone(),
            claimed_by_session: args.session.clone(),
            claimed_at: now(),
        };
        // write_atomic (temp + sync + rename) means a crash cannot leave a
        // zero-length marker at the final path and hide this return forever.
        // The exclusive lock plus pending_messages' non-empty-marker filter
        // make the write exactly-once: a valid marker is skipped, a missing or
        // zero-length one is claimed now.
        write_atomic(&claim_file(dir, &message.id), &claim)?;
        claimed.push(message);
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "claimed",
                "owner": args.owner,
                "session": args.session,
                "returns": claimed,
                "count": claimed.len(),
            }))?
        );
    } else if claimed.is_empty() {
        println!("no unclaimed returns for owner {}", args.owner);
    } else {
        for message in &claimed {
            println!(
                "{}\t{}\t{}\t{}",
                message.id,
                message.work,
                message.status,
                message.result.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

/// Ids are lowercase sha256 hex; rejecting anything else keeps caller input out
/// of the filesystem path so `show --id` cannot traverse out of the mailbox.
/// Uppercase hex is rejected too: it passes a case-insensitive shape check but
/// can never name a stored id, which would surface as `unknown return id`
/// instead of the invalid-id error this guard exists to give.
fn validate_message_id(id: &str) -> Result<()> {
    let lowercase_hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    if id.len() != 64 || !id.bytes().all(lowercase_hex) {
        bail!("invalid return id '{id}': expected 64 lowercase hex characters");
    }
    Ok(())
}

fn show(dir: &Path, args: ShowArgs) -> Result<()> {
    validate_message_id(&args.id)?;
    let Some(message) = read_message(dir, &args.id)? else {
        bail!("unknown return id '{}'", args.id);
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&message)?);
    } else {
        println!(
            "{} {} {}",
            message.work,
            message.status,
            message.result.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn status(dir: &Path, args: OwnerArgs) -> Result<()> {
    let record = holder(dir, &args.owner)?;
    let pending = pending_messages(dir, &args.owner)?.len();
    let all = list_json::<MessageRecord>(&dir.join("messages"))?
        .into_iter()
        .filter(|m| m.owner == args.owner)
        .count();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "owner": args.owner,
                "holder": record.as_ref().map(|r| r.holder_session.clone()),
                "holderSince": record.as_ref().map(|r| r.holder_since.clone()),
                "pending": pending,
                "total": all,
            }))?
        );
    } else {
        match record {
            Some(record) => println!(
                "owner {} held by {} (pending {pending}, total {all})",
                args.owner, record.holder_session
            ),
            None => println!(
                "owner {} unbound (pending {pending}, total {all})",
                args.owner
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("edda-return-{}", ulid::Ulid::new()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bind(dir: &Path, owner: &str, session: &str, replaces: Option<&str>) -> Result<()> {
        super::bind(
            dir,
            BindArgs {
                owner: owner.into(),
                session: session.into(),
                replaces_session: replaces.map(str::to_owned),
                json: true,
            },
        )
    }

    fn post(dir: &Path, owner: &str, work: &str, session: &str) -> Result<()> {
        post_with(dir, owner, work, session, "ok", "out.md")
    }

    fn post_with(
        dir: &Path,
        owner: &str,
        work: &str,
        session: &str,
        result: &str,
        deliverable: &str,
    ) -> Result<()> {
        super::post(
            dir,
            PostArgs {
                owner: owner.into(),
                work: work.into(),
                status: "done".into(),
                result: Some(result.into()),
                deliverable: Some(deliverable.into()),
                message_file: None,
                session: session.into(),
                json: true,
            },
        )
    }

    fn claim(dir: &Path, owner: &str, session: &str) -> Result<()> {
        super::claim(
            dir,
            ClaimArgs {
                owner: owner.into(),
                session: session.into(),
                json: true,
            },
        )
    }

    #[test]
    fn bind_is_explicit_about_replacement() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        assert!(bind(&dir, "assistant/p", "s2", None).is_err());
        bind(&dir, "assistant/p", "s2", Some("s1")).unwrap();
        assert_eq!(
            holder(&dir, "assistant/p").unwrap().unwrap().holder_session,
            "s2"
        );
    }

    #[test]
    fn return_is_claimed_exactly_once_and_survives_replacement() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        // Replacement takes over before the report is consumed.
        bind(&dir, "assistant/p", "s2", Some("s1")).unwrap();
        assert!(
            claim(&dir, "assistant/p", "s1").is_err(),
            "superseded session must be refused"
        );
        assert_eq!(
            pending_messages(&dir, "assistant/p").unwrap().len(),
            1,
            "refusal must not drop the return"
        );
        claim(&dir, "assistant/p", "s2").unwrap();
        assert!(pending_messages(&dir, "assistant/p").unwrap().is_empty());
        // Exactly once: the replacement's next claim is empty and cannot double-present.
        claim(&dir, "assistant/p", "s2").unwrap();
        assert!(pending_messages(&dir, "assistant/p").unwrap().is_empty());
    }

    #[test]
    fn post_is_idempotent() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 1);
    }

    #[test]
    fn a_corrected_result_is_a_new_return_not_a_silent_skip() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        post_with(
            &dir,
            "assistant/p",
            "job-a",
            "controller-1",
            "corrected",
            "out2.md",
        )
        .unwrap();
        let pending = pending_messages(&dir, "assistant/p").unwrap();
        assert_eq!(pending.len(), 2, "a corrected re-post must not be dropped");
        assert!(pending
            .iter()
            .any(|m| m.result.as_deref() == Some("corrected")));
        assert!(pending
            .iter()
            .any(|m| m.deliverable.as_deref() == Some("out2.md")));
        // The identical re-post of the corrected record is still idempotent.
        post_with(
            &dir,
            "assistant/p",
            "job-a",
            "controller-1",
            "corrected",
            "out2.md",
        )
        .unwrap();
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 2);
    }

    #[test]
    fn same_session_behaviour_is_unchanged() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        claim(&dir, "assistant/p", "s1").unwrap();
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 0);
        claim(&dir, "assistant/p", "s1").unwrap();
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 0);
    }

    #[test]
    fn unbound_owner_cannot_claim_and_pending_stays() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 1);
        assert!(claim(&dir, "assistant/other", "s1").is_err());
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 1);
    }

    #[test]
    fn a_claim_writes_a_durable_exactly_once_marker() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        let id = pending_messages(&dir, "assistant/p").unwrap()[0].id.clone();
        assert!(!is_claimed(&dir, &id).unwrap());
        claim(&dir, "assistant/p", "s1").unwrap();
        assert!(is_claimed(&dir, &id).unwrap());
    }

    #[test]
    fn an_empty_claim_marker_does_not_hide_a_return() {
        let dir = temp();
        bind(&dir, "assistant/p", "s1", None).unwrap();
        post(&dir, "assistant/p", "job-a", "controller-1").unwrap();
        let id = pending_messages(&dir, "assistant/p").unwrap()[0].id.clone();
        // Simulate a crash after create but before the record was written.
        fs::write(claim_file(&dir, &id), b"").unwrap();
        assert!(
            !is_claimed(&dir, &id).unwrap(),
            "an empty marker is not a claim"
        );
        assert_eq!(pending_messages(&dir, "assistant/p").unwrap().len(), 1);
        claim(&dir, "assistant/p", "s1").unwrap();
        assert!(is_claimed(&dir, &id).unwrap());
        let written = fs::read(claim_file(&dir, &id)).unwrap();
        assert!(serde_json::from_slice::<ClaimRecord>(&written).is_ok());
        assert!(pending_messages(&dir, "assistant/p").unwrap().is_empty());
    }

    #[test]
    fn show_rejects_an_id_that_is_not_a_content_hash() {
        let dir = temp();
        let traversal = show(
            &dir,
            ShowArgs {
                id: "../owners/x".into(),
                json: false,
            },
        )
        .unwrap_err();
        assert!(traversal.to_string().contains("invalid return id"));
        let short = show(
            &dir,
            ShowArgs {
                id: "abc".into(),
                json: false,
            },
        )
        .unwrap_err();
        assert!(short.to_string().contains("invalid return id"));
        // Uppercase hex has the right shape but can never name a stored sha256.
        let uppercase = show(
            &dir,
            ShowArgs {
                id: "A".repeat(64),
                json: false,
            },
        )
        .unwrap_err();
        assert!(uppercase.to_string().contains("invalid return id"));
        // A well-formed 64-hex id that does not exist is a clean miss.
        let missing = show(
            &dir,
            ShowArgs {
                id: "a".repeat(64),
                json: true,
            },
        )
        .unwrap_err();
        assert!(missing.to_string().contains("unknown return id"));
    }

    #[test]
    fn mailbox_root_prefers_an_explicit_absolute_override() {
        let repo = std::env::temp_dir();
        assert_eq!(mailbox_root(&repo, None).unwrap(), repo);
        assert_eq!(
            mailbox_root(&repo, Some(std::ffi::OsString::new())).unwrap(),
            repo
        );
        let shared = repo.join("shared-owners");
        assert_eq!(
            mailbox_root(&repo, Some(shared.clone().into_os_string())).unwrap(),
            shared
        );
        assert!(mailbox_root(&repo, Some(std::ffi::OsString::from("relative/path"))).is_err());
    }
}
