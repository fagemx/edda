//! Portable replication of immutable posted returns between isolated registries.
//!
//! `edda return replicate --export/--import` moves *only* allowlisted immutable
//! posted-return envelopes between two mailboxes through one portable JSON file.
//! It is deliberately not a network transport, a scheduler, a second task system
//! or a new carrier: one file, one direction per invocation, no daemon.
//!
//! Trust boundary: the file declares `version`/`kind` and every envelope is
//! deserialized with `deny_unknown_fields`, so an envelope carrying a foreign
//! key (`posted_by_session`, `session`, `holder`, `ownerRoot`, `path`, ...) is
//! refused rather than ignored, and a tampered `logicalId` is rejected by
//! recomputation. Import creates only `messages/`: it never creates or writes
//! `owners/` or `claims/`, so the local exactly-once claim arbiter is untouched
//! and an imported return stays unclaimable until this machine binds the owner.

use super::*;

#[derive(Args, Debug)]
pub struct ReplicateArgs {
    /// Export the local mailbox's immutable posted returns to a portable JSON file (read-only)
    #[arg(long, value_name = "FILE", conflicts_with = "import")]
    pub export: Option<String>,
    /// Import a portable JSON file's returns into the local mailbox (idempotent)
    #[arg(long, value_name = "FILE", conflicts_with = "export")]
    pub import: Option<String>,
    /// Restrict an export to one owner reference
    #[arg(long)]
    pub owner: Option<String>,
    /// Declared origin-machine label recorded on exported envelopes
    #[arg(long)]
    pub machine: Option<String>,
    #[arg(long)]
    pub json: bool,
}

/// One portable replication document. `deny_unknown_fields` on this file and
/// on every envelope is the trust boundary: a foreign key (`posted_by_session`,
/// `session`, `holder`, `ownerRoot`, `path`, …) is refused, never ignored.
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplicateFile {
    version: u32,
    kind: String,
    exported_at: String,
    origin_machine: String,
    returns: Vec<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplicateEnvelope {
    logical_id: String,
    message_id: String,
    owner: String,
    work: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deliverable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    posted_at: String,
    origin_machine: String,
}

const MAX_REPLICATE_BYTES: u64 = 64 * 1024 * 1024;
const REPLICATE_KIND: &str = "edda.return.replicate";

/// Counts of one import, so a caller can tell an appended return from a skipped
/// duplicate and a refused id collision.
#[derive(Default, Debug)]
struct ImportCounts {
    appended: usize,
    skipped: usize,
    refused: usize,
    refused_details: Vec<String>,
}

/// The dedupe key. It deliberately excludes the local posting session, the
/// local salted message id and `postedAt`, so the same logical return posted
/// from two machines collapses to one record on either side.
fn logical_id(
    owner: &str,
    work: &str,
    status: &str,
    result: Option<&str>,
    deliverable: Option<&str>,
    message: Option<&str>,
) -> String {
    sha256_hex(
        format!(
            "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
            owner,
            work,
            status,
            result.unwrap_or(""),
            deliverable.unwrap_or(""),
            message.unwrap_or("")
        )
        .as_bytes(),
    )
}

fn record_logical_id(record: &MessageRecord) -> String {
    logical_id(
        &record.owner,
        &record.work,
        &record.status,
        record.result.as_deref(),
        record.deliverable.as_deref(),
        record.message.as_deref(),
    )
}

fn envelope_of(record: &MessageRecord, machine: &str) -> ReplicateEnvelope {
    ReplicateEnvelope {
        logical_id: record_logical_id(record),
        message_id: record.id.clone(),
        owner: record.owner.clone(),
        work: record.work.clone(),
        status: record.status.clone(),
        result: record.result.clone(),
        deliverable: record.deliverable.clone(),
        message: record.message.clone(),
        posted_at: record.posted_at.clone(),
        origin_machine: machine.to_owned(),
    }
}
/// Ids on the wire are lowercase sha256 hex only, the same shape the local
/// mailbox enforces, so an envelope can never name a path outside `messages/`.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_label(what: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control) {
        bail!("invalid {what}: expected 1..=200 characters without control characters");
    }
    Ok(())
}

/// `--machine`, else `EDDA_MACHINE`, else `HOSTNAME`, else `COMPUTERNAME`, else
/// `unknown`. Declared provenance is a label, never an identity claim.
fn machine_label(given: Option<&str>) -> Result<String> {
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let label = given
        .map(str::to_owned)
        .or_else(|| env("EDDA_MACHINE"))
        .or_else(|| env("HOSTNAME"))
        .or_else(|| env("COMPUTERNAME"))
        .unwrap_or_else(|| "unknown".into());
    validate_label("machine", &label)?;
    Ok(label)
}

/// Bounds and identity are checked before anything is written: a tampered or
/// foreign envelope must not become a local return.
fn validate_envelope(envelope: &ReplicateEnvelope) -> Result<()> {
    validate_label("owner", &envelope.owner)?;
    validate_label("work", &envelope.work)?;
    validate_label("originMachine", &envelope.origin_machine)?;
    if envelope.status != "done" && envelope.status != "failed" {
        bail!(
            "invalid status '{}': expected 'done' or 'failed'",
            envelope.status
        );
    }
    if !is_sha256_hex(&envelope.message_id) {
        bail!("invalid messageId: expected 64 lowercase hex characters");
    }
    if !is_sha256_hex(&envelope.logical_id) {
        bail!("invalid logicalId: expected 64 lowercase hex characters");
    }
    if envelope.posted_at.is_empty() || envelope.posted_at.len() > 64 {
        bail!("invalid postedAt: expected 1..=64 bytes");
    }
    let bounded = [
        ("result", envelope.result.as_deref(), 4_096_usize),
        ("deliverable", envelope.deliverable.as_deref(), 4_096),
        ("message", envelope.message.as_deref(), 1 << 20),
    ];
    for (what, value, max) in bounded {
        if let Some(value) = value {
            if value.len() > max {
                bail!("{what} exceeds {max} bytes");
            }
        }
    }
    let recomputed = logical_id(
        &envelope.owner,
        &envelope.work,
        &envelope.status,
        envelope.result.as_deref(),
        envelope.deliverable.as_deref(),
        envelope.message.as_deref(),
    );
    if recomputed != envelope.logical_id {
        bail!("logicalId does not match the envelope content");
    }
    Ok(())
}

pub(super) fn execute(dir: &Path, args: ReplicateArgs) -> Result<()> {
    match (args.export.as_deref(), args.import.as_deref()) {
        (Some(file), None) => replicate_export(dir, file, &args),
        (None, Some(file)) => {
            let counts = replicate_import(dir, file)?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": if counts.refused > 0 { "refused" } else { "imported" },
                        "file": file,
                        "appended": counts.appended,
                        "skipped": counts.skipped,
                        "refused": counts.refused,
                        "refusedDetails": counts.refused_details,
                    }))?
                );
            } else {
                println!(
                    "imported {file}: {} appended, {} skipped, {} refused",
                    counts.appended, counts.skipped, counts.refused
                );
            }
            if counts.refused > 0 {
                bail!(
                    "refused {} envelope(s): {}",
                    counts.refused,
                    counts.refused_details.join("; ")
                );
            }
            Ok(())
        }
        _ => bail!("pass exactly one of --export <FILE> or --import <FILE>"),
    }
}

/// Read-only with respect to the mailbox: it lists `messages/` and writes one
/// portable file. Sessions, run ids, registry/workspace paths, holder bindings,
/// claims, locks, processes, tokens and transcripts are never emitted.
fn replicate_export(dir: &Path, file: &str, args: &ReplicateArgs) -> Result<()> {
    let machine = machine_label(args.machine.as_deref())?;
    let mut messages = list_json::<MessageRecord>(&dir.join("messages"))?;
    messages.sort_by(|a, b| a.posted_at.cmp(&b.posted_at).then_with(|| a.id.cmp(&b.id)));
    let returns = messages
        .iter()
        .filter(|record| {
            args.owner
                .as_deref()
                .is_none_or(|owner| owner == record.owner)
        })
        .map(|record| serde_json::to_value(envelope_of(record, &machine)))
        .collect::<serde_json::Result<Vec<_>>>()?;
    let count = returns.len();
    let document = ReplicateFile {
        version: 1,
        kind: REPLICATE_KIND.into(),
        exported_at: now(),
        origin_machine: machine.clone(),
        returns,
    };
    let path = Path::new(file);
    let bytes = serde_json::to_vec_pretty(&document)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    if let Err(error) = write_temp(&temp, &bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("rename into {}", path.display()));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "exported",
                "file": file,
                "count": count,
                "originMachine": machine,
            }))?
        );
    } else {
        println!("exported {count} returns to {file}");
    }
    Ok(())
}

/// Fail closed: a foreign envelope, an unknown version/kind, a size over the
/// bound, or a broken logical identity aborts the whole file before any write.
/// Import creates only `messages/`; it never creates or writes `owners/` or
/// `claims/`, so an imported return shows up pending but stays unclaimable until
/// this machine binds the owner locally. The local exactly-once claim arbiter is
/// untouched.
fn replicate_import(dir: &Path, file: &str) -> Result<ImportCounts> {
    let metadata = fs::metadata(file).with_context(|| format!("read {file}"))?;
    if metadata.len() > MAX_REPLICATE_BYTES {
        bail!(
            "refusing {file}: {} bytes exceeds the {} byte import bound",
            metadata.len(),
            MAX_REPLICATE_BYTES
        );
    }
    let bytes = fs::read(file).with_context(|| format!("read {file}"))?;
    let document: ReplicateFile =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {file}"))?;
    if document.version != 1 {
        bail!(
            "unsupported replicate file version {}: expected 1",
            document.version
        );
    }
    if document.kind != REPLICATE_KIND {
        bail!(
            "unexpected replicate file kind '{}': expected {REPLICATE_KIND}",
            document.kind
        );
    }
    validate_label("originMachine", &document.origin_machine)?;
    let mut envelopes = Vec::new();
    let mut reasons = Vec::new();
    for (index, value) in document.returns.iter().enumerate() {
        match serde_json::from_value::<ReplicateEnvelope>(value.clone())
            .map_err(anyhow::Error::from)
            .and_then(|envelope| validate_envelope(&envelope).map(|()| envelope))
        {
            Ok(envelope) => envelopes.push(envelope),
            Err(error) => reasons.push(format!("returns[{index}]: {error}")),
        }
    }
    if !reasons.is_empty() {
        bail!(
            "refused {file}: {} invalid envelope(s), nothing written: {}",
            reasons.len(),
            reasons.join("; ")
        );
    }
    fs::create_dir_all(dir.join("messages"))
        .with_context(|| format!("create {}", dir.join("messages").display()))?;
    let _guard = lock(dir)?;
    // A local return counts as present whatever its claim state: the claim
    // marker is a separate axis from the return's identity.
    let mut seen: std::collections::HashSet<String> =
        list_json::<MessageRecord>(&dir.join("messages"))?
            .iter()
            .map(record_logical_id)
            .collect();
    let mut counts = ImportCounts::default();
    for envelope in &envelopes {
        if seen.contains(&envelope.logical_id) {
            counts.skipped += 1;
            continue;
        }
        if let Some(existing) = read_message(dir, &envelope.message_id)? {
            counts.refused += 1;
            counts.refused_details.push(format!(
                "messages/{}.json already holds logical identity {}, not {}",
                envelope.message_id,
                record_logical_id(&existing),
                envelope.logical_id
            ));
            continue;
        }
        let record = MessageRecord {
            version: 1,
            id: envelope.message_id.clone(),
            owner: envelope.owner.clone(),
            work: envelope.work.clone(),
            status: envelope.status.clone(),
            result: envelope.result.clone(),
            deliverable: envelope.deliverable.clone(),
            message: envelope.message.clone(),
            // No origin session is replicated: an imported return is never
            // attributable to the session that posted it elsewhere.
            posted_by_session: String::new(),
            posted_at: envelope.posted_at.clone(),
            origin: Some(MessageOrigin {
                logical_id: envelope.logical_id.clone(),
                message_id: envelope.message_id.clone(),
                machine: envelope.origin_machine.clone(),
                exported_at: document.exported_at.clone(),
            }),
        };
        write_atomic(&message_file(dir, &envelope.message_id), &record)?;
        seen.insert(envelope.logical_id.clone());
        counts.appended += 1;
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd_return::tests::{bind, claim, post, temp};

    fn replicate_args(
        export: Option<&Path>,
        import: Option<&Path>,
        owner: Option<&str>,
        machine: Option<&str>,
    ) -> ReplicateArgs {
        ReplicateArgs {
            export: export.map(|path| path.to_string_lossy().into_owned()),
            import: import.map(|path| path.to_string_lossy().into_owned()),
            owner: owner.map(str::to_owned),
            machine: machine.map(str::to_owned),
            json: true,
        }
    }

    fn export_registry(dir: &Path, file: &Path, machine: &str) -> Result<()> {
        super::execute(dir, replicate_args(Some(file), None, None, Some(machine)))
    }

    fn import_registry(dir: &Path, file: &Path) -> Result<ImportCounts> {
        super::replicate_import(dir, &file.to_string_lossy())
    }

    fn exported_document(dir: &Path, name: &str, machine: &str) -> serde_json::Value {
        let file = dir.join(name);
        export_registry(dir, &file, machine).unwrap();
        serde_json::from_slice(&fs::read(&file).unwrap()).unwrap()
    }

    fn write_document(path: &Path, document: &serde_json::Value) {
        fs::write(path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    }

    fn post_body(dir: &Path, owner: &str, work: &str, session: &str, body: &str) -> Result<()> {
        let body_path = dir.join("body.md");
        fs::write(&body_path, body).unwrap();
        super::post(
            dir,
            PostArgs {
                owner: owner.into(),
                work: work.into(),
                status: "done".into(),
                result: Some("ok".into()),
                deliverable: Some("out.md".into()),
                message_file: Some(body_path.to_string_lossy().into_owned()),
                session: session.into(),
                json: true,
            },
        )
    }

    fn message_count(dir: &Path) -> usize {
        list_json::<MessageRecord>(&dir.join("messages"))
            .unwrap()
            .len()
    }

    #[test]
    fn replicate_round_trips_between_two_registries() {
        let a = temp();
        let b = temp();
        bind(&a, "assistant/p", "sA", None).unwrap();
        post_body(&a, "assistant/p", "job-a", "controller-1", "body text").unwrap();
        let file = a.join("export.json");
        export_registry(&a, &file, "machine-a").unwrap();
        assert!(
            super::execute(&a, replicate_args(Some(&file), Some(&file), None, None)).is_err(),
            "exactly one direction is required"
        );
        let first = import_registry(&b, &file).unwrap();
        assert_eq!((first.appended, first.skipped, first.refused), (1, 0, 0));
        assert_eq!(message_count(&b), 1);
        let imported = list_json::<MessageRecord>(&b.join("messages")).unwrap();
        let imported = &imported[0];
        assert_eq!(imported.owner, "assistant/p");
        assert_eq!(imported.work, "job-a");
        assert_eq!(imported.status, "done");
        assert_eq!(imported.result.as_deref(), Some("ok"));
        assert_eq!(imported.deliverable.as_deref(), Some("out.md"));
        assert_eq!(imported.message.as_deref(), Some("body text"));
        assert_eq!(imported.posted_by_session, "", "no origin session leaks in");
        let origin = imported.origin.clone().expect("import records provenance");
        assert_eq!(origin.machine, "machine-a");
        assert_eq!(origin.message_id, imported.id);
        assert_eq!(
            origin.logical_id,
            logical_id(
                "assistant/p",
                "job-a",
                "done",
                Some("ok"),
                Some("out.md"),
                Some("body text")
            )
        );
        assert!(!origin.exported_at.is_empty());
        // Visible pending, but not claimable before this machine binds the owner.
        assert_eq!(pending_messages(&b, "assistant/p").unwrap().len(), 1);
        assert!(claim(&b, "assistant/p", "sB").is_err());
        // Re-import is idempotent.
        assert_eq!(import_registry(&b, &file).unwrap().skipped, 1);
        assert_eq!(message_count(&b), 1);
        // A claimed return is not duplicated by a later import either.
        bind(&b, "assistant/p", "sB", None).unwrap();
        claim(&b, "assistant/p", "sB").unwrap();
        assert!(pending_messages(&b, "assistant/p").unwrap().is_empty());
        assert_eq!(import_registry(&b, &file).unwrap().skipped, 1);
        assert_eq!(message_count(&b), 1);
        assert!(pending_messages(&b, "assistant/p").unwrap().is_empty());
    }

    #[test]
    fn replicate_import_is_idempotent_on_logical_identity_not_message_id() {
        let a = temp();
        let c = temp();
        let b = temp();
        bind(&a, "assistant/p", "sA", None).unwrap();
        post(&a, "assistant/p", "job-a", "controller-1").unwrap();
        bind(&c, "assistant/p", "sC", None).unwrap();
        post(&c, "assistant/p", "job-a", "controller-2").unwrap();
        let id_a = list_json::<MessageRecord>(&a.join("messages")).unwrap()[0]
            .id
            .clone();
        let id_c = list_json::<MessageRecord>(&c.join("messages")).unwrap()[0]
            .id
            .clone();
        assert_ne!(id_a, id_c, "different local sessions salt different ids");
        let file_a = a.join("export-a.json");
        let file_c = c.join("export-c.json");
        export_registry(&a, &file_a, "machine-a").unwrap();
        export_registry(&c, &file_c, "machine-c").unwrap();
        let first = import_registry(&b, &file_a).unwrap();
        assert_eq!((first.appended, first.skipped), (1, 0));
        let second = import_registry(&b, &file_c).unwrap();
        assert_eq!((second.appended, second.skipped), (0, 1));
        assert_eq!(message_count(&b), 1);
    }

    #[test]
    fn replicate_import_fails_closed_on_unknown_or_foreign_envelope() {
        let source = temp();
        let target = temp();
        bind(&source, "assistant/p", "s1", None).unwrap();
        post(&source, "assistant/p", "job-a", "controller-1").unwrap();
        let mut foreign = exported_document(&source, "foreign.json", "machine-a");
        foreign["returns"][0]["posted_by_session"] = serde_json::json!("controller-1");
        let foreign_file = target.join("foreign.json");
        write_document(&foreign_file, &foreign);
        assert!(import_registry(&target, &foreign_file).is_err());
        let mut session = exported_document(&source, "session.json", "machine-a");
        session["returns"][0]["session"] = serde_json::json!("controller-1");
        let session_file = target.join("session.json");
        write_document(&session_file, &session);
        assert!(import_registry(&target, &session_file).is_err());
        let mut version = exported_document(&source, "version.json", "machine-a");
        version["version"] = serde_json::json!(2);
        let version_file = target.join("version.json");
        write_document(&version_file, &version);
        assert!(import_registry(&target, &version_file).is_err());
        assert_eq!(message_count(&target), 0);
        assert!(!target.join("owners").exists());
        assert!(!target.join("claims").exists());
    }

    #[test]
    fn replicate_import_refuses_a_whole_file_with_a_bad_envelope() {
        let source = temp();
        let target = temp();
        bind(&source, "assistant/p", "s1", None).unwrap();
        post(&source, "assistant/p", "job-a", "controller-1").unwrap();
        post(&source, "assistant/p", "job-b", "controller-1").unwrap();
        let mut mixed = exported_document(&source, "mixed.json", "machine-a");
        assert_eq!(mixed["returns"].as_array().unwrap().len(), 2);
        mixed["returns"][1]["logicalId"] = serde_json::json!("a".repeat(64));
        let mixed_file = target.join("mixed.json");
        write_document(&mixed_file, &mixed);
        assert!(import_registry(&target, &mixed_file).is_err());
        assert_eq!(
            message_count(&target),
            0,
            "one bad envelope must leave the whole file unimported"
        );
        let valid = exported_document(&source, "valid.json", "machine-a");
        let valid_file = target.join("valid.json");
        write_document(&valid_file, &valid);
        let counts = import_registry(&target, &valid_file).unwrap();
        assert_eq!((counts.appended, counts.refused), (2, 0));
        assert_eq!(message_count(&target), 2);
    }

    #[test]
    fn replicate_export_never_writes_forbidden_fields() {
        let source = temp();
        let session = "session-do-not-replicate-777";
        bind(&source, "assistant/p", "s1", None).unwrap();
        post(&source, "assistant/p", "job-a", session).unwrap();
        let file = source.join("export.json");
        export_registry(&source, &file, "machine-a").unwrap();
        let text = fs::read_to_string(&file).unwrap();
        for forbidden in [
            session,
            "posted_by_session",
            "session",
            "holder",
            "ownerRoot",
            "registry",
            "claims",
        ] {
            assert!(
                !text.contains(forbidden),
                "export must not contain {forbidden}"
            );
        }
        let raw = source.to_string_lossy().into_owned();
        assert!(
            !text.contains(&raw),
            "export must not contain the mailbox path"
        );
        assert!(!text.contains(&raw.replace('\\', "\\\\")));
    }

    #[test]
    fn replicate_import_does_not_touch_holders_or_claims() {
        let source = temp();
        let target = temp();
        bind(&source, "assistant/p", "s1", None).unwrap();
        post(&source, "assistant/p", "job-a", "controller-1").unwrap();
        let file = source.join("export.json");
        export_registry(&source, &file, "machine-a").unwrap();
        import_registry(&target, &file).unwrap();
        assert_eq!(message_count(&target), 1);
        assert!(
            !target.join("owners").exists(),
            "import must not create an owner binding"
        );
        assert!(
            !target.join("claims").exists(),
            "import must not create claim markers"
        );
        assert_eq!(pending_messages(&target, "assistant/p").unwrap().len(), 1);
        assert!(claim(&target, "assistant/p", "s1").is_err());
    }
}
