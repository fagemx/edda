//! Cross-project decision sync engine.
//!
//! Pull-based: the target project pulls shared decisions from source projects'
//! ledgers and creates `decision_import` events with provenance links.
//!
//! Two carrier kinds:
//!
//! - SQLite ledgers of registered group members ([`sync_from_sources`]).
//! - Committed markdown mirrors (GH-671): a git-tracked `docs/decisions/`
//!   directory produced by `edda export md --out` on another machine
//!   ([`sync_from_mirror`]). Same #394 rule as the sqlite path: same key with
//!   a different value imports **inactive** — merge, never overwrite.
//!
//! This module only accepts pre-resolved data — callers (L4: cli, serve)
//! are responsible for resolving project IDs and source paths via `edda-store`.

use crate::sqlite_store::{DecisionRow, ImportParams};
use crate::Ledger;
use anyhow::Context;
use edda_core::decision::extract_domain;
use edda_core::event::finalize_event;
use edda_core::types::{Event, Provenance, Refs, SCHEMA_VERSION};
use std::path::{Path, PathBuf};

/// A source project to sync from.
pub struct SyncSource {
    pub project_id: String,
    pub project_name: String,
    pub ledger_path: std::path::PathBuf,
}

/// A single imported decision record.
#[derive(Debug, Clone)]
pub struct ImportedDecision {
    pub key: String,
    pub value: String,
    pub source_project: String,
    pub source_event_id: String,
}

/// A conflict detected during sync.
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    pub key: String,
    pub local_value: String,
    pub remote_value: String,
    pub source_project: String,
}

/// An error that occurred while syncing from a specific source.
#[derive(Debug, Clone)]
pub struct SourceError {
    pub project_name: String,
    pub error: String,
}

/// Result of a sync operation.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub imported: Vec<ImportedDecision>,
    pub skipped: usize,
    pub conflicts: Vec<ConflictInfo>,
    pub errors: Vec<SourceError>,
    /// Set only by [`sync_from_mirror`] (GH-671).
    pub mirror: Option<MirrorImportMeta>,
}

/// Sync shared decisions from source projects into the target ledger.
///
/// `target_project_id` is the pre-resolved project ID for the target ledger
/// (callers compute this via `edda_store::project_id`).
///
/// For each source project:
/// 1. Open the source ledger and query shared/global decisions
/// 2. Skip decisions already imported (by source_project_id + source_event_id)
/// 3. For new decisions: create `decision_import` event with provenance
/// 4. For conflicts (same key, different value): import as inactive
///
/// Source-level failures (cannot open ledger or query decisions) are collected
/// in `SyncResult::errors` rather than silently swallowed.
pub fn sync_from_sources(
    target: &Ledger,
    sources: &[SyncSource],
    target_project_id: &str,
    dry_run: bool,
) -> anyhow::Result<SyncResult> {
    let branch = target.head_branch()?;
    let mut result = SyncResult::default();

    for source in sources {
        // Don't sync from self
        if source.project_id == target_project_id {
            continue;
        }

        let source_ledger = match Ledger::open(&source.ledger_path) {
            Ok(l) => l,
            Err(e) => {
                result.errors.push(SourceError {
                    project_name: source.project_name.clone(),
                    error: format!("failed to open ledger: {e}"),
                });
                continue;
            }
        };

        // Use internal SqliteStore to get raw DecisionRow (sync needs scope,
        // source_project_id fields not exposed via DecisionView).
        let shared = match source_ledger.sqlite.shared_decisions() {
            Ok(d) => d,
            Err(e) => {
                result.errors.push(SourceError {
                    project_name: source.project_name.clone(),
                    error: format!("failed to query shared decisions: {e}"),
                });
                continue;
            }
        };

        for decision in &shared {
            // Skip if already imported
            if target.is_already_imported(&source.project_id, &decision.event_id)? {
                result.skipped += 1;
                continue;
            }

            // Any differing active value is a conflict, regardless of whether
            // the current winner originated locally or from another source.
            let current = target.sqlite.find_active_decision(&branch, &decision.key)?;
            let is_conflict = current
                .as_ref()
                .map(|active| active.value != decision.value)
                .unwrap_or(false);

            if is_conflict {
                result.conflicts.push(ConflictInfo {
                    key: decision.key.clone(),
                    local_value: current
                        .as_ref()
                        .map(|active| active.value.clone())
                        .unwrap_or_default(),
                    remote_value: decision.value.clone(),
                    source_project: source.project_name.clone(),
                });
            }

            if dry_run {
                result.imported.push(ImportedDecision {
                    key: decision.key.clone(),
                    value: decision.value.clone(),
                    source_project: source.project_name.clone(),
                    source_event_id: decision.event_id.clone(),
                });
                continue;
            }

            let imported = import_decision(
                target,
                &branch,
                &source.project_id,
                &source.project_name,
                decision,
                !is_conflict,
                // A sqlite source carries neither citations (they live in the
                // peer's event payload, which this path never opens) nor a
                // mirror stamp. Only the committed mirror does.
                ImportExtras::default(),
            )?;
            result.imported.push(imported);
        }
    }

    Ok(result)
}

/// Shared import tail for every carrier (sqlite source or committed mirror):
/// create the `decision_import` event, insert the row (#394: inactive when
/// `import_active` is false), and return the result entry.
fn import_decision(
    target: &Ledger,
    branch: &str,
    source_project_id: &str,
    source_project_name: &str,
    decision: &DecisionRow,
    import_active: bool,
    extras: ImportExtras<'_>,
) -> anyhow::Result<ImportedDecision> {
    let parent_hash = target.last_event_hash()?;

    let mut event = make_import_event(
        branch,
        parent_hash.as_deref(),
        decision,
        source_project_id,
        source_project_name,
        extras,
    )?;
    finalize_event(&mut event)?;

    let domain = extract_domain(&decision.key);
    target.insert_imported_decision(ImportParams {
        event: &event,
        key: &decision.key,
        value: &decision.value,
        reason: &decision.reason,
        domain: &domain,
        scope: &decision.scope,
        source_project_id,
        source_event_id: &decision.event_id,
        is_active: import_active,
        authority: &decision.authority,
        affected_paths: &decision.affected_paths,
        tags: &decision.tags,
        review_after: decision.review_after.as_deref(),
        reversibility: &decision.reversibility,
        village_id: decision.village_id.as_deref(),
    })?;

    Ok(ImportedDecision {
        key: decision.key.clone(),
        value: decision.value.clone(),
        source_project: source_project_name.to_string(),
        source_event_id: decision.event_id.clone(),
    })
}

// ── Committed markdown mirror (GH-671) ────────────────────────────────

/// Default staleness threshold, in hours, for a committed mirror's INDEX
/// stamp. Documented in `docs/guides/multi-agent.md` — change the doc when
/// this changes.
pub const DEFAULT_MIRROR_STALE_HOURS: i64 = 24;

/// A committed markdown mirror source: a git-tracked directory produced by
/// `edda export md --out <dir>` on another machine (GH-671).
///
/// The mirror is read as markdown — never as a second sqlite file.
pub struct MirrorSource {
    /// Directory containing `INDEX.md` and `decisions/`.
    pub mirror_dir: PathBuf,
}

/// Freshness of a mirror, derived from its `INDEX.md` stamp.
#[derive(Debug, Clone)]
pub struct MirrorFreshness {
    /// The `- **Exported at**:` value, if present.
    pub exported_at: Option<String>,
    /// The `- **Exporting machine**:` value, if present.
    pub machine: Option<String>,
    /// Age of the stamp in hours (negative on clock skew); `None` when the
    /// stamp is missing or unparseable.
    pub age_hours: Option<f64>,
    pub threshold_hours: i64,
}

impl MirrorFreshness {
    /// Stale = stamp older than the threshold, **or unreadable**. Unknown
    /// freshness must be visible (death visibility), never silently fresh.
    pub fn is_stale(&self) -> bool {
        match self.age_hours {
            Some(h) => h >= self.threshold_hours as f64,
            None => true,
        }
    }
}

/// Provenance of a mirror import, carried on [`SyncResult::mirror`].
#[derive(Debug, Clone)]
pub struct MirrorImportMeta {
    /// Stable dedup identity for imports from this mirror.
    pub source_id: String,
    /// Display name — the exporting machine from INDEX.md when present.
    pub source_name: String,
    pub freshness: MirrorFreshness,
}

/// Mirror index metadata — only what freshness needs.
struct MirrorIndexMeta {
    exported_at: Option<String>,
    machine: Option<String>,
}

/// A decision parsed out of a mirror file: the storage row it would import
/// as, plus the ratification fact the markdown carries.
struct MirrorDecision {
    row: DecisionRow,
    ratified_by: Option<String>,
    ratified_at: Option<String>,
    /// The citation chain (GH-761). Not a `DecisionRow` field: `cites` lives
    /// in the decision event payload, never in the projected row
    /// (`decision.cites=event-payload-not-sqlite-column`), so it rides the
    /// mirror separately and is written back into the import event.
    cites: Vec<String>,
}

/// Provenance an import carries beyond the projected row.
///
/// A sqlite-source import has neither; a mirror import has both, and both go
/// into the `decision_import` event payload rather than a new column — the
/// ledger is append-only and old ledgers stay readable.
#[derive(Default, Clone, Copy)]
struct ImportExtras<'a> {
    /// GH-761 citations, read back by `edda ratify --by-rule`.
    cites: &'a [String],
    /// GH-671 mirror stamp, read back by `edda ask` as the staleness signal.
    mirror: Option<&'a MirrorImportMeta>,
}

/// Import decisions from a committed markdown mirror (GH-671).
///
/// Machine B with an empty or different ledger checks out the repo, points
/// [`MirrorSource`] at `docs/decisions/`, and every active decision of the
/// source machine becomes visible locally:
///
/// - values, reasons, authority (original actor), scope, paths, tags and
///   governance metadata are carried **verbatim** — the carrier is
///   `ledger.cross-machine-projection=committed-mirror-stamped-at-wave-close-
///   quote-never-paraphrase`;
/// - same key with a different value imports **inactive** (#394) — merge,
///   never overwrite;
/// - a mirror decision whose original event already exists locally is
///   skipped (a machine importing its own mirror is a no-op);
/// - ratified decisions get the mirror's ratification replayed as an
///   append-only `decision_ratify` event so standard derivation sees it.
pub fn sync_from_mirror(
    target: &Ledger,
    source: &MirrorSource,
    dry_run: bool,
) -> anyhow::Result<SyncResult> {
    let mut result = SyncResult::default();

    // Freshness first: the stale signal must surface even if parsing later
    // fails, so read INDEX.md before anything else can bail.
    let index = read_mirror_index(&source.mirror_dir)?;
    let freshness = mirror_freshness(&index, DEFAULT_MIRROR_STALE_HOURS);
    let dir_name = source
        .mirror_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mirror")
        .to_string();
    let source_name = index.machine.clone().unwrap_or(dir_name);
    let source_id = format!("mirror:{source_name}");
    result.mirror = Some(MirrorImportMeta {
        source_id: source_id.clone(),
        source_name: source_name.clone(),
        freshness,
    });

    let decisions = parse_mirror(&source.mirror_dir)?;
    let branch = target.head_branch()?;

    for md in &decisions {
        // Locality / self-import guard: the original event already lives in
        // this ledger, so the row is either local or already mirrored 1:1.
        if target.get_event(&md.row.event_id)?.is_some() {
            result.skipped += 1;
            continue;
        }
        if target.is_already_imported(&source_id, &md.row.event_id)? {
            result.skipped += 1;
            continue;
        }

        // #394, same rule as sqlite sources: any differing active value is a
        // conflict and imports inactive — merge, never overwrite.
        let current = target.sqlite.find_active_decision(&branch, &md.row.key)?;
        let is_conflict = current
            .as_ref()
            .map(|active| active.value != md.row.value)
            .unwrap_or(false);

        if is_conflict {
            result.conflicts.push(ConflictInfo {
                key: md.row.key.clone(),
                local_value: current
                    .as_ref()
                    .map(|active| active.value.clone())
                    .unwrap_or_default(),
                remote_value: md.row.value.clone(),
                source_project: source_name.clone(),
            });
        }

        // Same ruling, already ratified here by someone other than a mirror:
        // there is nothing to learn from a peer's copy. Importing it would
        // supersede the row the operator ratified, and ratification binds to a
        // decision event, so the key would come out **unratified** — or, if the
        // mirror's own ratification were replayed over it, restated as
        // `mirror:<machine>`, silently losing whose act it was. Both are worse
        // than doing nothing. Asked before the insert, because afterwards the
        // active row is the one just written and the answer is always "no".
        if !is_conflict && ratified_locally(target, &branch, &md.row.key)? {
            result.skipped += 1;
            continue;
        }

        if dry_run {
            result.imported.push(ImportedDecision {
                key: md.row.key.clone(),
                value: md.row.value.clone(),
                source_project: source_name.clone(),
                source_event_id: md.row.event_id.clone(),
            });
            continue;
        }

        let imported = import_decision(
            target,
            &branch,
            &source_id,
            &source_name,
            &md.row,
            !is_conflict,
            ImportExtras {
                cites: &md.cites,
                mirror: result.mirror.as_ref(),
            },
        )?;

        // Preserve ratified/unratified: replay the mirror's ratification as
        // an append-only fact on this ledger. It binds the just-imported row
        // (the latest decision for the key), so `edda ask` and exports show
        // it ratified without any view-layer special case. Conflicts import
        // inactive and never carry ratification.
        //
        // Never over an operator's own ratification, though: derivation takes
        // the LATEST ratify per (branch, key), so replaying here would silently
        // restate a local operator act as `mirror:<machine>`. The direction is
        // conservative — it can only weaken attribution, never mint authority —
        // but a machine that has already ruled on a key locally does not need a
        // peer's copy of that fact, and must not lose whose ruling it was.
        if !is_conflict {
            if let Some(by) = &md.ratified_by {
                append_mirror_ratification(target, &branch, &md.row.key, by, &source_name)?;
            }
        }

        result.imported.push(imported);
    }

    Ok(result)
}

/// Whether this key already carries a ratification that did **not** come from
/// a mirror — i.e. one an operator issued on this machine.
fn ratified_locally(target: &Ledger, branch: &str, key: &str) -> anyhow::Result<bool> {
    let active = target.sqlite.find_active_decision(branch, key)?;
    let Some(row) = active else {
        return Ok(false);
    };
    Ok(target
        .ratified_decisions_map()?
        .get(&row.event_id)
        .is_some_and(|info| !info.ratified_by.starts_with("mirror:")))
}

/// Replay a mirror ratification as an append-only fact, attributed to the
/// **mirror** rather than to the name the markdown claimed.
///
/// The line this comes from is `- **Governance**: ratified by <who> at <ts>`
/// in a text file. Nothing about it is authenticated: no hash chain, no source
/// event hash, no operator present. Passing `<who>` through would mint a local
/// `decision_ratify` indistinguishable from one an operator actually issued
/// here — and `edda_core::event::new_decision_ratify_event` exists precisely
/// so that "operator authority is conferred by an event and can never be
/// self-declared on write". A file declaring itself ratified is a self-declared
/// write. It matters more since GH-671's SessionStart trigger: the import runs
/// unattended on every session, so the forged name would spread with nobody
/// looking.
///
/// So the ratification is preserved — doneWhen requires the ratified *state*
/// to survive the round trip — but recorded under the typed prefix
/// `mirror:<machine>` per `ratify.authority=typed-prefix`. A reader and
/// `edda ratify --by-rule` can both tell a replayed ratification from a local
/// operator act, which a free-text name can never be told apart from.
fn append_mirror_ratification(
    target: &Ledger,
    branch: &str,
    key: &str,
    ratified_by: &str,
    source_name: &str,
) -> anyhow::Result<()> {
    let parent_hash = target.last_event_hash()?;
    let note = format!("ratified by {ratified_by} on {source_name}; replayed from its committed mirror, not an operator act on this machine");
    let event = edda_core::event::new_decision_ratify_event(
        branch,
        parent_hash.as_deref(),
        key,
        &format!("mirror:{source_name}"),
        Some(&note),
    )?;
    target.append_event(&event)?;
    Ok(())
}

fn read_mirror_index(mirror_dir: &Path) -> anyhow::Result<MirrorIndexMeta> {
    let path = mirror_dir.join("INDEX.md");
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "read mirror index {} (run `edda export md --out <dir>` on the source machine)",
            path.display()
        )
    })?;
    Ok(parse_index_meta(&text))
}

/// Parse INDEX.md for freshness fields. Unknown lines (including hand-added
/// gloss lines) are ignored — a value is never minted from the index.
fn parse_index_meta(text: &str) -> MirrorIndexMeta {
    let mut meta = MirrorIndexMeta {
        exported_at: None,
        machine: None,
    };
    // Prefixes carry no trailing space, and every value is trimmed: the
    // SessionStart trigger reads the same stamp line to decide whether the
    // mirror moved (`edda_bridge_claude::mirror_import::read_stamp`). If one
    // reader demanded the space and the other did not, a hand-touched INDEX
    // would give the trigger a stamp while freshness read "unknown" — and
    // unknown freshness is treated as stale, so the mirror would import and
    // then warn about itself.
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("- **Exported at**:") {
            meta.exported_at = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("- **Exporting machine**:") {
            meta.machine = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        }
    }
    meta
}

fn mirror_freshness(meta: &MirrorIndexMeta, threshold_hours: i64) -> MirrorFreshness {
    let age_hours = meta.exported_at.as_deref().and_then(|ts| {
        let then =
            time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()?;
        let secs = (time::OffsetDateTime::now_utc() - then).whole_seconds();
        Some(secs as f64 / 3600.0)
    });
    MirrorFreshness {
        exported_at: meta.exported_at.clone(),
        machine: meta.machine.clone(),
        age_hours,
        threshold_hours,
    }
}

/// Parse every `decisions/*.md` file of a mirror into importable rows.
fn parse_mirror(mirror_dir: &Path) -> anyhow::Result<Vec<MirrorDecision>> {
    let decisions_dir = mirror_dir.join("decisions");
    if !decisions_dir.is_dir() {
        anyhow::bail!(
            "not a committed mirror (no decisions/ directory): {} — run `edda export md --out <dir>` on the source machine",
            decisions_dir.display()
        );
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&decisions_dir)
        .with_context(|| format!("read {}", decisions_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    files.sort();

    let mut out = Vec::new();
    for f in files {
        let stem = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let text = std::fs::read_to_string(&f)
            .with_context(|| format!("read mirror file {}", f.display()))?;
        out.extend(parse_domain_markdown(&stem, &text)?);
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MirrorEncoding {
    Raw,
    Escaped,
}

/// PR #1017 introduced escaping and the required Scope/Authority lines in
/// the same writer change. Those lines are therefore the existing format
/// discriminator for generated domain mirrors; a partial or mixed file is
/// ambiguous and must fail closed instead of selecting a lossy decoder.
fn detect_mirror_encoding(text: &str) -> anyhow::Result<MirrorEncoding> {
    let mut detected = None;
    let mut section = None;

    for line in text.lines() {
        if line.starts_with("## `") {
            if let Some((scope, authority)) = section.replace((false, false)) {
                record_section_encoding(scope, authority, &mut detected)?;
            }
        } else if let Some((scope, authority)) = section.as_mut() {
            *scope |= line.starts_with("- **Scope**: ");
            *authority |= line.starts_with("- **Authority**: ");
        }
    }
    if let Some((scope, authority)) = section {
        record_section_encoding(scope, authority, &mut detected)?;
    }

    Ok(detected.unwrap_or(MirrorEncoding::Raw))
}

fn record_section_encoding(
    has_scope: bool,
    has_authority: bool,
    detected: &mut Option<MirrorEncoding>,
) -> anyhow::Result<()> {
    let encoding = match (has_scope, has_authority) {
        (false, false) => MirrorEncoding::Raw,
        (true, true) => MirrorEncoding::Escaped,
        _ => anyhow::bail!(
            "ambiguous mirror encoding: decision must contain both Scope and Authority or neither"
        ),
    };
    if detected.is_some_and(|previous| previous != encoding) {
        anyhow::bail!("ambiguous mirror encoding: raw and escaped decision shapes are mixed");
    }
    *detected = Some(encoding);
    Ok(())
}

/// Parse one domain file of the mirror format (the exact shape
/// `edda export md` renders) into decisions. Missing optional lines fall
/// back to conservative defaults so pre-GH-671 mirrors still import.
fn parse_domain_markdown(file_domain: &str, text: &str) -> anyhow::Result<Vec<MirrorDecision>> {
    let encoding = detect_mirror_encoding(text)?;
    let mut out: Vec<MirrorDecision> = Vec::new();
    let mut header_domain: Option<String> = None;
    let mut current: Option<MirrorDecision> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# Domain: `") {
            header_domain = rest
                .strip_suffix('`')
                .map(|value| decode_mirror_field(value, encoding));
            continue;
        }
        if let Some(rest) = line.strip_prefix("## `") {
            if let Some(done) = current.take() {
                finish_mirror_decision(done, &mut out)?;
            }
            // Trim before decoding, never after: decoding first can produce a
            // trailing newline that `trim` would then eat.
            let key = decode_mirror_field(rest.strip_suffix('`').unwrap_or(rest).trim(), encoding);
            if key.is_empty() {
                continue;
            }
            let domain = header_domain
                .clone()
                .unwrap_or_else(|| file_domain.to_string());
            current = Some(MirrorDecision {
                row: DecisionRow {
                    event_id: String::new(),
                    key,
                    value: String::new(),
                    reason: String::new(),
                    domain,
                    branch: "main".to_string(),
                    supersedes_id: None,
                    is_active: true,
                    ts: None,
                    scope: "local".to_string(),
                    source_project_id: None,
                    source_event_id: None,
                    status: "active".to_string(),
                    authority: String::new(),
                    affected_paths: "[]".to_string(),
                    tags: "[]".to_string(),
                    review_after: None,
                    reversibility: "medium".to_string(),
                    village_id: None,
                },
                ratified_by: None,
                ratified_at: None,
                cites: Vec::new(),
            });
            continue;
        }
        let Some(decision) = current.as_mut() else {
            continue;
        };
        parse_mirror_field_line(line, decision, encoding);
    }
    if let Some(done) = current.take() {
        finish_mirror_decision(done, &mut out)?;
    }
    Ok(out)
}

/// Validate and collect a fully-parsed mirror decision.
fn finish_mirror_decision(
    mut decision: MirrorDecision,
    out: &mut Vec<MirrorDecision>,
) -> anyhow::Result<()> {
    if decision.row.authority.is_empty() {
        // No Authority line (pre-GH-671 mirror) and no unratified(...) gloss:
        // default to agent rather than over-claiming operator authorship.
        decision.row.authority = "agent".to_string();
    }
    if decision.row.event_id.is_empty() {
        anyhow::bail!(
            "mirror decision `{}` has no event_id line",
            decision.row.key
        );
    }
    if decision.row.value.is_empty() {
        anyhow::bail!(
            "mirror decision `{}` ({}) has no value",
            decision.row.key,
            decision.row.event_id
        );
    }
    out.push(decision);
    Ok(())
}

/// Match one `- **Field**: value` line inside a decision section.
/// Unrecognized lines (headers, prose, gloss) are ignored.
///
/// Every caller-supplied field is decoded here according to the domain file's
/// writer era: post-#1017 fields are unescaped, while raw-era bytes are kept
/// literal. Branch and ts are the exception on both sides: a branch name is
/// restricted to `[A-Za-z0-9._/-]` by
/// [`crate::validate_branch_name`] and a ts is a machine RFC3339 stamp, so
/// neither can carry an escape to undo.
fn parse_mirror_field_line(line: &str, decision: &mut MirrorDecision, encoding: MirrorEncoding) {
    let row = &mut decision.row;
    if let Some(v) = line.strip_prefix("- **Value**: `") {
        let v = v.strip_suffix('`').unwrap_or(v);
        row.value = decode_mirror_field(v, encoding);
    } else if let Some(v) = line.strip_prefix("- **Reason**: ") {
        row.reason = decode_mirror_field(v.trim_end(), encoding);
    } else if let Some(v) = line.strip_prefix("- **Branch/ts**: `") {
        if let Some((branch, ts)) = v.split_once("` · ") {
            row.branch = branch.trim().to_string();
            row.ts = Some(ts.trim().to_string());
        }
    } else if let Some(v) = line.strip_prefix("- **Governance**: ") {
        if let Some(rest) = v.strip_prefix("ratified by ") {
            if let Some((who, ts)) = rest.rsplit_once(" at ") {
                // The name is unauthenticated either way — `append_mirror_
                // ratification` records the mirror, not this string — but it
                // is quoted into that event's note, so it is carried exactly
                // rather than half-decoded.
                decision.ratified_by = Some(decode_mirror_field(who.trim(), encoding));
                decision.ratified_at = Some(ts.trim().to_string());
            }
        } else if let Some(rest) = v.strip_prefix("unratified (") {
            let auth = rest.strip_suffix(')').unwrap_or(rest).trim();
            if !auth.is_empty() {
                row.authority = decode_mirror_field(auth, encoding);
            }
        }
    } else if let Some(v) = line.strip_prefix("- **Scope**: ") {
        row.scope = decode_mirror_field(v.trim(), encoding);
    } else if let Some(v) = line.strip_prefix("- **Authority**: ") {
        row.authority = decode_mirror_field(v.trim(), encoding);
    } else if let Some(v) = line.strip_prefix("- **Affected paths**: ") {
        row.affected_paths = backtick_list_to_json(v, encoding);
    } else if let Some(v) = line.strip_prefix("- **Tags**: ") {
        row.tags = backtick_list_to_json(v, encoding);
    } else if let Some(v) = line.strip_prefix("- **Review after**: ") {
        row.review_after = Some(decode_mirror_field(v.trim(), encoding));
    } else if let Some(v) = line.strip_prefix("- **Reversibility**: ") {
        row.reversibility = decode_mirror_field(v.trim(), encoding);
    } else if let Some(v) = line.strip_prefix("- **Village**: ") {
        row.village_id = Some(decode_mirror_field(v.trim(), encoding));
    } else if let Some(v) = line.strip_prefix("- **Cites**: ") {
        // GH-761 citations ride the mirror as a backtick list, same encoding
        // as Tags and Affected paths. Dropping them would make the mirror lie
        // by omission about what authority a decision rests on.
        decision.cites = backtick_list(v, encoding);
    } else if let Some(v) = line.strip_prefix("- **event_id**: `") {
        let v = v.strip_suffix('`').unwrap_or(v);
        row.event_id = decode_mirror_field(v.trim(), encoding);
    }
}

/// `` `a`, `b` `` → `["a","b"]` as a JSON array string.
fn backtick_list_to_json(s: &str, encoding: MirrorEncoding) -> String {
    serde_json::to_string(&backtick_list(s, encoding)).unwrap_or_else(|_| "[]".to_string())
}

/// `` `a`, `b` `` → `vec!["a", "b"]`, decoded for its writer era.
///
/// The export escapes every item, backtick included (GH-1044:
/// `cmd_export::escape_field`), so a tag or a citation containing a
/// backslash, a newline, or a backtick — including the two-character
/// sequence `` `, `` that would otherwise read as this list's own item
/// separator — survives the single-line encoding instead of splitting the
/// list or truncating a value.
///
/// The list's own outer wrapping backticks are stripped exactly once, from
/// the *whole* string, before splitting (GH-1044) — never per fragment: a
/// fragment-local `strip_suffix('`')` cannot tell an escaped content
/// backtick's bare half and a genuine wrapping delimiter apart, so it would
/// sometimes strip the wrong one.
///
/// The split itself has to be escape-aware (GH-1044 Round 1 P0 —
/// `split_unescaped_backtick_comma` below). A plain `split("`, `")` matches
/// four literal bytes and never looks at what precedes them, so an item
/// whose value *ends* in exactly backtick, comma, space — escaped by the
/// writer to `` \`,  `` — still supplies the delimiter's opening half from
/// its own escaped content, one byte before the genuine wrapper backtick
/// that follows it. Because a plain split is leftmost and non-overlapping,
/// that false match consumes the real delimiter's opening half too,
/// corrupting both the item that ends there and the one after it.
///
/// Raw pre-#1017 mirrors use their historical plain split and preserve field
/// bytes. Escaped mirrors use the escape-aware split. Selecting the rule from
/// the domain-file shape prevents raw trailing backslashes from swallowing a
/// wrapper while retaining GH-1044's current-writer round trip.
fn backtick_list(s: &str, encoding: MirrorEncoding) -> Vec<String> {
    let inner = s.strip_prefix('`').unwrap_or(s);
    let inner = inner.strip_suffix('`').unwrap_or(inner);
    let parts = match encoding {
        MirrorEncoding::Raw => inner.split("`, `").collect(),
        MirrorEncoding::Escaped => split_unescaped_backtick_comma(inner),
    };
    parts
        .into_iter()
        .map(|p| decode_mirror_field(p.trim(), encoding))
        .filter(|p| !p.is_empty())
        .collect()
}

/// Split `inner` on the list's item delimiter `` `, ` ``, treating a
/// backtick as delimiter material only when it is not itself escaped
/// content (GH-1044 Round 1 P0).
///
/// Walks `inner` left to right the same way [`unescape_field`] does: a `\`
/// and whatever character follows it are consumed together as one unit and
/// can never begin a delimiter match. `escape_field` escapes every content
/// backtick, so the only backtick this scan ever reaches as a fresh,
/// unconsumed character is a genuine item wrapper — an escaped content
/// backtick is always the second half of a pair the backslash branch has
/// already stepped over. A fresh backtick not immediately followed by
/// `` , ` `` is left in place rather than treated as an error, the same
/// leniency `backtick_list` has always extended to malformed input.
///
/// This function is used only for [`MirrorEncoding::Escaped`]. Raw mirrors
/// retain the historical plain split in [`backtick_list`].
fn split_unescaped_backtick_comma(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut chars = inner.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next(); // escaped unit — cannot itself begin a delimiter
            continue;
        }
        if c == '`' && inner[i..].starts_with("`, `") {
            parts.push(&inner[start..i]);
            for _ in 0..3 {
                chars.next(); // ',', ' ', the delimiter's closing '`'
            }
            start = i + "`, `".len();
        }
    }
    parts.push(&inner[start..]);
    parts
}

fn decode_mirror_field(s: &str, encoding: MirrorEncoding) -> String {
    match encoding {
        MirrorEncoding::Raw => s.to_string(),
        MirrorEncoding::Escaped => unescape_field(s),
    }
}

/// Inverse of `edda-cli::cmd_export::escape_field` for escaped-era mirrors.
/// Raw-era fields bypass this function so literal `\\`, `\\n`, and `` \\` ``
/// sequences are preserved rather than reinterpreted as escapes.
fn unescape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some('`') => out.push('`'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn make_import_event(
    branch: &str,
    parent_hash: Option<&str>,
    decision: &crate::sqlite_store::DecisionRow,
    source_project_id: &str,
    source_project_name: &str,
    extras: ImportExtras<'_>,
) -> anyhow::Result<Event> {
    let affected_paths: serde_json::Value = serde_json::from_str(&decision.affected_paths)?;
    let decision_tags: serde_json::Value = serde_json::from_str(&decision.tags)?;
    let mut payload = serde_json::json!({
        "role": "system",
        "text": format!(
            "[sync] imported {key}={value} from {source}",
            key = decision.key,
            value = decision.value,
            source = source_project_name,
        ),
        "tags": ["decision", "decision_import"],
        "decision": {
            "key": decision.key,
            "value": decision.value,
            "reason": decision.reason,
            "scope": decision.scope,
            "authority": decision.authority,
            "affected_paths": affected_paths,
            "tags": decision_tags,
            "review_after": decision.review_after,
            "reversibility": decision.reversibility,
            "village_id": decision.village_id,
        },
        "source_project_id": source_project_id,
        "source_project_name": source_project_name,
        "source_event_id": decision.event_id,
    });

    // GH-761: `cites` is read back from `payload["decision"]["cites"]` by the
    // ratify sweep (`edda ratify --by-rule`), keyed on this event's id — which
    // is also the imported row's `event_id`. Emitted only when non-empty, so
    // an import that carries no citations is byte-identical to before.
    if !extras.cites.is_empty() {
        payload["decision"]["cites"] = serde_json::json!(extras.cites);
    }

    // GH-671: the mirror stamp the row arrived under. `edda ask` reads it back
    // as the staleness signal, so a decision imported from a dead mirror is
    // marked at the read end and not only at import time.
    if let Some(meta) = extras.mirror {
        payload["mirror"] = serde_json::json!({
            "machine": meta.source_name,
            "exported_at": meta.freshness.exported_at,
        });
    }

    let provenance = vec![Provenance {
        target: decision.event_id.clone(),
        rel: edda_core::types::rel::IMPORTED_FROM.to_string(),
        note: Some(format!("project:{source_project_name}")),
    }];

    let event = Event {
        event_id: format!("evt_{}", ulid::Ulid::new().to_string().to_lowercase()),
        ts: time_now_rfc3339(),
        event_type: "decision_import".to_string(),
        branch: branch.to_string(),
        parent_hash: parent_hash.map(|s| s.to_string()),
        hash: String::new(),
        payload,
        refs: Refs {
            blobs: Vec::new(),
            events: vec![decision.event_id.clone()],
            provenance,
        },
        schema_version: SCHEMA_VERSION,
        digests: Vec::new(),
        event_family: None,
        event_level: None,
    };

    Ok(event)
}

fn time_now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}
#[cfg(test)]
mod tests;
