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

/// The line a fleet read prints when it found nothing.
///
/// It exists because the obvious guard — `if empty && misses.is_empty()` — has
/// the logic backwards, and all three verbs shipped with it. Suppressing the
/// summary when something failed removes the sentence precisely when the reader
/// most needs it: with a miss printed and no summary, "the other projects had
/// nothing" and "the other projects were never looked at" produce identical
/// output, which is the silence this whole verb exists to break.
///
/// So the count is never `scope.len()`. It is what actually answered, and the
/// shortfall is stated rather than left for the reader to subtract.
///
/// `what` names the thing not found ("results", "tasks on the rail"); `tail`
/// carries the caller's qualifier (" for: {query}") and is placed before the
/// shortfall clause so the sentence still reads in order.
pub fn empty_summary(what: &str, tail: &str, scope_len: usize, misses: &[FleetMiss]) -> String {
    let answered = scope_len.saturating_sub(misses.len());
    if misses.is_empty() {
        format!("No {what} across {answered} project(s){tail}")
    } else {
        format!(
            "No {what} in the {answered} project(s) that answered{tail}; {} could not be read (above)",
            misses.len()
        )
    }
}

/// Ask the rest of the fleet before a local read reports absence (GH-407,
/// acceptance 4).
///
/// This is the point of the issue rather than a flourish on it. Every read verb
/// sees one workspace, so `No results` has always meant "not in this repo" while
/// reading as "nowhere" — and the knowledge is genuinely split across repos, so
/// the reading is routinely wrong. A bare miss may only stay bare once the fleet
/// has been asked and had nothing to add.
///
/// Returns `None` in exactly that case: the caller's own message is then true,
/// and the hint exists to correct a lie, not to decorate a fact.
///
/// The home project is skipped — it is the one that just missed, and probing it
/// again would report its own silence back as news. A project that cannot be
/// probed is named rather than counted as empty, for the same reason its
/// `--fleet` miss is: not looking is not the same as finding nothing.
///
/// The scope is injected rather than looked up so this stays testable without
/// the global registry, which is process-wide state.
pub fn elsewhere_hint<F>(
    scope: &[ProjectEntry],
    home_project_id: &str,
    what: &str,
    count_in: F,
) -> Option<String>
where
    F: Fn(&ProjectEntry) -> anyhow::Result<usize>,
{
    let mut found: Vec<String> = Vec::new();
    let mut unchecked: Vec<String> = Vec::new();

    for entry in scope {
        if entry.project_id == home_project_id {
            continue;
        }
        if !Path::new(&entry.path).join(".edda").is_dir() {
            continue; // absent repos are not news on a local miss
        }
        match count_in(entry) {
            Ok(0) => {}
            Ok(n) => found.push(format!("{n} {what}(s) in [{}]", entry.name)),
            Err(_) => unchecked.push(format!("[{}]", entry.name)),
        }
    }

    if found.is_empty() && unchecked.is_empty() {
        return None;
    }

    let mut parts = String::new();
    if !found.is_empty() {
        parts.push_str(&found.join(", "));
    }
    if !unchecked.is_empty() {
        if !parts.is_empty() {
            parts.push_str("; ");
        }
        parts.push_str(&format!("{} could not be checked", unchecked.join(", ")));
    }
    Some(format!("  {parts} — rerun with --fleet"))
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

    /// The point of the whole issue: a workspace that found nothing must not
    /// imply the fleet found nothing. Without this, "not here" and "nowhere"
    /// are the same sentence.
    #[test]
    fn a_local_miss_names_the_projects_that_do_have_hits() {
        let (a, b, c) = (live_repo(), live_repo(), live_repo());
        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("foundry", &b.path().to_string_lossy()),
            entry("dazun", &c.path().to_string_lossy()),
        ];

        let hint = elsewhere_hint(&scope, "pid-edda", "result", |e| {
            Ok(match e.name.as_str() {
                "foundry" => 3,
                _ => 1,
            })
        })
        .expect("hits elsewhere must be reported");

        assert!(hint.contains("3 result(s) in [foundry]"), "{hint}");
        assert!(hint.contains("1 result(s) in [dazun]"), "{hint}");
        assert!(hint.contains("--fleet"), "must say how to see them: {hint}");
    }

    /// The home project is the one that just missed. Probing it again would
    /// report its own silence back as news.
    #[test]
    fn the_workspace_that_already_missed_is_not_probed_again() {
        let (a, b) = (live_repo(), live_repo());
        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("foundry", &b.path().to_string_lossy()),
        ];

        let hint = elsewhere_hint(&scope, "pid-edda", "result", |e| {
            assert_ne!(e.name, "edda", "the home project must not be re-asked");
            Ok(2)
        })
        .expect("foundry has hits");

        assert!(hint.contains("[foundry]"), "{hint}");
        assert!(!hint.contains("[edda]"), "{hint}");
    }

    /// When the fleet really is empty, the bare message is true and must stay
    /// bare — the hint corrects a lie, it does not decorate a fact.
    #[test]
    fn nothing_is_added_when_the_fleet_has_nothing_either() {
        let (a, b) = (live_repo(), live_repo());
        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("foundry", &b.path().to_string_lossy()),
        ];

        assert_eq!(
            elsewhere_hint(&scope, "pid-edda", "result", |_| Ok(0)),
            None
        );
    }

    /// A project that exists but could not answer is not evidence of absence.
    #[test]
    fn a_project_that_could_not_be_probed_is_said_so_not_counted_as_empty() {
        let (a, b) = (live_repo(), live_repo());
        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("foundry", &b.path().to_string_lossy()),
        ];

        let hint = elsewhere_hint(&scope, "pid-edda", "result", |_| {
            anyhow::bail!("index not built")
        })
        .expect("an unprobed project must not render as an empty one");

        assert!(hint.contains("[foundry]"), "{hint}");
        assert!(hint.contains("could not be checked"), "{hint}");
    }

    /// A repo that is not on this machine cannot be probed and is not news: the
    /// operator knows what they have checked out, `--fleet` reports it anyway,
    /// and "rerun with --fleet" would not make it appear. Silence here is the
    /// one case that is not a lie.
    #[test]
    fn a_repo_absent_from_this_machine_is_left_out_of_the_hint() {
        let a = live_repo();
        let gone = live_repo();
        let gone_path = gone.path().to_string_lossy().into_owned();
        drop(gone);

        let scope = vec![
            entry("edda", &a.path().to_string_lossy()),
            entry("dazun", &gone_path),
        ];

        assert_eq!(
            elsewhere_hint(&scope, "pid-edda", "result", |_| panic!(
                "an absent repo must not be probed"
            )),
            None
        );
    }

    /// The line that had it backwards. A fleet read that found nothing must
    /// say what it *did* cover, and it may never count a project it never
    /// reached — otherwise "nothing there" and "did not look" render the same,
    /// which is the failure the whole verb exists to remove.
    #[test]
    fn an_empty_fleet_read_never_counts_projects_it_could_not_reach() {
        let plain = empty_summary("results", " for: q", 4, &[]);
        assert_eq!(plain, "No results across 4 project(s) for: q");

        let misses = vec![FleetMiss {
            project: "yushan".to_string(),
            reason: "index not built".to_string(),
        }];
        let partial = empty_summary("results", " for: q", 4, &misses);

        assert!(
            partial.contains("3 project(s) that answered"),
            "must report the 3 it actually covered: {partial}"
        );
        assert!(
            !partial.contains("across 4"),
            "must not claim all 4 were covered when 1 never answered: {partial}"
        );
        assert!(
            partial.contains("1 could not be read"),
            "the shortfall must be accounted for, not implied: {partial}"
        );
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
