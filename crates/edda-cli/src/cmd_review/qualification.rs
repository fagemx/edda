//! R22 engine × surface qualification for the review brief.
//!
//! `REVIEW.md` §6.1 makes a reviewer a checklist-type engine *unless the brief
//! names it qualified for this class*, and an unqualified engine must escalate
//! the spec's only `[判斷]` item (`D5`) rather than close it — which §6.4 turns
//! into a provisional LGTM that §8 says never satisfies the merge gate. A brief
//! that carries no qualification therefore disqualifies every round on its own
//! silence (GH-999).
//!
//! The table this module states is **rule R22 of `docs/fleet/rules.md`**, which
//! is operator-maintained and remains the source; the copy here is mirrored
//! into code so the brief can carry it, and `r22_*` tests below pin the copy.

use anyhow::Result;
use edda_core::model_id::canonical_model_id;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// R22's four surfaces, strictest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    /// 審判面 — what decides reviews.
    Review,
    /// 閘門面 — CI, hooks and lane gates.
    Gate,
    /// 出貨面 — what ships to users.
    Shipping,
    /// 內部工具面 — R22's default for every other path.
    InternalTool,
}

impl Surface {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Gate => "gate",
            Self::Shipping => "shipping",
            Self::InternalTool => "internal-tool",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Review => "review (審判面)",
            Self::Gate => "gate (閘門面)",
            Self::Shipping => "shipping (出貨面)",
            Self::InternalTool => "internal-tool (內部工具面)",
        }
    }
}

/// R22 surfaces by path, listed strictest first. One changed path on a surface
/// puts the whole PR on it, and this order is R22's precedence
/// (`review > gate > shipping > internal-tool`) for a diff matching several.
const SURFACE_PATHS: &[(Surface, &[&str])] = &[
    (
        Surface::Review,
        &[
            "REVIEW.md",
            "docs/fleet/rules.md",
            "tests/canaries/**",
            "scripts/review-pr.sh",
            "scripts/pr-review-watch.sh",
            "scripts/review-l0.sh",
            "scripts/reviewer-capabilities.sh",
            "scripts/fleet/reviewer-capabilities.ps1",
            "scripts/fleet/daily-digest.sh",
        ],
    ),
    (
        Surface::Gate,
        &[
            ".github/**",
            "lefthook.yml",
            "scripts/lint-*.sh",
            "scripts/fleet/lane-launch.ps1",
            "scripts/fleet-claim-issue.sh",
        ],
    ),
    (
        Surface::Shipping,
        &[
            "crates/**",
            "Cargo.toml",
            "Cargo.lock",
            "install.sh",
            "rust-toolchain*",
            "clippy.toml",
        ],
    ),
];

/// R22 also puts *any* script that posts a status or performs a merge on the
/// review surface. That clause reads script contents, not paths, so the
/// classifier cannot decide it; the brief states it so the engine can.
const UNENUMERATED_REVIEW_SCRIPTS: &str = "R22 also puts any script that posts a status or performs a merge on the review surface. That clause is about what a script does, not where it lives, so the surface above is derived from R22's enumerated paths only — if this diff adds or changes such a script, say so in your review.";

fn matcher(patterns: &[&str]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        // literal_separator keeps `scripts/lint-*.sh` from reaching into
        // nested directories; `**` stays explicit.
        builder.add(GlobBuilder::new(pattern).literal_separator(true).build()?);
    }
    Ok(builder.build()?)
}

/// R22's engine table. Opus is authoritative only through Claude Code (R27);
/// glm-5.3-flash is authoritative on the internal-tool surface and SHADOW
/// elsewhere; any engine the table does not name is a checklist-type engine
/// everywhere, so an unknown or unnamed model is never authoritative.
fn is_authoritative(engine: &str, transport: &str, surface: Surface) -> bool {
    match engine {
        "claude-opus-5" => transport == "claude-code",
        "gpt-5.6-sol" => true,
        "glm-5.3-flash" => surface == Surface::InternalTool,
        _ => false,
    }
}

/// The engines R22 makes authoritative for `surface`. This is the authoritative
/// engine pool the qualification table defines: an Opus-authored PR needs a
/// non-author member of it to be reviewable (#926, absorbed into GH-999).
fn authoritative_pool(surface: Surface) -> Vec<&'static str> {
    let mut pool = vec!["claude-opus-5 (only via Claude Code)", "gpt-5.6-sol"];
    if surface == Surface::InternalTool {
        pool.push("glm-5.3-flash");
    }
    pool
}

/// What the brief tells the engine about its own authority, recorded so the
/// receipt can be read back.
pub(crate) struct Qualification {
    pub surface: Surface,
    /// The changed path that put the PR on `surface`. `None` for the
    /// internal-tool default, which no path decides.
    pub deciding_path: Option<String>,
    /// Canonical requested model id, or the raw request when it names no known
    /// model family.
    pub engine: String,
    /// How the engine is reached; R22 grants Opus authority only via Claude Code.
    pub transport: String,
    pub authoritative: bool,
    pub require_model_diversity: bool,
}

pub(crate) fn assess(
    files: &[String],
    model_requested: &str,
    transport: &str,
    require_model_diversity: bool,
) -> Result<Qualification> {
    let (surface, deciding_path) = classify(files)?;
    let engine =
        canonical_model_id(model_requested).unwrap_or_else(|| model_requested.trim().to_owned());
    Ok(Qualification {
        authoritative: is_authoritative(&engine, transport, surface),
        surface,
        deciding_path,
        engine,
        transport: transport.to_owned(),
        require_model_diversity,
    })
}

fn classify(files: &[String]) -> Result<(Surface, Option<String>)> {
    for (surface, patterns) in SURFACE_PATHS {
        let set = matcher(patterns)?;
        if let Some(path) = files.iter().find(|file| set.is_match(file.as_str())) {
            return Ok((*surface, Some(path.clone())));
        }
    }
    Ok((Surface::InternalTool, None))
}

/// Emitted verbatim into the brief; the reviewer's behaviour is driven by this
/// text, so it is the contract REVIEW.md §6.1 asks the brief to carry.
pub(crate) const SECTION_HEADING: &str = "## ENGINE QUALIFICATION (R22)";

impl Qualification {
    pub(crate) fn authority(&self) -> &'static str {
        if self.authoritative {
            "authoritative"
        } else {
            "not-authoritative"
        }
    }

    pub(crate) fn brief_section(&self) -> String {
        let decided_by = self.deciding_path.as_ref().map_or_else(
            || "no changed path is on a stricter surface".to_owned(),
            |path| format!("decided by changed path `{path}`"),
        );
        let verdict = if self.authoritative {
            "AUTHORITATIVE for this surface — close D5 yourself; do not escalate it."
        } else {
            "NOT authoritative for this surface — escalate D5 as REVIEW.md §6.1 requires."
        };
        let diversity = if self.require_model_diversity {
            "`--require-model-diversity` was passed, so this round's independence policy is `model`: a reviewer that cannot be shown to differ from the author's model disqualifies the round. This section does not change that flag."
        } else {
            "`--require-model-diversity` was not passed, so this round's independence policy is `session`: an author/reviewer pair on the same model is recorded in the receipt as `independence: same-model` and stays visible, but does not disqualify the round. This section does not change that flag."
        };
        format!(
            "{SECTION_HEADING} — host-computed policy, not data\n\
             R22 surface: {} — {decided_by}.\n\
             Running engine: {} (transport {}).\n\
             Authoritative engines for this surface (R22 engine table): {}.\n\
             VERDICT: {verdict}\n\
             An engine this table does not name is a checklist-type engine per REVIEW.md §6.1, and an unknown engine is never authoritative.\n\
             {UNENUMERATED_REVIEW_SCRIPTS}\n\
             Model diversity: {diversity}\n",
            self.surface.label(),
            self.engine,
            self.transport,
            authoritative_pool(self.surface).join(", "),
        )
    }
}

/// Add the brief's qualification to a receipt without renaming or dropping an
/// existing key, so `--json` consumers and the ledger read back what the engine
/// was told.
pub(crate) fn augment(value: &mut serde_json::Value, qualification: &Qualification) {
    value["engine_qualification"] = serde_json::json!({
        "surface": qualification.surface.as_str(),
        "deciding_path": qualification.deciding_path,
        "engine": qualification.engine,
        "authority": qualification.authority(),
        "authoritative_engines": authoritative_pool(qualification.surface),
        "require_model_diversity": qualification.require_model_diversity,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assessed(files: &[&str], model: &str, transport: &str) -> Qualification {
        let files = files.iter().map(|f| (*f).to_owned()).collect::<Vec<_>>();
        assess(&files, model, transport, false).unwrap()
    }

    #[test]
    fn r22_surface_table_classifies_every_listed_path() {
        for (expected, paths) in [
            (
                Surface::Review,
                &[
                    "REVIEW.md",
                    "docs/fleet/rules.md",
                    "tests/canaries/dispatch.sh",
                    "scripts/review-pr.sh",
                    "scripts/pr-review-watch.sh",
                    "scripts/review-l0.sh",
                    "scripts/reviewer-capabilities.sh",
                    "scripts/fleet/reviewer-capabilities.ps1",
                    "scripts/fleet/daily-digest.sh",
                ][..],
            ),
            (
                Surface::Gate,
                &[
                    ".github/workflows/ci.yml",
                    "lefthook.yml",
                    "scripts/lint-file-length.sh",
                    "scripts/fleet/lane-launch.ps1",
                    "scripts/fleet-claim-issue.sh",
                ][..],
            ),
            (
                Surface::Shipping,
                &[
                    "crates/edda-core/src/lib.rs",
                    "Cargo.toml",
                    "Cargo.lock",
                    "install.sh",
                    "rust-toolchain.toml",
                    "clippy.toml",
                ][..],
            ),
            (
                Surface::InternalTool,
                &[
                    "scripts/fleet/collision-scan.sh",
                    "scripts/test-fleet.sh",
                    "docs/reference/cli.md",
                    ".claude/CLAUDE.md",
                ][..],
            ),
        ] {
            for path in paths {
                let files = [(*path).to_owned()];
                let (surface, deciding) = classify(&files).unwrap();
                assert_eq!(surface, expected, "{path}");
                assert_eq!(
                    deciding.as_deref(),
                    (expected != Surface::InternalTool).then_some(*path),
                    "{path}"
                );
            }
        }
    }

    #[test]
    fn r22_precedence_is_review_then_gate_then_shipping_then_internal_tool() {
        let mixed = [
            "docs/reference/cli.md",
            "crates/edda-core/src/lib.rs",
            ".github/workflows/ci.yml",
            "REVIEW.md",
        ];
        for (take, surface, deciding) in [
            (4, Surface::Review, Some("REVIEW.md")),
            (3, Surface::Gate, Some(".github/workflows/ci.yml")),
            (2, Surface::Shipping, Some("crates/edda-core/src/lib.rs")),
            (1, Surface::InternalTool, None),
        ] {
            let files = mixed[..take]
                .iter()
                .map(|f| (*f).to_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                classify(&files).unwrap(),
                (surface, deciding.map(str::to_owned))
            );
        }
    }

    #[test]
    fn r22_engine_table_authorizes_only_the_engines_it_names() {
        for path in [
            "REVIEW.md",
            ".github/workflows/ci.yml",
            "crates/edda-core/src/lib.rs",
            "docs/reference/cli.md",
        ] {
            let internal_tool = path == "docs/reference/cli.md";
            // Opus reaches authority only through Claude Code; the same id on
            // another transport is authoritative nowhere (R22 with R27).
            assert!(assessed(&[path], "claude-opus-5", "claude-code").authoritative);
            assert!(!assessed(&[path], "claude-opus-5", "pi").authoritative);
            // sol is authoritative on all four surfaces.
            assert!(assessed(&[path], "openai-codex/gpt-5.6-sol", "pi").authoritative);
            // flash only on the internal-tool surface; SHADOW elsewhere.
            assert_eq!(
                assessed(&[path], "openrouter/z-ai/glm-5.3-flash", "pi").authoritative,
                internal_tool
            );
            for unknown in ["claude-sonnet-5", "inherited", "", "totally-made-up"] {
                assert!(
                    !assessed(&[path], unknown, "claude-code").authoritative,
                    "{unknown}"
                );
            }
        }
    }

    #[test]
    fn requested_model_id_is_canonicalized_for_the_table_and_the_brief() {
        let q = assessed(&["docs/reference/cli.md"], "Claude Opus 5", "claude-code");
        assert_eq!(q.engine, "claude-opus-5");
        assert!(q.authoritative);
        // An id naming no known model family is carried through verbatim and
        // stays unqualified rather than being guessed at.
        let q = assessed(&["docs/reference/cli.md"], "some-lab/whatever", "pi");
        assert_eq!(q.engine, "some-lab/whatever");
        assert!(!q.authoritative);
    }

    #[test]
    fn brief_section_tells_an_authoritative_engine_to_close_d5_and_others_to_escalate() {
        let authoritative = assessed(
            &["crates/edda-cli/src/cmd_review/mod.rs"],
            "claude-opus-5",
            "claude-code",
        )
        .brief_section();
        assert!(authoritative.starts_with(SECTION_HEADING));
        assert!(authoritative.contains("R22 surface: shipping (出貨面)"));
        assert!(authoritative
            .contains("decided by changed path `crates/edda-cli/src/cmd_review/mod.rs`"));
        assert!(authoritative.contains("Running engine: claude-opus-5 (transport claude-code)"));
        assert!(authoritative.contains(
            "VERDICT: AUTHORITATIVE for this surface — close D5 yourself; do not escalate it."
        ));
        assert!(!authoritative.contains("NOT authoritative"));

        let checklist_type = assessed(
            &["crates/edda-cli/src/cmd_review/mod.rs"],
            "glm-5.3-flash",
            "pi",
        )
        .brief_section();
        assert!(checklist_type.contains(
            "VERDICT: NOT authoritative for this surface — escalate D5 as REVIEW.md §6.1 requires."
        ));
        // The pool stays visible either way: an Opus-authored PR needs a
        // non-author authoritative engine to be nameable from the brief alone.
        for text in [&authoritative, &checklist_type] {
            assert!(text.contains(
                "Authoritative engines for this surface (R22 engine table): claude-opus-5 (only via Claude Code), gpt-5.6-sol."
            ));
        }
        assert!(assessed(&["docs/reference/cli.md"], "glm-5.3-flash", "pi")
            .brief_section()
            .contains("gpt-5.6-sol, glm-5.3-flash."));
    }

    #[test]
    fn brief_section_states_the_model_diversity_interaction_both_ways() {
        let files = ["crates/edda-cli/src/cmd_review/mod.rs".to_owned()];
        let off = assess(&files, "claude-opus-5", "claude-code", false).unwrap();
        assert!(off.brief_section().contains(
            "`--require-model-diversity` was not passed, so this round's independence policy is `session`"
        ));
        assert!(off.brief_section().contains("`independence: same-model`"));
        let on = assess(&files, "claude-opus-5", "claude-code", true).unwrap();
        assert!(on.brief_section().contains(
            "`--require-model-diversity` was passed, so this round's independence policy is `model`"
        ));
    }

    #[test]
    fn receipt_gains_the_qualification_without_disturbing_existing_keys() {
        let mut value = serde_json::json!({"schema":"review_verdict/0","qualified":true});
        augment(
            &mut value,
            &assessed(&["REVIEW.md"], "openai-codex/gpt-5.6-sol", "pi"),
        );
        assert_eq!(value["schema"], "review_verdict/0");
        assert_eq!(value["qualified"], true);
        assert_eq!(
            value["engine_qualification"],
            serde_json::json!({
                "surface": "review",
                "deciding_path": "REVIEW.md",
                "engine": "gpt-5.6-sol",
                "authority": "authoritative",
                "authoritative_engines": ["claude-opus-5 (only via Claude Code)", "gpt-5.6-sol"],
                "require_model_diversity": false,
            })
        );
        let mut value = serde_json::json!({});
        augment(
            &mut value,
            &assessed(&["docs/reference/cli.md"], "inherited", "pi"),
        );
        assert_eq!(
            value["engine_qualification"]["authority"],
            "not-authoritative"
        );
        assert_eq!(value["engine_qualification"]["surface"], "internal-tool");
        assert_eq!(
            value["engine_qualification"]["deciding_path"],
            serde_json::Value::Null
        );
    }
}
