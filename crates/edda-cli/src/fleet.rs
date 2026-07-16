//! How the CLI renders a fleet read (GH-407).
//!
//! The fan-out itself lives in `edda_store::fleet`: the SessionStart pack needs
//! it too (GH-408), and the CLI is a binary crate, so nothing else could ever
//! link it from here.
//!
//! What stays is the part that is genuinely the CLI's — the sentences. Every
//! `--fleet` verb reports misses the same way, spells "unavailable" the same
//! way, and counts an empty read the same way, because each of those diverged
//! once already and the copies disagreed.

pub use edda_store::fleet::{fan_out, group_by_project, FleetHit, FleetMiss};
use edda_store::registry::ProjectEntry;
use std::path::Path;

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
