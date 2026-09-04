# Finding contract (GH-602)

Status: design candidate, not a shipped finding API. Decision recorded on
2026-09-04 as `finding.model=independent-events-bound-verdict-run-local` in
the project ledger, agent-authored/unratified. Basis for this design:
`467e8be02eb98fb47d0eca82e9dd400d91d67e6a`.

## Storage and identity

Use independent append-only `finding.created`, `finding.attempted`,
`finding.verified`, `finding.promoted`, `finding.dropped`, and `finding.linked`
events. A finding has a stable ID; the derived view folds these events in ledger
order. Tasks still describe work and lease/dependency state; adding a task kind
would conflate completed work with verified knowledge. Tasks may reference a
finding ID in their receipt. No extra runtime crate is needed.

All events carry project_id, finding_id, event_id, actor (stable principal plus
session ID), timestamp, schema_version=1, and optional strategy_run_id. The
ledger supplies the receipt event ID after append; it is not a circular input.
The writer enforces valid prior revision and transition atomically. An import
records original source time separately from ingestion time and never pretends
historical evidence was executed by the importer.

## Field and state matrix

R = required, O = optional, inherited = immutable from creation.

| Field | candidate creation | verified | decision / issued | dropped |
|---|---|---|---|---|
| question, author, project_id, finding_id | R, nonempty | inherited | inherited | inherited |
| basis | R: git full SHA or document URI + immutable version | exact creation basis | inherited | inherited |
| evidence_bar | R: nonempty list of repro / failing_test / trace / direct_code_proof | satisfied explicitly by verdict | inherited | inherited |
| attempts | R list, may initially be empty | at least one evidence item | inherited | inherited |
| next_experiment | R nonempty bounded experiment | O | O | O |
| strategy_run_id | O, opaque string | inherited | inherited | inherited |
| visibility | R, default run_local with run ID, otherwise private | project | project or explicit global | prior visibility |
| verdict | absent | R approval, verifier, basis, RAN/READ and rationale | inherited | O rejection evidence |
| outputs | empty | empty or prior issued refs | R nonempty typed references | no new output |
| reason | O | R verdict rationale | R promotion rationale | R disposition reason |
| receipt | event ID returned on every append | R verification event ID | R output/promotion IDs | R drop event ID |

`basis: {kind: git, sha: <40 hex>}` or
`basis: {kind: document, uri: <stable locator>, version: <immutable digest/version>}`.
An attempt has actor, observed_at, kind, command_or_locator, outcome, and
evidence_ref (durable blob or URL). A verdict identifies subject finding_id,
basis, finding revision, verifier principal/session, approved/rejected,
ran[] and read[] evidence references, rationale. Either evidence list may be
empty, but their union must satisfy evidence_bar. READ names the original
executor and source receipt; it never increments the current verifier's RAN.

## Transitions, independence, and promotion

| From | Event / actor | To | Required guard |
|---|---|---|---|
| absent | created / author | candidate | all creation fields valid |
| candidate | attempted / contributing researcher | candidate | evidence append; no basis mutation |
| candidate | verified / independent verifier | verified | approved bound verdict, evidence bar satisfied |
| candidate | dropped / author or controller | dropped | reason plus any negative evidence |
| verified | promoted / controller | decision or issued | approved exact revision/basis, durable output refs |
| verified | dropped / controller | dropped | reason; retain verification history |
| issued | promoted / controller | issued | append additional distinct output refs idempotently |
| any | linked / controller | unchanged | same basis/question scope, preserve aliases/evidence |

Verifier principal must differ from the original author and researchers who
materially authored the candidate claim before verification. The verifier may
run independent checks and attach their evidence to its verdict; that does
not make it the candidate's author. Changing a session ID does not create
independence. This is checked from the trusted host actor identity, not a
self-asserted payload string. Changed basis or changed substantive claim creates
a new candidate linked with `supersedes`; it cannot reuse the old verdict.
Terminal decision/dropped states cannot be silently reopened.

Finding owns a typed verdict rather than reusing the plan-phase `edda verdict`
subject namespace: a phase approval is not proof of a finding. The conductor's
`finding_verdict` check consumes the approved finding revision and exact basis
([carrier contract](carrier.md)); it does not manufacture a verification.

Promotion to a decision invokes the existing decision writer with
`provenance: {finding_id, basis, verification_event_id}` and returns its event
ID. That decision remains **unratified**; promotion never calls `ratify`.
Promotion to issues renders the GH-599 body contract (What happened, Why it
matters, Suspected surface, doneWhen, Relation to existing issues), attaching
the same provenance, and records returned issue URLs. One verified audit may
produce several issues, each mapped to its evidence and scope. External issue
creation uses an idempotency marker `(finding_id, output_key)` and reconciliation
before retry: a crash after HTTP success must not create a duplicate. Mark
issued only after at least one durable output reference is recorded; partial
fan-out remains visible with pending output keys.

## Shared scent and reporting

Candidates with strategy_run_id are immediately visible to the same project's
same run through pack/hook delivery after durable append. Candidate with no run
ID is private to its author/controller; it must not fall into a project-wide
NULL run bucket. Verified findings become project-visible. Global export is an
explicit operator action after verification, with source project provenance;
it is never the default. Doorbells carry event IDs; readers resume from a
ledger cursor after dropped notifications rather than polling every lane.

Deduplication suggests matches by normalized question + basis + source scope
within the project/run. A controller confirms equivalence; similar text alone
cannot merge distinct claims. Canonical ID is the earliest ledger event (event
ID tie-break), all other IDs become aliases, contributors/evidence retain their
authorship. Stronger evidence augments the canonical object, never steals credit
or silently upgrades its state. Conflicting findings remain separate and linked.

The control report groups by project, strategy_run_id (including explicit
unattributed bucket), and ISO UTC week. Count created/verified/issued/decision/
dropped transitions once per canonical finding; report verification survival
as verified members of a creation-week cohort / created members, plus its
as-of time. Report pending members and zero denominator as n/a. Deduped aliases
and imported historical records are separate counts, preventing a backfill from
inflating this week's discovery rate. Output issue count is distinct from
finding count. No transcript mining is necessary.

## Historical lifecycle: PR #587

This is a **backfill mapping**, not a new verification or a claim that the
finding command exists. Authoritative source:
[#587 post-merge review](https://github.com/fagemx/edda/pull/587#issuecomment-5502748470),
basis `580e98678fe6a39f57ad7a4dcbff74ecf47f2be4`. The source names the detached
read-only pi reviewer session `review-pr587-postmerge`, observed model
`gpt-5.6-sol`, 19 turns / 9m52s / $1.20, and independent controller verification.

1. `finding.created`: ID `finding-587`; question “Does the merged skill export
   satisfy its safety and policy contract?”; author the research reviewer;
   basis above; run `review-587`; visibility run_local; evidence_bar
   `[repro, direct_code_proof]`; attempts `[]`; next_experiment “Audit the
   frozen skill behaviors and reproduce each failing branch without mutation.”
2. `finding.attempted`: reviewer evidence from that comment: RAN shell control
   flow printed `WOULD_DELETE_EXISTING_INTEL_TREE`; RAN backup check-ignore
   returned exit 1; READ exact merge-tree merge-authority and verification
   ladder text. Four separately scoped claims are attached to this audit.
3. `finding.verified`: the **controller**, distinct from the research author,
   independently reproduced the backup omission and read the cited hazardous
   shell/policy blocks. Its bound approved verdict certifies that the four
   defects exist, not that PR #587 passes review. READ references preserve
   which repro belonged to the reviewer; controller RAN is only the
   check-ignore reproduction stated in the source. Project visibility begins.
4. `finding.promoted`: state issued; four outputs, each with its own output key:
   [#595](https://github.com/fagemx/edda/issues/595) P0 shell precedence and stash
   safety; [#596](https://github.com/fagemx/edda/issues/596) P1 backup ignore;
   [#597](https://github.com/fagemx/edda/issues/597) P1 merge/cleanup authority;
   [#598](https://github.com/fagemx/edda/issues/598) P1 gate-text contradiction.
   Each issue cites the same frozen basis and review. No decision promotion or
   ratification occurred; the merged tree was not reverted.

Real names/receipt IDs not exposed by the historical carrier remain source
references, not invented ledger IDs. An actual import must resolve distinct
trusted principals before accepting the verified transition. The example
demonstrates fan-out and provenance without laundering the earlier superseded
pre-merge approval into the authoritative verdict.
