//! Fan-out for fleet reads (GH-407).
//!
//! Truth stays home: nothing is centralised. A fleet read visits each project in
//! scope, runs the same query against that project's own ledger or index, and
//! merges the answers — tagged with where each came from.
//!
//! The shape is shared, the query is not: `ask` reads decisions, `search` reads
//! a Tantivy index, `log` and `task list` read the ledger, and the SessionStart
//! pack reads siblings' rulings. So this owns the loop, the tagging, and the
//! failure accounting, and takes the per-project work as a closure — the same
//! inversion `edda-search-fts::sync` uses for its events.
//!
//! It lives beside `registry` rather than in the CLI because the CLI is a binary
//! crate: nothing else can link it. The read verbs were the first consumer, the
//! pack is the second (GH-408), and a second copy of this loop is exactly what
//! the shared home exists to prevent. Rendering stays with each consumer — this
//! is the mechanism, not the sentence.

use crate::registry::ProjectEntry;
use std::path::Path;

/// One project's contribution to a fleet read.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetHit<T> {
    /// The project's registered name, rendered as `[name]` beside the hit.
    pub project: String,
    pub item: T,
}

/// A project that could not be read.
///
/// These are results, not omissions. A fleet read that quietly skipped a repo
/// would answer "nothing there" when the truth is "did not look" — the exact
/// silent-empty failure GH-407 exists to remove, so every miss carries the
/// project it belongs to and the reason it failed.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetMiss {
    pub project: String,
    pub reason: String,
}

/// Run `query` against every project in scope, tagging hits and collecting
/// failures.
///
/// A project whose repo is absent from this machine is a miss, not an error and
/// not a silence: a fleet ledger is legitimately read on a machine that does not
/// have every repo checked out, and the reader still needs to know it was not
/// looked at. That case is detected here rather than in `query`, so no caller
/// has to remember to.
pub fn fan_out<T, F>(scope: &[ProjectEntry], query: F) -> (Vec<FleetHit<T>>, Vec<FleetMiss>)
where
    F: Fn(&ProjectEntry) -> anyhow::Result<Vec<T>>,
{
    let mut hits = Vec::new();
    let mut misses = Vec::new();

    for entry in scope {
        if !Path::new(&entry.path).join(".edda").is_dir() {
            misses.push(FleetMiss {
                project: entry.name.clone(),
                reason: format!("repo not on this machine ({})", entry.path),
            });
            continue;
        }
        match query(entry) {
            Ok(items) => hits.extend(items.into_iter().map(|item| FleetHit {
                project: entry.name.clone(),
                item,
            })),
            Err(e) => misses.push(FleetMiss {
                project: entry.name.clone(),
                reason: format!("{e}"),
            }),
        }
    }

    (hits, misses)
}

/// Collect a fan-out's hits back into per-project groups, preserving the order
/// projects were visited in.
///
/// Generic because nothing in it is verb-specific: it is `fan_out`'s output
/// reshaped, and every verb that fans out needs exactly this. It lives beside
/// `fan_out` so the next one does not grow a private copy.
pub fn group_by_project<T>(hits: &[FleetHit<T>]) -> Vec<(String, Vec<&T>)> {
    let mut out: Vec<(String, Vec<&T>)> = Vec::new();
    for hit in hits {
        match out.iter_mut().find(|(p, _)| *p == hit.project) {
            Some((_, items)) => items.push(&hit.item),
            None => out.push((hit.project.clone(), vec![&hit.item])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str) -> ProjectEntry {
        ProjectEntry {
            project_id: format!("pid-{name}"),
            path: path.to_string(),
            name: name.to_string(),
            registered_at: "2026-07-15T00:00:00Z".to_string(),
            last_seen: "2026-07-15T00:00:00Z".to_string(),
            group: None,
        }
    }

    fn live_repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".edda")).unwrap();
        d
    }

    #[test]
    fn hits_carry_the_project_they_came_from() {
        let a = live_repo();
        let b = live_repo();
        let scope = vec![
            entry("foundry", &a.path().to_string_lossy()),
            entry("edda", &b.path().to_string_lossy()),
        ];

        let (hits, misses) = fan_out(&scope, |e| Ok(vec![format!("hit-in-{}", e.name)]));

        assert!(misses.is_empty());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].project, "foundry");
        assert_eq!(hits[0].item, "hit-in-foundry");
        assert_eq!(hits[1].project, "edda");
    }

    /// The core promise: a repo that is not here is reported, never skipped.
    #[test]
    fn an_absent_repo_is_a_reported_miss_not_a_silence() {
        let here = live_repo();
        let gone = live_repo();
        let gone_path = gone.path().to_string_lossy().into_owned();
        drop(gone);

        let scope = vec![
            entry("foundry", &here.path().to_string_lossy()),
            entry("dazun", &gone_path),
        ];

        let (hits, misses) = fan_out(&scope, |_| Ok(vec!["x"]));

        assert_eq!(hits.len(), 1, "the present repo still answers");
        assert_eq!(misses.len(), 1, "the absent one is accounted for");
        assert_eq!(misses[0].project, "dazun");
        assert!(
            misses[0].reason.contains("not on this machine"),
            "the reason must say why: {}",
            misses[0].reason
        );
    }

    /// One project failing must not take the others down with it.
    #[test]
    fn a_failing_project_is_a_miss_and_the_rest_still_answer() {
        let a = live_repo();
        let b = live_repo();
        let scope = vec![
            entry("foundry", &a.path().to_string_lossy()),
            entry("dazun", &b.path().to_string_lossy()),
        ];

        let (hits, misses) = fan_out(&scope, |e| {
            if e.name == "dazun" {
                anyhow::bail!("index not built")
            } else {
                Ok(vec!["found"])
            }
        });

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].project, "foundry");
        assert_eq!(misses.len(), 1);
        assert_eq!(misses[0].project, "dazun");
        assert_eq!(misses[0].reason, "index not built");
    }

    #[test]
    fn an_empty_scope_yields_nothing_rather_than_erroring() {
        let (hits, misses) = fan_out(&[], |_| Ok(vec!["never"]));
        assert!(hits.is_empty());
        assert!(misses.is_empty());
    }

    /// Grouping preserves visit order, so a fleet read renders in registry
    /// order rather than whatever a hash map felt like.
    #[test]
    fn grouping_keeps_each_project_together_in_the_order_visited() {
        let hits = vec![
            FleetHit {
                project: "edda".to_string(),
                item: "a",
            },
            FleetHit {
                project: "dazun".to_string(),
                item: "b",
            },
            FleetHit {
                project: "edda".to_string(),
                item: "c",
            },
        ];

        let grouped = group_by_project(&hits);

        assert_eq!(grouped.len(), 2, "two projects, not three rows");
        assert_eq!(grouped[0].0, "edda");
        assert_eq!(grouped[0].1, vec![&"a", &"c"], "non-adjacent hits regroup");
        assert_eq!(grouped[1].0, "dazun");
        assert_eq!(grouped[1].1, vec![&"b"]);
    }
}
