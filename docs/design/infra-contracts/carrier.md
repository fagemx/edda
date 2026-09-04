# Conductor carrier contract (GH-603)

Status: unratified design plus executable **schema preview only**.
`conductor.carrier=typed-schema-preview-first` recorded 2026-09-04 after the
[finding](finding.md) and [freshness](freshness.md) decisions.

## Carrier and isolation

Phase `deliverable` is optional. Existing plans omitting all new fields keep
their existing behavior. Declaring it opts into a mandatory identity/acceptance
contract in the future runtime; missing, inaccessible, or mismatched declared
delivery fails completion. No implicit carrier is guessed from prose output.

| kind | Required identity | Mandatory future acceptance |
|---|---|---|
| pr | repository, positive number, head_sha (full SHA) | exact-head CI + independent current-head review verdict |
| finding | finding_id, basis (git SHA or immutable document version) | approved independent finding verdict at that basis/revision |
| draft | path, immutable version, nonempty decision_refs | bound independent review against referenced ratified decisions |

Identities are literal frozen references in this first schema. Plans can be
materialized by the wave adapter after the artifact identity exists; automatic
“create and discover the PR number” interpolation is a follow-up concern.
Verification plan for a coding artifact is still the coding shape. Finding
IDs may be allocated before research starts. Draft versions are immutable
content digests or document revisions, not mutable names such as “latest”.

Explicit check lists are additive: they cannot remove the carrier's mandatory
acceptance. An operator waiver must be a separate audited receipt with waived
status; it cannot turn a rejected carrier into accepted. Existing plans retain
legacy `gate: verdict` semantics. A phase verdict does not substitute for a
finding verdict. `on_fail: skip` reports skipped, never delivered/accepted.

`isolation: none | scratch | worktree` is optional; omitted means the existing
externally managed cwd arrangement. `none` means no filesystem write surface;
`scratch` requires an adapter-created disposable directory; `worktree` requires
an adapter-provisioned isolated checkout at cwd. Conductor validates the
declared isolation before dispatch, but does not own provisioning or cleanup.
Tool policy remains separate; `none` alone is not a sandbox.

`owns` continues to mean file globs. `owns_objects: [finding:<id>, source:<source>/<instance>, draft:<id>]`
is a separate typed namespace resolved within the project (and strategy run
for run-local findings), not fake filesystem paths. Conflicting exclusive
object ownership serializes work through the claim registry (GH-581).
Read-only interests do not imply ownership or exclusion.

## Checks and backwards compatibility

New checks use tagged YAML only:

```yaml
check:
  - type: finding_verdict
    finding_id: finding-587
    basis: {kind: git, sha: 580e98678fe6a39f57ad7a4dcbff74ecf47f2be4}
  - type: fresh
    source: heartbeat
    instance: lane-2
    within_sec: 60
```

Fresh uses the shared reducer; within_sec optionally tightens its limit.
Finding-verdict consumes the object contract. When paired with a deliverable,
extra checks may refer to other objects, but the declared carrier still gets
its own mandatory guard. Nesting these draft checks in wait_until is not in
this schema version and fails clearly; legacy checks and short syntax keep
working. No runner or new approval CLI is implemented in this PR.

The draft schema is typed in `edda-conductor/src/plan/preview.rs` and wired to
the existing `edda conduct run <plan> --dry-run`. It validates new fields,
then the existing plan parser and topology. Its legacy projection exists only
for dry-run display: new checks are not executed, not represented as passed,
and not installed into a runnable Plan. The display announces this limitation.
Normal `parse_plan` rejects any carrier, isolation, object claim, run stamp or
new check before dispatch, including explicit null fields. It must never
silently discard a delivery or isolation promise.

Examples: [coding](coding.yaml), [research](research.yaml), [loop](loop.yaml).
Run each with `edda conduct run docs/design/infra-contracts/<shape>.yaml --dry-run`.
The examples are design fixtures: they do not assert a live artifact passes
acceptance. Dry-run performs no network lookup or agent dispatch. Parser tests
also require malformed basis, kind, isolation, run identity and check fields
to fail, and legacy plans to remain valid. CLI process tests exercise the real
dry-run entry point and reject execution without spawning an agent.

## Receipt and run stamp

`strategy_run_id: Option<String>` is a nonempty opaque plan field, supplied by
the wave adapter and copied unchanged to phase-start/progress/completed events,
receipt, verdict request/response binding, and finding creation. Absent remains
absent; a phase cannot override it. A finding from another run may be a cited
input but cannot be relabeled. Conductor does not interpret the five strategy
parameters, select candidates, or expand waves. Parallelism remains between
single-phase plans; templates stay in parallel-wave assets or `.edda/` data.

Proposed completion payload, extending the existing structured GH-584
phase-result path rather than writing another prose-only note:

```json
{
  "schema_version": 1,
  "plan_id": "research-audit",
  "phase_id": "review",
  "attempt": 1,
  "strategy_run_id": "review-587",
  "status": "passed",
  "deliverable": {
    "kind": "finding", "finding_id": "finding-587",
    "basis": {"kind": "git", "sha": "580e98678fe6a39f57ad7a4dcbff74ecf47f2be4"}
  },
  "checks": [{"type": "finding_verdict", "passed": true, "evidence_event_id": "verification-event-id"}],
  "receipt_event_id": "completion-event-id"
}
```

The example event IDs are placeholders. The append returns the receipt ID;
the derived receipt can include it without self-hashing the event. Existing
cost/token measuredness fields remain unchanged; unavailable evidence stays
unknown. Report/status read structured identity and group run ID × problem
class. A terminal result requires output references before emission; retried
delivery reconciles the same `(plan, phase, attempt, carrier identity)` receipt.
The actual stamped runtime events and readers ship in follow-ups.

No parallel-wave/fleet-orchestrate rewrite is included. The reusable wave
layer consumes these contracts when implementations are available.
