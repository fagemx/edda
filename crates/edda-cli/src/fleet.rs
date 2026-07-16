//! Fan-out for fleet reads (GH-407).
//!
//! Truth stays home: nothing is centralised. A `--fleet` read visits each
//! project in scope, runs the same query against that project's own ledger or
//! index, and merges the answers — tagged with where each came from.
//!
//! The shape is shared, the query is not: `ask` reads decisions, `search` reads
//! a Tantivy index, `log` and `task list` read the ledger. So this owns the loop,
//! the tagging, and the failure accounting, and takes the per-project work as a
//! closure — the same inversion `edda-search-fts::sync` uses for its events.

use edda_store::registry::ProjectEntry;
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
/// `fan_out` so the next one does not grow a third private copy.
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

/// Report the projects that did not answer, in the one form every fleet verb
/// uses.
///
/// No prefix: `fan_out`'s reasons are already complete phrases, and a fixed
/// prefix would contradict half of them — the most common miss is "repo not on
/// this machine", which is absent, not unreadable.
pub fn print_misses(misses: &[FleetMiss]) {
    for miss in misses {
        println!("  [{}] {}", miss.project, miss.reason);
    }
}

/// The `--json` body of a fleet read: what answered, and what did not.
///
/// The keys live here rather than at each call site because they are the part
/// that actually diverged — `ask` said `unreadable` while `task` said
/// `unavailable` for the identical array. A helper that returned only the
/// values would leave the next verb free to invent a third spelling, which is
/// the divergence this is meant to end.
///
/// Having one spelling matters more than which spelling won. `unavailable` is
/// the word that covers both misses `fan_out` actually produces — absent *and*
/// errored; `unreadable` only covers the second, and would contradict the
/// commonest reason of all, "repo not on this machine".
///
/// `projects` is pre-built by the caller: every verb tags its rows the same
/// way, but what hangs off each project is the verb's own business.
pub fn json_envelope(projects: Vec<serde_json::Value>, misses: &[FleetMiss]) -> serde_json::Value {
    serde_json::json!({
        "projects": projects,
        "unavailable": misses
            .iter()
            .map(|m| serde_json::json!({ "project": m.project, "reason": m.reason }))
            .collect::<Vec<_>>(),
    })
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

    /// Pins the key names themselves. They are the whole point of the helper —
    /// a verb free to spell this `unreadable` is the divergence that made it
    /// necessary — so a rename must not be able to pass quietly.
    #[test]
    fn the_json_envelope_names_what_answered_and_what_did_not() {
        let misses = vec![FleetMiss {
            project: "dazun".to_string(),
            reason: "repo not on this machine (D:\\gone)".to_string(),
        }];

        let env = json_envelope(vec![serde_json::json!({ "project": "edda" })], &misses);

        assert_eq!(env["projects"][0]["project"], "edda");
        assert_eq!(env["unavailable"][0]["project"], "dazun");
        // Verbatim: `fan_out`'s reasons are complete phrases, and the absent
        // repo is the case a fixed "unreadable:" prefix used to contradict.
        assert_eq!(
            env["unavailable"][0]["reason"],
            "repo not on this machine (D:\\gone)"
        );
        assert!(
            env.get("unreadable").is_none() && env.get("fleet").is_none(),
            "the retired spellings must not come back: {env}"
        );
    }
}
