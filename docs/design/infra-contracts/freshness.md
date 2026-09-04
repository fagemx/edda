# Freshness contract (GH-604)

Status: unratified design, `signal.freshness=ledger-cadence-scoped-progress`,
recorded 2026-09-04. No registry/reducer/pack writer ships in this design PR.

## One definition for all consumers

Writer registers append-only `source.registered` with schema_version,
project_id, source, instance_id, generation, actor, expected_cadence_sec,
stale_after_sec, dead_after_sec, effective_at. Cadence must be positive and
`cadence <= stale_after < dead_after`. Defaults are 2× and 10× cadence;
overrides are explicit, audited registrations. Latest active generation wins;
updates do not erase earlier observations. `source.retired` ends expected
activity and reports inactive, never fresh. Config can supply initial defaults
but cannot silently override the ledger history.

Registry scope is `(project_id, source, instance_id, generation)`. This avoids
one active lane's heartbeat masking another dead lane. Consumers join only
the current generation and use ledger acceptance timestamps, not arbitrary
payload timestamps; future/skewed remote times are diagnostics, not evidence
of freshness. Query takes an explicit UTC `as_of`; unknown registrations or
missing/corrupt timestamps produce unknown, never fresh. Strict inequalities:

| Source / active instance | Cadence | Fresh age | Stale age | Dead age |
|---|---:|---:|---:|---:|
| heartbeat / lane session | 30s | <=60s | >60s and <=300s | >300s |
| digest / pending transcript job | 300s | <=600s | >600s and <=3000s | >3000s |
| conductor / running phase attempt | 30s | <=60s | >60s and <=300s | >300s |
| dispatch / active dispatch run | 30s | <=60s | >60s and <=300s | >300s |

These are proposed initial defaults, not claims about current writer cadence.
Enable a registration only when its writer can fulfill that cadence. Event-
driven digest jobs register when work is pending and retire when caught up;
idle sessions do not accumulate false deaths. Phase begin/done alone are not
periodic: conductor uses progress heartbeat while running and terminal events
retire that instance. Dispatch similarly joins its existing heartbeat with
terminal outcome; completion event frequency is not its cadence.

`source.observed` references a real durable event and observation class
`heartbeat` or `progress`. Registration declares which class it expects.
Empty/duplicate digest output does not count as progress: the transcript cursor
must advance and the writer must report input/output counts. A healthy process
heartbeat cannot hide a stuck progress source. If no qualifying observation
exists, show `starting (none)` up to stale_after since registration, then stale
or dead at the same boundaries; `starting` is never accepted by a fresh check.
At a new generation, old timestamps cannot keep a restarted source fresh.

## First consumption proof

The first implementation wires the shared derived view into memory pack:

```text
sources: heartbeat/lane-2 fresh 18s ago · digest/job-9 DEAD 4h ago (cadence 5m)
         conductor/phase-a stale 90s ago · dispatch/run-7 starting (none)
```

The line includes source/instance, age or none, state, and cadence when stale/
dead. DEAD remains visible under pack truncation before healthy source detail;
multiple dead instances are summarized with count and a query locator rather
than silently dropped. Missing registration displays `unregistered` until
rollout completes. Historical data without a cadence is unknown, not dead.
Consumers expose the registration/event IDs and as_of for audit.

GH-573 remains a separate fleet-watch consumer, **not merged into this issue**.
It reads this reducer rather than recomputing heartbeat absence. Its existing
behavior remains until the adapter lands; no alternate production thresholds
are introduced here. Cost report GH-582 and status GH-567 follow the same
view. Observation never grants recovery/reclaim/merge authority.

Conductor `fresh` requires an active registered source with at least one
qualifying observation whose derived state is fresh. Optional within_sec is
a stricter age cap, combined with registry freshness using AND. Missing,
unknown, starting, inactive, stale and dead fail with structured reason.
Exactly 2× cadence passes, exactly 10× is stale, greater than 10× is dead.
The check receives explicit source + instance; it cannot accidentally select
the newest heartbeat from another run. This definition is shared with the
[carrier schema](carrier.md), not duplicated in a runner.

Implementation tests must cover threshold equality, restart generation,
per-instance isolation, no first observation, retirement, future timestamps,
empty digest progress, pack truncation and the same facts through fresh check
and watch. This delivers death visibility, not only a timestamp writer.
