//! One resolution authority for conductor plan state (GH-557).
//!
//! `edda conduct run` writes state under the directory it was launched in
//! (`<store>/.edda/conductor/<plan>/state.json`). The recovery verbs
//! (`status`/`retry`/`skip`/`abort`) run from a possibly different cwd and
//! used to resolve only `find_root(invocation cwd)`, so a plan launched from
//! a git worktree (or a plain subdirectory) answered `no state for plan` to
//! the very verbs `run` named.
//!
//! Every verb now resolves through this module:
//!
//! 1. [`record_registry`] — `run` records `<plan> -> <store>` in the
//!    workspace registry, so even a store that no worktree scan can reach
//!    (the plan YAML's own plain directory) stays discoverable;
//! 2. [`discover_plans`] — one discovery pass reads every candidate store's
//!    registry first, then scans each store's `.edda/conductor/` directory;
//! 3. [`resolve_plan_store`] — the single answer every mutating verb acts on.
//!
//! The candidate stores are the invocation root plus every git worktree of
//! the same repository (`git worktree list`), deduped by normalized identity
//! so Windows 8.3 short names / case / slash direction cannot list one
//! physical store twice.

use anyhow::{bail, Context, Result};
use edda_conductor::state::persist::load_state;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Normalize a store path for identity comparison.
///
/// Canonicalizes when possible and strips the Windows `\\?\` verbatim
/// prefix, unifies separators, and case-folds on Windows only — POSIX paths
/// are case- and separator-sensitive, so folding there would collapse
/// distinct stores into one.
pub(super) fn normalize_store_path(p: &Path) -> String {
    let canonical = std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().trim_start_matches(r"\\?\").to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string());
    if cfg!(windows) {
        canonical.replace('/', "\\").to_lowercase()
    } else {
        canonical
    }
}

/// Candidate stores for `repo_root`: the root itself, then every git
/// worktree of the same repository, deduped by [`normalize_store_path`].
///
/// Order is deterministic (root first, then `git worktree list` order) so
/// the verb resolution is reproducible. A failed enumeration is warned about
/// only when `repo_root` really is a git checkout — a non-git demo cwd is
/// not a fault.
pub(super) fn candidate_stores(repo_root: &Path) -> Vec<PathBuf> {
    let mut stores: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    push_unique(&mut stores, &mut seen, repo_root.to_path_buf());

    let out = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output();
    match out {
        Ok(out) if out.status.success() => {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(path) = line.strip_prefix("worktree ") {
                    push_unique(&mut stores, &mut seen, PathBuf::from(path));
                }
            }
        }
        Ok(out) if repo_root.join(".git").exists() => {
            eprintln!(
                "⚠ could not enumerate git worktrees ({}); searching the invocation root only",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        _ => {}
    }
    stores
}

fn push_unique(stores: &mut Vec<PathBuf>, seen: &mut Vec<String>, path: PathBuf) {
    let key = normalize_store_path(&path);
    if !seen.contains(&key) {
        seen.push(key);
        stores.push(path);
    }
}

/// Registry location: `<root>/.edda/conductor/.store-registry.json`.
pub(super) fn registry_path(root: &Path) -> PathBuf {
    root.join(".edda")
        .join("conductor")
        .join(".store-registry.json")
}

/// Read a store registry. A missing file is an empty map; a corrupt file is
/// an error — silently treating it as empty would let the next record
/// overwrite every other plan's entry.
pub(super) fn read_registry(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let path = registry_path(root);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading store registry {}", path.display()))?;
    let parsed: BTreeMap<String, String> = serde_json::from_str(&content)
        .with_context(|| format!("parsing store registry {}", path.display()))?;
    Ok(parsed
        .into_iter()
        .map(|(plan, store)| (plan, PathBuf::from(store)))
        .collect())
}

/// Record `<plan> -> <store>` under `root` (GH-557).
///
/// Best-effort and lock-protected: two concurrent `conduct run` processes
/// are the parallel-wave norm, so an unlocked read-modify-write could lose
/// entries. The lock is blocking. A corrupt existing registry is warned
/// about and skipped rather than destroyed.
pub(super) fn record_registry(root: &Path, plan: &str, store: &Path) {
    let path = registry_path(root);
    // `lock_file` and `write_atomic` each create the parent directory and
    // report their own failure, so no separate create is needed here.
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let lock = match edda_store::lock_file(&lock_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("⚠ store registry lock failed ({}): {e}", path.display());
            return;
        }
    };
    let mut map = match read_registry(root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("⚠ store registry unreadable, skipping record: {e}");
            return;
        }
    };
    map.insert(plan.to_string(), store.to_path_buf());
    match serde_json::to_string_pretty(&map) {
        Ok(data) => {
            if let Err(e) = edda_store::write_atomic(&path, data.as_bytes()) {
                eprintln!("⚠ store registry write failed ({}): {e}", path.display());
            }
        }
        Err(e) => eprintln!("⚠ store registry serialize failed: {e}"),
    }
    drop(lock);
}

/// Registry roots for a `run` launched with `run_cwd`, invoked from
/// `shell_cwd`: the shell's workspace root first (the lane the operator
/// stands in), then the run cwd's workspace root.
///
/// `find_root(run_cwd)` returns `run_cwd` itself once its `.edda` exists, so
/// a single-root choice would file every plan after the first where nothing
/// reads it. Recording into every candidate and reading from every scanned
/// store makes write and read meet.
pub(super) fn registry_roots_for(run_cwd: &Path, shell_cwd: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for r in [
        edda_ledger::EddaPaths::find_root(shell_cwd),
        edda_ledger::EddaPaths::find_root(run_cwd),
    ]
    .into_iter()
    .flatten()
    {
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    if roots.is_empty() {
        roots.push(run_cwd.to_path_buf());
    }
    roots
}

/// Plans found, as `(name, store)` — the shared discovery output type.
pub(super) type DiscoveredPlans = Vec<(String, PathBuf)>;
/// Corrupt-state diagnostics from a discovery pass: `(name, store, error)`.
pub(super) type DiscoveryCorruption = Vec<(String, PathBuf, anyhow::Error)>;

/// One discovery pass shared by `status` and the recovery verbs, so the two
/// surfaces can never disagree about which store holds a plan.
///
/// Registry-referenced states are collected first (before any directory
/// scan), so a live lane outranks a stale same-name `state.json` elsewhere.
/// Results are deduped by `(plan, normalized store)`.
pub(super) fn discover_plans(repo_root: &Path) -> (DiscoveredPlans, DiscoveryCorruption) {
    let mut found: DiscoveredPlans = Vec::new();
    let mut corrupt: DiscoveryCorruption = Vec::new();
    let stores = candidate_stores(repo_root);

    // PASS 1 — every store's registry.
    for store in &stores {
        let registry = match read_registry(store) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("⚠ store registry unreadable ({}): {e}", store.display());
                continue;
            }
        };
        for (name, referenced) in &registry {
            match load_state(referenced, name) {
                Ok(Some(_)) => mark_found(&mut found, name, referenced),
                Ok(None) => {}
                Err(e) => corrupt.push((
                    name.clone(),
                    referenced.clone(),
                    e.context(format!(
                        "plan \"{name}\" registry points to {}",
                        referenced.display()
                    )),
                )),
            }
        }
    }

    // PASS 2 — directory scan of every candidate store.
    for store in &stores {
        let conductor_dir = store.join(".edda").join("conductor");
        if !conductor_dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&conductor_dir) {
            Ok(rd) => rd,
            Err(e) => {
                eprintln!(
                    "⚠ could not read conductor dir {}: {e}",
                    conductor_dir.display()
                );
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                eprintln!("⚠ skipping unreadable entry in {}", conductor_dir.display());
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                eprintln!("⚠ skipping unreadable entry in {}", conductor_dir.display());
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if already_resolved(&found, &corrupt, store, &name) {
                continue;
            }
            match load_state(store, &name) {
                Ok(Some(_)) => mark_found(&mut found, &name, store),
                Ok(None) => {}
                Err(e) => corrupt.push((
                    name.clone(),
                    store.clone(),
                    e.context(format!(
                        "plan \"{name}\" state in {} is unreadable",
                        store.display()
                    )),
                )),
            }
        }
    }

    found.sort_by(|a, b| a.0.cmp(&b.0));
    (found, corrupt)
}

fn same_store(a: &Path, b: &Path) -> bool {
    normalize_store_path(a) == normalize_store_path(b)
}

fn mark_found(found: &mut DiscoveredPlans, name: &str, store: &Path) {
    if !found.iter().any(|(n, s)| n == name && same_store(s, store)) {
        found.push((name.to_string(), store.to_path_buf()));
    }
}

fn already_resolved(
    found: &DiscoveredPlans,
    corrupt: &DiscoveryCorruption,
    store: &Path,
    name: &str,
) -> bool {
    found.iter().any(|(n, s)| n == name && same_store(s, store))
        || corrupt
            .iter()
            .any(|(n, s, _)| n == name && same_store(s, store))
}

/// Every store currently holding `<plan>`'s `state.json`, in deterministic
/// order (invocation root first, then worktree order).
///
/// A corrupt state file is an error, not "absent" — swallowing it would
/// retarget a mutating verb onto a stale same-name state in another store.
fn stores_holding(repo_root: &Path, plan: &str) -> Result<Vec<PathBuf>> {
    edda_conductor::state::persist::validate_plan_name(plan)?;
    let (found, corrupt) = discover_plans(repo_root);
    let holding: Vec<PathBuf> = found
        .into_iter()
        .filter(|(name, _)| name == plan)
        .map(|(_, store)| store)
        .collect();
    if holding.is_empty() {
        if let Some((_, _, e)) = corrupt.iter().find(|(name, _, _)| name == plan) {
            return Err(anyhow::anyhow!("{e:#}"));
        }
    } else {
        // A healthy store outranks a corrupt same-name duplicate, but the
        // duplicate must not vanish silently: a destructive verb acting on
        // one store while another is unreadable is exactly the ambiguity
        // worth surfacing (GH-557 verifier report, gap 1).
        for (name, store, e) in &corrupt {
            if name == plan {
                eprintln!(
                    "⚠ plan \"{plan}\" state in {} is unreadable and shadowed: {e:#}",
                    store.display()
                );
            }
        }
    }
    Ok(holding)
}

/// The single store a verb should act on: the first store holding the plan.
///
/// When more than one store holds the same plan name, the others are
/// shadowed — warn on stderr so a destructive verb is never mute about it.
pub(super) fn resolve_plan_store(repo_root: &Path, plan: &str) -> Result<Option<PathBuf>> {
    let holding = stores_holding(repo_root, plan)?;
    match holding.len() {
        0 => Ok(None),
        1 => Ok(Some(holding[0].clone())),
        _ => {
            eprintln!(
                "⚠ plan \"{plan}\" exists in {} stores; acting on {} (shadowed: {})",
                holding.len(),
                holding[0].display(),
                holding[1..]
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(Some(holding[0].clone()))
        }
    }
}

/// Refuse to act when the plan's state is nowhere on the repo. The runner's
/// own blocked-phase message names retry/skip/abort, so those commands must
/// never answer "no state for plan" without pointing at the stores searched.
pub(super) fn no_state_error(repo_root: &Path, plan: &str) -> anyhow::Error {
    let searched: Vec<String> = candidate_stores(repo_root)
        .iter()
        .filter(|p| p.join(".edda").join("conductor").is_dir())
        .map(|p| p.display().to_string())
        .collect();
    let shown = if searched.is_empty() {
        "no store with a .edda/conductor directory".to_string()
    } else if searched.len() > 5 {
        format!("{} … ({} stores)", searched[..5].join(", "), searched.len())
    } else {
        searched.join(", ")
    };
    anyhow::anyhow!(
        "no state for plan \"{plan}\" (searched: {shown}); a plan's state lives in the store it was \
         launched from — the --cwd passed to conduct run, the plan's cwd: key, or the plan file's \
         own directory"
    )
}

/// Resolve the plan a bare recovery verb (no `--plan`) should act on.
///
/// Auto-detection scope is the whole discovery pass but refuses when more
/// than one store contributes a plan, so a destructive verb can never
/// silently reach into another lane. Callers pass `--plan` to disambiguate.
pub(super) fn resolve_plan_name(repo_root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    let (found, corrupt) = discover_plans(repo_root);
    for (name, store, e) in &corrupt {
        eprintln!(
            "⚠ plan \"{name}\" state in {} unreadable, excluded from auto-detection: {e:#}",
            store.display()
        );
    }

    let mut contributing: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for (name, store) in &found {
        if let Some(entry) = contributing.iter_mut().find(|(s, _)| same_store(s, store)) {
            if !entry.1.contains(name) {
                entry.1.push(name.clone());
            }
        } else {
            contributing.push((store.clone(), vec![name.clone()]));
        }
    }

    if contributing.is_empty() {
        if !corrupt.is_empty() {
            let shown = corrupt
                .iter()
                .map(|(name, store, _)| format!("{name} ({})", store.display()))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "plan state unreadable for: {shown}. Specify --plan <name>, or repair the state."
            );
        }
        bail!("No plans found. Specify --plan <name>.");
    }
    if contributing.len() > 1 {
        let shown = contributing
            .iter()
            .map(|(s, ns)| format!("{} ({})", normalize_store_path(s), ns.join("/")))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("Plans found in multiple stores: {shown}. Specify --plan <name>.");
    }
    let names = &contributing[0].1;
    match names.len() {
        0 => bail!("No plans found. Specify --plan <name>."),
        1 => Ok(names[0].clone()),
        _ => bail!(
            "Multiple plans found: {}. Use --plan to specify.",
            names.join(", ")
        ),
    }
}
