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
    {
        let mut file =
            fs::File::create(&temp).with_context(|| format!("create {}", temp.display()))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp, path).with_context(|| format!("rename into {}", path.display()))?;
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

fn is_claimed(dir: &Path, id: &str) -> Result<bool> {
    Ok(claim_file(dir, id).exists())
}

fn pending_messages(dir: &Path, owner: &str) -> Result<Vec<MessageRecord>> {
    let mut messages = list_json::<MessageRecord>(&dir.join("messages"))?;
    messages.retain(|m| m.owner == owner && !is_claimed(dir, &m.id).unwrap_or(true));
    messages.sort_by(|a, b| a.posted_at.cmp(&b.posted_at).then_with(|| a.id.cmp(&b.id)));
    Ok(messages)
}

pub fn execute(cmd: ReturnCmd, repo_root: &Path) -> Result<()> {
    let dir = returns_dir(repo_root);
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
    let id = sha256_hex(
        format!(
            "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
            args.owner,
            args.work,
            args.status,
            args.session,
            message.as_deref().unwrap_or("")
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
        let path = claim_file(dir, &message.id);
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&serde_json::to_vec_pretty(&claim)?)?;
                claimed.push(message);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).with_context(|| format!("claim {}", path.display())),
        }
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

fn show(dir: &Path, args: ShowArgs) -> Result<()> {
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
        super::post(
            dir,
            PostArgs {
                owner: owner.into(),
                work: work.into(),
                status: "done".into(),
                result: Some("ok".into()),
                deliverable: Some("out.md".into()),
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
}
