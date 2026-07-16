//! The pack's Fleet section (GH-408).
//!
//! The push side of GH-407's fan-out. The read verbs answer when asked; this
//! answers when nobody asked, which is the only channel that reliably works —
//! the retrieval review found unprompted cross-repo queries never happen, so a
//! sibling's ruling that only a `--fleet` query would surface is, in practice, a
//! ruling nobody sees.
//!
//! Deliberately thin. It shows what a sibling *decided* and how much work is
//! *waiting* there, and nothing else: the pack's job is to make an agent
//! suspect there is something to look up, not to be the lookup.

use edda_ledger::tasks::TaskStatus;
use edda_ledger::Ledger;
use edda_store::fleet::{fan_out, FleetMiss};
use edda_store::registry::{fleet_scope, ProjectEntry};
use std::path::Path;

/// One sibling's contribution to the Fleet section.
#[derive(Debug, Clone, PartialEq)]
pub struct SiblingBrief {
    pub project: String,
    /// Recent shared/global rulings, newest first, already capped.
    pub decisions: Vec<String>,
    pub ready_tasks: usize,
}

/// How many rulings to carry per sibling before the budget even gets a say.
///
/// The pack is read by an agent deciding whether to look further, so the whole
/// section is a pointer, not a payload. Two lines per sibling is enough to make
/// it suspect; more of them would push the core sections out for no gain.
const DECISIONS_PER_SIBLING: usize = 2;

/// Read every sibling's ledger for what it decided and what it has waiting.
///
/// The home project is excluded: the pack already renders its own decisions and
/// its own rail, and repeating them under a "Fleet" heading would be noise
/// dressed as news.
///
/// The scope is injected rather than looked up so this is testable against
/// temporary workspaces — the registry is process-wide state.
pub fn collect_fleet_brief(
    scope: &[ProjectEntry],
    home_project_id: &str,
    read: impl Fn(&ProjectEntry) -> anyhow::Result<SiblingBrief>,
) -> (Vec<SiblingBrief>, Vec<FleetMiss>) {
    let siblings: Vec<ProjectEntry> = scope
        .iter()
        .filter(|e| e.project_id != home_project_id)
        .cloned()
        .collect();

    let (hits, misses) = fan_out(&siblings, |entry| Ok(vec![read(entry)?]));
    let briefs: Vec<SiblingBrief> = hits
        .into_iter()
        .map(|h| h.item)
        .filter(|b| !b.decisions.is_empty() || b.ready_tasks > 0)
        .collect();
    (briefs, misses)
}

/// Read one sibling's ledger: what it ruled, and what it has waiting.
///
/// Only `shared`/`global` rulings, because `local` ones are local by the
/// author's own declaration and forwarding them would overrule that. Only
/// active ones — a superseded ruling shown to a neighbour is worse than no
/// ruling, since it reads as current.
///
/// Read at query time from the sibling's own ledger rather than from a
/// sync-imported copy: fresher, and no second place for the truth to live.
fn read_sibling_brief(entry: &ProjectEntry) -> anyhow::Result<SiblingBrief> {
    let ledger = Ledger::open(Path::new(&entry.path))?;

    let mut shared: Vec<_> = ledger
        .shared_decisions()?
        .into_iter()
        .filter(|d| d.status == "active")
        .collect();
    shared.sort_by(|a, b| b.ts.cmp(&a.ts)); // newest first
    let decisions = shared
        .iter()
        .take(DECISIONS_PER_SIBLING)
        .map(|d| {
            let reason = d.reason.trim();
            if reason.is_empty() {
                format!("{}={}", d.key, d.value)
            } else {
                format!("{}={} — {}", d.key, d.value, first_clause(reason))
            }
        })
        .collect();

    let ready_tasks = ledger
        .task_views()?
        .iter()
        .filter(|t| t.status == TaskStatus::Ready)
        .count();

    Ok(SiblingBrief {
        project: entry.name.clone(),
        decisions,
        ready_tasks,
    })
}

/// The first sentence of a reason, capped.
///
/// Reasons in this ledger run to paragraphs. The pack wants the hook, not the
/// argument — whoever needs the argument has `edda ask`.
fn first_clause(reason: &str) -> String {
    const MAX: usize = 90;
    let head = reason
        .split(['\n', '。', ';'])
        .next()
        .unwrap_or(reason)
        .trim();
    if head.chars().count() <= MAX {
        return head.to_string();
    }
    let cut: String = head.chars().take(MAX).collect();
    format!("{cut}…")
}

/// The Fleet section for the project at `repo_root`, or nothing.
///
/// The one call site the pack needs; everything below it takes its scope as a
/// parameter so the tests never touch the registry.
pub fn fleet_section(repo_root: &Path, budget: usize) -> Option<String> {
    let scope = fleet_scope(repo_root);
    let home = edda_store::project_id(repo_root);
    let (briefs, misses) = collect_fleet_brief(&scope, &home, read_sibling_brief);
    render_fleet_section(&briefs, &misses, budget)
}

/// Render the Fleet section, or nothing at all.
///
/// `None` for a solo project, and `None` when every sibling is quiet: a heading
/// over an empty list is a claim that the fleet was checked and had nothing,
/// which is true but costs budget to say and nobody asked. An unreachable
/// sibling still gets its line — that one is not silence, it is a gap.
pub fn render_fleet_section(
    briefs: &[SiblingBrief],
    misses: &[FleetMiss],
    budget: usize,
) -> Option<String> {
    if briefs.is_empty() && misses.is_empty() {
        return None;
    }

    let mut out = String::from("## Fleet (sibling projects)\n");

    // Queue counts first: the shortest line any sibling can contribute, and the
    // one an operator scans for.
    for b in briefs {
        if b.ready_tasks > 0 {
            out.push_str(&format!(
                "- [{}] {} ready task(s)\n",
                b.project, b.ready_tasks
            ));
        }
    }

    // Then rulings by rank rather than by project — everyone's first before
    // anyone's second.
    //
    // Rendering project-by-project made the budget cut fall entirely on whoever
    // came last, and registry order is stable, so the same sibling would be
    // invisible every session with nothing but "(truncated)" to hint at it. A
    // budget may cost detail; it may not cost a whole project, or "C said
    // nothing" and "C was cut" become the same output — GH-407's silent-empty
    // failure, arriving through the budget instead of through a query.
    let deepest = briefs.iter().map(|b| b.decisions.len()).max().unwrap_or(0);
    for rank in 0..deepest {
        for b in briefs {
            if let Some(d) = b.decisions.get(rank) {
                out.push_str(&format!("- [{}] {d}\n", b.project));
            }
        }
    }
    // A sibling that could not be read is one short line, never an omission:
    // "did not look" must not render as "nothing there" (GH-407).
    for m in misses {
        out.push_str(&format!("- [{}] unavailable: {}\n", m.project, m.reason));
    }

    Some(truncate_on_line(&out, budget))
}

/// Cut to `budget` on a line boundary, saying so.
///
/// The section truncates before the core sections do, which is the whole reason
/// it is allowed to exist: it is the least important thing in the pack and must
/// be the first to give way.
fn truncate_on_line(s: &str, budget: usize) -> String {
    if s.len() <= budget {
        return s.to_string();
    }
    let marker = "- (fleet truncated by budget)\n";
    let room = budget.saturating_sub(marker.len());
    let mut end = 0;
    for (i, _) in s.char_indices().filter(|(i, c)| *c == '\n' && *i < room) {
        end = i + 1;
    }
    format!("{}{marker}", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str) -> ProjectEntry {
        ProjectEntry {
            project_id: format!("pid-{name}"),
            path: path.to_string(),
            name: name.to_string(),
            registered_at: "2026-07-16T00:00:00Z".to_string(),
            last_seen: "2026-07-16T00:00:00Z".to_string(),
            group: None,
        }
    }

    fn live_repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".edda")).unwrap();
        d
    }

    fn brief(project: &str, decisions: &[&str], ready: usize) -> SiblingBrief {
        SiblingBrief {
            project: project.to_string(),
            decisions: decisions.iter().map(|s| s.to_string()).collect(),
            ready_tasks: ready,
        }
    }

    /// The home project's own rulings are already in the pack. Repeating them
    /// under "Fleet" would be noise dressed as news.
    #[test]
    fn the_home_project_is_not_its_own_sibling() {
        let (a, b) = (live_repo(), live_repo());
        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("dazun", &b.path().to_string_lossy()),
        ];

        let (briefs, _) = collect_fleet_brief(&scope, "pid-edda", |e| {
            assert_ne!(e.name, "edda", "home must not be read as a sibling");
            Ok(brief(&e.name, &["db.engine=postgres"], 0))
        });

        assert_eq!(briefs.len(), 1);
        assert_eq!(briefs[0].project, "dazun");
    }

    /// A sibling with nothing decided and nothing waiting contributes no line.
    #[test]
    fn a_quiet_sibling_takes_up_no_room() {
        let (a, b) = (live_repo(), live_repo());
        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("dazun", &b.path().to_string_lossy()),
        ];

        let (briefs, misses) =
            collect_fleet_brief(&scope, "pid-edda", |e| Ok(brief(&e.name, &[], 0)));

        assert!(briefs.is_empty(), "nothing to say means no line");
        assert!(misses.is_empty());
        assert_eq!(render_fleet_section(&briefs, &misses, 4096), None);
    }

    /// Acceptance: no Fleet section when there are no siblings.
    #[test]
    fn a_solo_project_renders_no_section_at_all() {
        let a = live_repo();
        let scope = vec![entry("edda", &a.path().to_string_lossy())];

        let (briefs, misses) = collect_fleet_brief(&scope, "pid-edda", |_| {
            panic!("a solo project has nobody to ask")
        });

        assert_eq!(render_fleet_section(&briefs, &misses, 4096), None);
    }

    /// Acceptance: a sibling's ruling and its waiting work, both tagged.
    #[test]
    fn a_siblings_ruling_and_queue_are_shown_tagged_with_the_project() {
        let briefs = vec![brief("dazun", &["db.engine=postgres — need JSONB"], 5)];

        let out = render_fleet_section(&briefs, &[], 4096).expect("a sibling spoke");

        assert!(out.contains("[dazun] 5 ready task(s)"), "{out}");
        assert!(
            out.contains("[dazun] db.engine=postgres — need JSONB"),
            "{out}"
        );
    }

    /// A sibling that could not be read is a gap, not a silence — the same
    /// promise `--fleet` makes.
    #[test]
    fn an_unreachable_sibling_gets_a_line_rather_than_vanishing() {
        let here = live_repo();
        let gone = live_repo();
        let gone_path = gone.path().to_string_lossy().into_owned();
        drop(gone);

        let scope = vec![
            entry("edda", &here.path().to_string_lossy()),
            entry("dazun", &gone_path),
        ];

        let (briefs, misses) = collect_fleet_brief(&scope, "pid-edda", |_| Ok(brief("x", &[], 0)));

        assert_eq!(misses.len(), 1);
        let out = render_fleet_section(&briefs, &misses, 4096).expect("a gap is worth saying");
        assert!(out.contains("[dazun] unavailable:"), "{out}");
    }

    /// A budget may cost detail; it may not cost a whole project.
    ///
    /// Rendering sibling-by-sibling made the cut fall on whoever was last in
    /// registry order — and registry order is stable, so the same project would
    /// be invisible every session, with `(fleet truncated by budget)` the only
    /// hint and no way to tell "C said nothing" from "C was cut". That is the
    /// silent-empty failure of GH-407 arriving through the budget instead of
    /// through a query.
    #[test]
    fn a_budget_cut_costs_detail_but_never_a_whole_project() {
        let briefs = vec![
            brief("foundry", &["a.one=1 — first", "a.two=2 — second"], 0),
            brief("dazun", &["b.one=1 — first", "b.two=2 — second"], 0),
            brief("yushan", &["c.one=1 — first", "c.two=2 — second"], 0),
        ];

        // Room for the header and four-ish lines — not enough for all six.
        let out = render_fleet_section(&briefs, &[], 190).expect("three siblings spoke");
        assert!(
            out.contains("truncated"),
            "the budget must bite here:\n{out}"
        );

        // The invariant, stated positionally so it holds at any budget: every
        // project's first ruling comes before anyone's second, so whatever the
        // cut takes, it takes seconds.
        let first_second = out.find("=2 —").unwrap_or(out.len());
        for p in ["foundry", "dazun", "yushan"] {
            let at = out.find(&format!("[{p}]")).unwrap_or_else(|| {
                panic!("every project that spoke must be named, {p} is missing:\n{out}")
            });
            assert!(
                at < first_second,
                "{p} must be named before any project's second ruling:\n{out}"
            );
        }
    }

    /// The section is the least important thing in the pack, so it must be the
    /// first to give way — and it has to admit it was cut rather than end
    /// mid-thought as if that were all there was.
    #[test]
    fn the_section_truncates_within_budget_and_says_that_it_did() {
        let briefs: Vec<SiblingBrief> = (0..40)
            .map(|i| brief(&format!("proj{i}"), &["some.key=some-value — a reason"], 3))
            .collect();

        let out = render_fleet_section(&briefs, &[], 300).expect("plenty to say");

        assert!(out.len() <= 300, "over budget: {} bytes", out.len());
        assert!(
            out.contains("truncated by budget"),
            "must admit the cut: {out}"
        );
        assert!(out.ends_with('\n'), "cut on a line boundary: {out:?}");
    }
}
