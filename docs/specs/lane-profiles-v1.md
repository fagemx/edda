# Lane Profiles v1 (GH-593)

Status: design approved, pending implementation — this document ships no code.
Issue: https://github.com/fagemx/edda/issues/593
Operator ruling: issue comment 2026-09-02, six-row table.
Recorded decision: `fleet.lane-profile = actor-is-profile` (active; `edda ask "lane-profile"`).
Verified against: `origin/main` @ `f582ef3` — every line citation below was
re-opened in a worktree at that SHA.

An **agent actor is the profile**. There is no fourth config surface: the
`.edda/actors.yaml` entry that already names an agent gains the execution
defaults — model, thinking level, tool policy, budget, permission mode — that
today live only in an operator's head and in five CLI flags typed by hand.

---

## 1. What the issue said, and what is true at `f582ef3`

The issue body was written against `580e896`. Seven of its claims about current
behaviour were re-checked below: two hold exactly as written, five had drifted
or were incomplete. The drift is not cosmetic — one item (GH-574) changes what
this design has to build.

| Issue claim | Status at `f582ef3` |
|---|---|
| `ActorDef` at `edda-core/src/policy.rs:100-113` | **Exact.** Struct opens at `:100`, closes at `:113`. |
| `runtime` readers are `cmd_actor.rs:194 / :232 / :252` | **Incomplete.** Those three are real (`:194` and `:232` JSON output, `:252-253` human display), but `edda-serve/src/api/briefs.rs:48` and `:69` also read `def.runtime` and publish it through `GET /api/actors` and `GET /api/actors/:name`. The *substance* survives — still display-only, still no execution reader — but the field's type is part of a public HTTP response body, which constrains §3.3. |
| `Plan` / `Phase` at `plan/schema.rs:6, :33` | **Line numbers exact** (`Plan` at `:6`, `Phase` at `:33`). |
| `Plan` / `Phase` have no `agent` / `model` field | **Half stale.** No `agent` field: still true, and it is the gap this design fills. No `model` field: **false** — `Phase.model` is at `plan/schema.rs:77`, alongside `thinking` (`:82`), `tools` (`:66`), `exclude_tools` (`:71`), `permission_mode` (`:84`) and `budget_usd` (`:54`). |
| `agent_kind.rs:63-84` is `build_launcher` | **Moved.** `build_launcher` is now at `crates/edda-cli/src/agent_kind.rs:191`. Lines `:62-83` are now the GH-574 support predicates `supports_tool_policy` / `supports_permission_mode` / `supports_session_dir` / `supports_model_listing`. |
| plan-context injection at `launcher.rs:158` | **Moved.** Plan-context injection is `crates/edda-conductor/src/agent/launcher.rs:131-132` (`--append-system-prompt`). Line `:158` is now inside the GH-574 tool-denylist merge. The ruling's "v1 does not change plan-context injection" retargets to `:131-132`. |
| Implementation depends on #574 (flags must exist first) | **Discharged.** #574 is CLOSED and landed. `edda dispatch` accepts `--model` / `--thinking` / `--tools` / `--exclude-tools` / `--permission-mode` / `--session-dir` (`cmd_dispatch.rs:58-83`), validates them against a per-backend support matrix (`agent_kind.rs:46-83`, `:107-158`), and copies them onto a synthetic phase (`cmd_dispatch.rs:283-317`). Profile expansion now has real flags to expand into. |

Two further facts that shape the design and appear nowhere in the issue:

- **`.edda/` is untracked.** `git ls-files .edda` is empty at `f582ef3`; no
  `actors.yaml` or `tool_tiers.yaml` ships with the repository. Every schema
  change below is therefore a change to a *workspace-local* file that edda
  itself must migrate, with no in-repo fixture to migrate alongside it.
- **`AgentKind` lives in the binary crate.** It is
  `crates/edda-cli/src/agent_kind.rs:14`, in the package named `edda`
  (`crates/edda-cli/Cargo.toml:2`). `ActorDef` is in `edda-core`. `edda-core`
  cannot depend on `edda-cli`. See the obstruction in §8.1.

---

## 2. The gap this closes

Three policy surfaces exist (`.edda/actors.yaml`, `.edda/tool_tiers.yaml`,
`.edda/policy.yaml`) and none of them reaches execution. `load_actors_from_dir`
has exactly seven call sites at `f582ef3` — `cmd_actor.rs:115`,
`cmd_controls.rs:320`, `cmd_draft.rs:295`, `cmd_propose.rs:353`,
`briefs.rs:38`, `briefs.rs:61`, `policy.rs:186` — and **not one of them is in
`cmd_dispatch.rs`, `cmd_conduct.rs`, `agent_kind.rs`, or anywhere in
`edda-conductor`.** The dispatch path never opens `actors.yaml`.

So "this lane is the read-only reviewer, so it runs the expensive model with no
write tools" is prose in a brief, not a setting edda applies. The observed
consequence is on the record: two reviews dispatched with `--agent pi` landed
silently on the cheap execution model, and dispatch output, PR comment and
receipt were all identical to a correctly-modelled run.

---

## 3. Profile schema

### 3.1 Shape

`ActorDef` (`edda-core/src/policy.rs:100-113`) gains seven fields. All are
optional, all are meaningful only when `kind: agent`, and all default to
absent. Absent means **inherited** — see §4.

```yaml
version: 2
actors:
  reviewer:
    kind: agent
    roles: [reviewer]
    runtime: claude          # existing field, vocabulary narrowed — §3.3
    model: anthropic/claude-opus-5
    thinking: high
    exclude_tools: [Edit, Write, NotebookEdit]
    permission_mode: bypassPermissions
    budget_usd: 2.0
  worker:
    kind: agent
    roles: [implementer]
    runtime: pi
    model: zai/glm-5.3-flash
    budget_usd: 6.0
```

| Field | Type | Required | Absent means |
|---|---|---|---|
| `runtime` | `Option<String>`, narrowed vocabulary (§3.3) | optional | no backend preference; `--agent` decides |
| `model` | `Option<String>` | optional | inherited — backend's own default |
| `thinking` | `Option<String>` | optional | inherited |
| `tools` | `Option<Vec<String>>` | optional | inherited — no allowlist claimed |
| `exclude_tools` | `Option<Vec<String>>` | optional | inherited — no denylist claimed |
| `permission_mode` | `Option<String>` | optional | inherited (see §4.3) |
| `budget_usd` | `Option<f64>` | optional | no budget ceiling from the profile |
| `max_tier` | `Option<ToolTier>` | **reserved** | reserved — §3.4 |
| `lifecycle` | reserved name | **reserved** | reserved — §3.5 |

Every field except `runtime` is named to match the `Phase` field it resolves
against (`plan/schema.rs:54-84`), so the precedence rule in §4 is a per-name
merge rather than a translation table. `budget_usd` is `f64` because
`Phase.budget_usd` is (`plan/schema.rs:54`).

### 3.2 `None` is not the empty list

`tools: Option<Vec<String>>` must not collapse `None` and `Some(vec![])`. The
distinction is load-bearing today: `launcher.rs:152-165` branches on whether
`phase.tools` is `Some`, and an allowlist spawn additionally emits
`--disallowedTools "mcp__*"` while a denylist-only spawn does not.

`Some(vec![])` is nonetheless **rejected at load** with an explicit error. An
empty allowlist would expand to `--tools ""`, which claims a restriction the
backend will not read as one — precisely the GH-574 failure mode. The error
should say: an empty allowlist is not expressible; omit the key to inherit, or
use `exclude_tools`.

### 3.3 Migrating `runtime` onto `AgentKind`

Today `runtime` is `Option<String>` with no validation anywhere. The writer
(`cmd_actor.rs:146-152`) stores whatever `--runtime` was given; the readers
print it. The only vocabulary that exists is in two doc comments —
`policy.rs:110` and `cmd_actor.rs:27`, both reading `(e.g. "claude", "opencode")`.

`AgentKind` (`agent_kind.rs:14-21`) is exactly `Claude | Pi | Codex`. So
**`"opencode"` — the value edda's own help text advertises — is not an
`AgentKind`.** A hard retype breaks any `actors.yaml` carrying it, and, because
`briefs.rs:48` / `:69` publish the field, changes a public HTTP response body's
type at the same time.

Migration is therefore **narrow at read, not at parse**, in three steps:

1. **Prerequisite** — give `edda-core` the backend vocabulary. `AgentKind` is
   in the binary crate and derives `clap::ValueEnum`; `edda-core`'s manifest
   (`crates/edda-core/Cargo.toml:13-24`) has no `clap` dependency. Either move
   the enum plus its `supports_*` matrix into `edda-core` and add `clap` there,
   or define the vocabulary in `edda-core` and keep the `ValueEnum` derive in
   `edda-cli` on a newtype. This is a real prerequisite — see §8.1.
2. **On-disk shape unchanged.** `runtime` stays `Option<String>` in YAML and in
   the `GET /api/actors` response. No file rewrite, no `actors.yaml` version
   bump (`cmd_actor.rs:154` already sets `version = 2` on every add), no
   serve-contract change.
3. **Validate where it is consumed, and where it is written.** Add
   `ActorDef::agent_kind(&self) -> Result<Option<AgentKind>>`, which parses the
   string and errors on an unknown value naming the legal set. Dispatch calls
   it; the error surfaces only when a bad value would actually select a
   backend. Independently, `edda actor add --runtime` validates at write time
   so new bad values cannot be created, and names `"opencode"` explicitly as
   retired in the message.

Net effect: existing `actors.yaml` files keep loading and keep listing, and
exactly one path — using a profile to pick a backend — refuses an unknown
runtime rather than guessing.

### 3.4 `max_tier` — reserved, and refused rather than ignored

Per ruling #5, tier-to-tool derivation is deferred. `max_tier` is parsed and
round-trips so the name cannot be taken by an unrelated meaning, and
`ToolTier` already exists (`edda-core/src/tool_tier.rs:17-23`) with readers in
`edda-mcp/src/lib.rs:413-414` and `edda-serve/src/api/policy.rs:198-199`.

**A profile that declares `max_tier` is refused at dispatch**, not accepted and
ignored. This is not new policy: it is `validate_dispatch_options`'s existing
doctrine (`agent_kind.rs:148-157`) applied to a field edda cannot yet honour.
Accepting `max_tier: T0` and spawning an unrestricted agent would recreate
exactly the silent-no-op GH-574 removed. The refusal message names the
follow-up issue.

### 3.5 `lifecycle` — reserved, and only weakly

Per ruling #6, content awaits #575. Reserving the *name* is what v1 can do.

Honest limit: `ActorDef` does not derive `serde(deny_unknown_fields)`
(`policy.rs:99-100`), so today an `actors.yaml` containing `lifecycle:` is
parsed and the key silently dropped. A reservation that cannot be enforced is a
comment, not a schema. Adding `deny_unknown_fields` would enforce it but is
itself a compatibility change — it would start rejecting files that load today.
v1 documents the reservation and does **not** add `deny_unknown_fields`; #575
decides whether the enforcement is worth the break. Recorded here so the choice
is deliberate rather than forgotten.

---

## 4. Precedence

### 4.1 The rule

> For every capability field `F`, the value edda passes to the backend is the
> first present value in the ordered list
> **CLI flag → phase field → profile field → (nothing)**.
> "Nothing" is not a guess: edda spawns no flag for `F`, the backend's own
> default applies, and every report prints the literal `inherited` for `F`.

```text
resolve(F) = cli.F ?? phase.F ?? profile.F        // Option, never a sentinel
None  =>  no flag on the command line
```

The `inherited` literal is not invented here; it is the existing dispatch
contract (`cmd_dispatch.rs:148-151`, `:519-522`).

| Field | CLI flag | Phase field | Profile field | Fallback |
|---|---|---|---|---|
| backend | `--agent` (`cmd_dispatch.rs:31`, `cmd_conduct.rs:41-42`) | `profile:` (new, ruling #4) | `runtime` | required — `--agent` has no default on dispatch, defaults to `claude` on conduct |
| `model` | `--model` (`cmd_dispatch.rs:64`) | `model` (`plan/schema.rs:77`) | `model` | backend default; report `inherited` |
| `thinking` | `--thinking` (`cmd_dispatch.rs:68`) | `thinking` (`:82`) | `thinking` | backend default |
| `tools` | `--tools` (`cmd_dispatch.rs:77`) | `tools` (`:66`) | `tools` | no allowlist spawned |
| `exclude_tools` | `--exclude-tools` (`cmd_dispatch.rs:83`) | `exclude_tools` (`:71`) | `exclude_tools` | no denylist spawned |
| `permission_mode` | `--permission-mode` (`cmd_dispatch.rs:58`) | `permission_mode` (`:84`) | `permission_mode` | `bypassPermissions` — §4.3 |
| `budget_usd` | `--budget-usd` (`cmd_dispatch.rs:49`) | `budget_usd` (`:54`) | `budget_usd` | plan-level `budget_usd`, else none |

### 4.2 Testable cases

Written so an implementer can transcribe them into `#[test]`s:

| # | Setup | Expected |
|---|---|---|
| P1 | profile `model: A`, phase `model: B`, CLI `--model C` | spawn `--model C`; report `Model requested: C` |
| P2 | profile `model: A`, phase silent, CLI silent | spawn `--model A`; report `Model requested: A` |
| P3 | no profile, no phase field, no flag | **no** `--model` argument in the spawn line; report `Model requested: inherited`. An assertion of this exact shape already exists for the phase-only case at `launcher.rs:441-447` |
| P4 | profile `tools: []` | load error (§3.2); nothing spawned |
| P5 | profile `thinking: high`, `--agent claude` | refused with an error naming the profile as the source (§4.4); **not** dropped |
| P6 | profile `max_tier: T0` | refused, message names the follow-up issue (§3.4) |
| P7 | profile `runtime: opencode` | refused with the legal `AgentKind` set (§3.3) |

### 4.3 `permission_mode` cannot express "inherited" at phase level

`Phase.permission_mode` is `String`, not `Option<String>`, with a serde default
of `"bypassPermissions"` (`plan/schema.rs:83-84` and `:215-217`). A constructed
`Phase` therefore *always* carries a concrete value, and `launcher.rs:112-113`
always spawns `--permission-mode`.

Consequence: **precedence for `permission_mode` must be resolved before the
phase is built, not after.** If a profile's value were merged into an
already-constructed phase, the serde default would look like an explicit
setting and would win over the profile every time.

Dispatch already resolves it in the right place — `cmd_dispatch.rs:428-431`
computes the effective mode and hands it to `build_phase` as a `&str`
(`cmd_dispatch.rs:283-287`, `:312`). The profile lookup slots in there.
`edda conduct` has no equivalent seam yet; the follow-up issue for phase-level
profiles owns creating one.

### 4.4 The GH-574 refusal gate must move to the resolved values

`validate_dispatch_options` runs at `cmd_dispatch.rs:364-374`, on `args` —
the CLI flags — deliberately before every short-circuit including
`--list-models`. It validates what the operator typed.

A profile supplies values the operator did not type. If the gate keeps reading
`args`, a profile declaring `thinking: high` against `--agent claude` passes
validation and is then silently dropped, because `launcher.rs`'s
`build_command` simply never reads `phase.thinking`. That is the GH-574 bug,
reintroduced through the new surface.

**Rule:** the gate validates the *resolved* set, after profile merge and before
launcher construction. Two consequences for the implementer:

- The `--list-models` short-circuit must stay downstream of the moved gate, or
  the round-3 property (`cmd_dispatch.rs:361-363`) regresses.
- The refusal message at `agent_kind.rs:151-156` ends with "drop them or
  dispatch with a backend that supports them", which is wrong advice when the
  value came from `actors.yaml` and not from a flag. It must carry provenance:
  which field, which source, which profile.

---

## 5. Expansion mapping — every field by every backend

Aligned with the #574 support matrix (`agent_kind.rs:46-83`). "Rejected" means
an explicit error, never a silent drop.

| Profile field | claude | pi | codex |
|---|---|---|---|
| `runtime` | selects `ClaudeCodeLauncher` (`agent_kind.rs:191` onward) | selects `PiRpcLauncher` | selects `CodexLauncher` |
| `model` | `--model <v>` (`launcher.rs:136-138`) | `--model <v>` (`pi_rpc.rs:182-184`) | **rejected** (`agent_kind.rs:46-48`; `codex_rpc.rs:263-285`) |
| `thinking` | **rejected** (`agent_kind.rs:51-53`) | `--thinking <v>` (`pi_rpc.rs:192-194`) | **rejected** |
| `tools` | `--tools <csv>` **and** `--disallowedTools` with `mcp__*` merged in (`launcher.rs:152-163`) | `--tools <csv>` (`pi_rpc.rs:195-197`) | **rejected** |
| `exclude_tools` | `--disallowedTools <csv>`; merged into the single flag above when `tools` is also set (`launcher.rs:158-165`) | `--exclude-tools <csv>` (`pi_rpc.rs:198-200`) | **rejected** |
| `permission_mode` | `--permission-mode <v>` (`launcher.rs:112-113`) | **rejected** (`agent_kind.rs:70-72`) | **rejected** |
| `budget_usd` | `--max-budget-usd <v>` (`launcher.rs:125-128`) | enforced post-hoc from reported usage | **accepted but unenforced**, with a warning — codex reports no usage (`cmd_dispatch.rs:422-424`, `cmd_conduct.rs:481-486`, `codex_rpc.rs:543-546`) |
| `max_tier` | reserved — refused (§3.4) | reserved — refused | reserved — refused |
| `lifecycle` | reserved — no effect (§3.5) | reserved — no effect | reserved — no effect |

Two asymmetries a profile author must be told about, because a profile makes
them invisible in a way a typed flag does not:

- **`tools` means more on claude than on pi.** On pi, `--tools` replaces the
  default tool set and nothing else. On claude it restricts only the built-in
  set, so edda additionally denies every unlisted MCP tool via
  `--disallowedTools "mcp__*"` (`launcher.rs:152-163`). The same profile line
  yields a strictly tighter sandbox on claude.
- **`budget_usd` on codex is a wish.** It is accepted, warned about, and not
  enforced. A profile that exists to cap a cheap execution lane must not be
  read as a guarantee when that lane runs codex.

`session_dir` is deliberately **not** a profile field in v1. It is per-lane
state, not per-role policy; two lanes sharing the `worker` profile must not
share a session directory. It stays a flag (`cmd_dispatch.rs:87`,
`pi_rpc.rs:185-187`).

---

## 6. Alignment with #582 — the event field

**Settled, per ruling #3: the field is `actor`, type `Option<String>`.**

Status at `f582ef3`: **#582 is OPEN and unimplemented**, and
`crates/edda-cli/src/cmd_dispatch.rs` contains no `Ledger` reference and
appends nothing — a dispatch's cost evaporates when the process exits. #582's
own doneWhen commits to a minimal write-end carrying session id, agent, model,
elapsed and `cost_usd`. **`actor` must be reserved in that payload when it is
first written, not retrofitted afterwards.**

| Property | Value |
|---|---|
| Name | `actor` |
| Type | `Option<String>` |
| Value | the `actors.yaml` map key — `ActorsConfig.actors` is a `BTreeMap<String, ActorDef>` (`policy.rs:64-68`) — never the display name |
| `None` | dispatched without a profile. Never a sentinel string such as `"none"` or `""` |
| Purpose | the group-by key for `edda report cost` |

Why `Option` here while `VerdictPayload.actor` is a plain `String`
(`edda-core/src/types.rs:243`): a verdict always has an issuer, whereas a
dispatch may legitimately have no profile. The *name* is deliberately the same
word, and this is the payoff of ruling #1 — because a profile **is** an actor,
`actor` in a dispatch event and `actor` in a verdict event are the same
namespace by construction. Under option (b) they would have been two
identifiers requiring a join in every report.

Phase events use the same field name. The conductor's phase terminal state
reaches the ledger as a note with a structured `conductor_phase` payload
`{ plan_id, phase_id, status, cost_usd }` (`runner/edda.rs:171-217`); `actor`
is added there with identical semantics. Note the sequencing: the backend is
resolved once per run at `cmd_conduct.rs:41-42`, so a phase-level `profile:`
must reach `record_phase_done_with_plan`'s call site before the field can carry
anything but the run-level value.

**Ask to #582's implementer:** reserve `actor: Option<String>` in the dispatch
cost payload, serialized as JSON `null` when absent — the same
absent-is-null discipline #533 established for `cost_usd`
(`runner/edda.rs:176-181`). Nothing else from this design is required of #582.

---

## 7. Explicitly out of scope for v1

- **Plan-context injection is unchanged.** Ruling #4. The injection site is
  `crates/edda-conductor/src/agent/launcher.rs:131-132` for claude
  (`--append-system-prompt`), and inline prepending for pi (`pi_rpc.rs:261-265`)
  and codex (`codex_rpc.rs:289-293`). A profile does not alter what is
  injected; the brief text remains the operator's.
- **No tier-to-tool derivation.** Ruling #5. `max_tier` is a name, refused if
  set.
- **No lifecycle semantics.** Ruling #6, pending #575.
- **No inference from an agent's private config.** Explicitly not done, per the
  issue's own scope-out: edda does not read pi's `settings.json` or claude
  settings to discover a model.
- **No flag work.** #574 shipped it.
- **No cost reporting.** #582 / #584 / #585.

---

## 8. Open obstructions

The design as ruled is implementable, but four things are true at `f582ef3`
that the ruling did not account for. None of them changes a decision; each
changes what the implementation has to do first.

### 8.1 `AgentKind` is in the wrong crate for the ruling as literally written

Ruling #1 says `runtime` is narrowed to `AgentKind`. `AgentKind` is
`crates/edda-cli/src/agent_kind.rs:14`, in the binary package `edda`;
`ActorDef` is `edda-core`. `edda-core` cannot depend on `edda-cli`, and
`edda-core`'s manifest carries no `clap` (`crates/edda-core/Cargo.toml:13-24`),
which `AgentKind`'s `ValueEnum` derive needs.

Not a blocker — §3.3 keeps the field a `String` on disk and narrows at the
consumption site, which sidesteps the layering entirely for the schema. But the
*vocabulary* still has to be reachable from `edda-core` or from wherever
validation lands, and that is a prerequisite commit, not a line in the profile
issue.

### 8.2 The refusal gate validates flags, not resolved values

§4.4. Left unmoved, profile-supplied unsupported fields are accepted and
silently dropped — the exact defect GH-574 closed. The move is small and must
preserve the round-3 property that the gate precedes `--list-models`.

### 8.3 `Phase.permission_mode` has no absent state

§4.3. Precedence for this one field must resolve before phase construction.
Dispatch has the seam; conduct does not.

### 8.4 A reserved name cannot be enforced today

§3.5. Without `serde(deny_unknown_fields)` on `ActorDef`, `lifecycle:` in an
`actors.yaml` is silently discarded rather than reserved. Adding the attribute
is a compatibility break in its own right. v1 documents; #575 decides.

### 8.5 `docs/specs/` did not exist

This document creates the directory. Its conventions are taken from the nearest
in-repo analogues: the status/issue/depends header block of
`docs/plan/search-auto-index/SEARCH_AUTO_INDEX_V1.md` and the numbered-section,
explicit-non-goals structure of `docs/decision/decision-model/schema-v0.md`.

---

## 9. Implementation split

This document ships no Rust. The work below becomes follow-up issues, ordered
by dependency.

| # | Scope | Depends on |
|---|---|---|
| F1 | Backend vocabulary reachable outside `edda-cli`; `ActorDef::agent_kind()` validating accessor; `edda actor add --runtime` write-time validation, naming `opencode` as retired | — |
| F2 | `ActorDef` gains `model` / `thinking` / `tools` / `exclude_tools` / `permission_mode` / `budget_usd`; `Some([])` rejected at load; `max_tier` parsed and refused; `lifecycle` documented as reserved | F1 |
| F3 | `edda dispatch --profile <actor>`; resolution per §4; refusal gate moved to resolved values with provenance in the message; `permission_mode` resolved before `build_phase` | F2 |
| F4 | `Phase.profile` field; run-level `--agent` override; conduct-side pre-phase resolution seam | F2, F3 |
| F5 | `actor: Option<String>` populated in the dispatch cost event and in `conductor_phase` | F3, F4, #582 |
| F6 | `max_tier` to per-backend tool list derivation; removes the F2 refusal | F3, `.edda/tool_tiers.yaml` in use |
| F7 | `lifecycle` content | #575 |
