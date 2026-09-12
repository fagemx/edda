# Pi fork / parallel coding experiment

Issue: [#1161](https://github.com/fagemx/edda/issues/1161).
Completed local measurements: 68 primary/follow-up coding sessions, plus 18 paid
pilot coding sessions. All raw artifacts are preserved privately.

## Primary results

| Condition | Runs | Strict delivery passed | Median seconds | Range seconds | Median reported USD |
|---|---:|---:|---:|---:|---:|
| Three serial workers, inherited | 3 | 3/3 | 63.406 | 48.033–70.850 | 0.009896 |
| Three parallel workers, inherited | 3 | 3/3 | 27.032 | 25.899–29.459 | 0.009198 |
| Three serial workers, brief | 3 | 2/3 | 74.414 | 69.314–82.456 | 0.008836 |
| Three parallel workers, brief | 3 | 1/3 | 33.861 | 29.905–36.601 | 0.007986 |

All 14 primary/secondary integrated outputs passed **138 functional checks each**.
The three strict failures were extra local scratch/self-check files, not functional
failures or changes to another session's worktree. This distinction matters:
an integration policy could quarantine harmless extra files; this experiment keeps
the original strict scope verdict and separately reports functional correctness.

Matched inherited parallel delivery reduced median wall time by 57% (2.35x speedup)
relative to matched serial. But the single-session inherited secondary baseline took **31.908s**
(USD 0.005117), close to the parallel median of 27.032s. The single-session brief
baseline took 55.397s (USD 0.007133). These one-off secondary results show why
splitting every tiny task is not a general speed rule.

The primary run used 38 coding sessions and 175 completed worker model requests,
plus one preparation request. Preparation took 4.890s; its checkpoint was 33,184
bytes / four persisted entries, with exact source supplied in the parent prompt.
Reported cost including preparation was **USD 0.122348454**. The largest sampled
sum of active worker RSS in a primary arm was **324.96 MiB**, excluding the idle
parent and controller. RAM was not the observed constraint at this scale.

See [primary.json](primary.json) for all rows, tokens/cache counts, costs,
fingerprints, resource samples summarized by arm, and validation receipts.
[pilots.json](pilots.json) retains the failed/superseded pilot observations and
their reported usage separately; they are not pooled into these medians.

## Follow-up results and usable recipe

| Custom submission-tool condition | Deliveries | Seconds | Workers accepted first submission | Reported USD |
|---|---:|---:|---:|---:|
| Six inherited workers, run 1 | 2 | 40.607 | 6/6 | 0.023358 |
| Six brief workers, run 1 | 2 | 38.129 | 6/6 | 0.013448 |
| Six brief workers, run 2 | 2 | 33.877 | 6/6 | 0.012132 |
| Six inherited workers, run 2 | 2 | 39.367 | 6/6 | 0.020276 |
| Changed interface, inherited + verified release | 1 | 43.030 | 3/3 | 0.009761 |
| Changed interface, brief + verified release | 1 | 47.500 | 3/3 | 0.009517 |

All 30 follow-up workers passed on their first submission, with no scope drift or
tool errors. Every complete delivery passed 138 checks. Both changed-interface
consumers were released after real producer acceptance and **before producer turn
settlement**. Their immutable artifact digests are in [followup.json](followup.json).
All four fanout waves sampled six simultaneous workers; maximum sampled combined
worker RSS was **668.15 MiB**. No provider-rate-limit error was observed at six.

The follow-up reported USD 0.088491486. Primary, follow-up and completed pilot
message receipts together report **USD 0.269225064**. Interrupted pilot `d` has no
usage receipt, so this is an observed accounting total, not a guaranteed final bill.

The feasible recipe from these observations is:

1. Split by independent deliverable and stable interface, not a fixed turn count
   or one file per agent. Keep tiny, closely related changes in one session.
2. Use a completed native fork when historical decisions materially help the child.
   Use a compact current contract when the bundle is self-contained. The custom
   submission-tool brief runs delivered correctly with substantially fewer input
   tokens than the inherited runs; wholesale history copying is not mandatory.
3. Start with a small bounded pool (the experiment exercised three and six), give
   each worker a separate worktree, and keep one integrator. A submitted artifact
   and a completed assistant turn are different events.
4. Put ordinary write-and-test feedback in the worker's tool loop. Read-only task
   retrieval plus a scoped submit/validate operation removed the observed extra
   scratch files without an approval conversation. The follow-up changes several
   factors at once, so this is a successful recipe, not proof of one causal factor.
5. Automatically collect receipts and release validated dependencies. Keep final
   integration acceptance; do not infer it from “done,” a live process, or message
   delivery. Recoverable tool errors should not become extra authorization gates.

The experiment establishes feasibility for this small coding workload. It does
not establish optimal fanout, behavior on hundreds of turns, adversarial isolation,
or reliable unattended production work across arbitrary repositories. General
managed `run-fork`/recovery and automatic task-boundary discovery remain subsequent
product work; this PR intentionally ships the measured prototype and evidence.

## Question and method

Can a cheap coding model deliver independent small bundles faster when a controller
forks completed context into separate Pi sessions, rather than running them serially?

The primary experiment controls session count: each arm uses three workers assigned
the same three pure JavaScript modules (normalization, attention ordering, bounded
UTF-8 overview). Serial runs those sessions one at a time; parallel runs them
together. Inherited arms add one identical native Pi checkpoint; brief arms start
fresh with the same current assignment and accessible frozen source. Four conditions
run three times in varied order. Two single-session all-module runs are secondary
operational baselines, always run last, not randomized causal comparisons.

The source contract and executable acceptance are in
[`integrations/pi/fixtures/fork-workload`](../../../integrations/pi/fixtures/fork-workload).
Each worker gets a separate Git worktree, a declared output file, the same model and
tool policy, and up to two acceptance-driven repair prompts. One controller
integrates only assigned files and runs combined acceptance. Assistant completion
text is never acceptance. Extra files count as scope drift under the predeclared
strict primary contract, even when isolated and harmless.

Model: OpenRouter `deepseek/deepseek-v4.1-flash`, observed thinking `high`.
Pi 0.85.1, Node 24.14.0, Windows, 32 logical CPUs, about 95.8 GiB RAM.
Provider automatic retries and Pi compaction are disabled. The primary worker tools
are `read`, `write`, `edit`; the controller runs tests. A recoverable tool error is
telemetry and does not suppress later repair.

Parent preparation, each arm's wall time, startup, model-active time, repair,
validation and integration are separate receipts. Costs are the Pi provider's
reported usage totals, not an independently reconciled bill. RSS/CPU are sampled
roughly every three seconds for owned Pi children only; they are not exact peaks.
Provider cache/load, model randomness and host background activity remain sources
of variance. No statistical generality or model-ranking claim is warranted.

## Native fork mechanics

The experiment uses `SessionManager.forkFrom` from the installed Pi SDK, then opens
the child session file in a separate RPC process and target worktree. It does not
call a live runtime's in-place fork operation. A parent must be idle with a linear,
uncompacted history and no unmatched tool call. Identity, leaf and bytes are checked
before creating an immutable private snapshot. Fork intent is exclusive; a repeated
worker directory cannot silently create another child.

Actual offline Pi tests prove that two child providers see inherited user context,
all three processes/session IDs differ, children leave the parent unchanged, and
the parent subsequently continues without seeing child messages. Busy/incomplete
checkpoints, changed snapshot bytes and duplicate intent are rejected. A deliberately
hung offline provider also produces a timeout and its owned process stops.

This is an explicitly owned experimental runtime. It does not add a general
`run-fork` command to the managed supervisor, supervise arbitrary user sessions,
or promise crash-resume orchestration. A context snapshot is not shared live state
and does not grant broader authority. Git worktrees and narrowed tools are not an
OS security sandbox; generated JavaScript is executed by acceptance.

## Pilot corrections and negative observations

- Pilot `a`: requesting `low` produced observed `high` in this installed Pi/model.
  Worker profile checks rejected before coding prompts; this is a harness/profile
  failure, not evidence that the model cannot code. Subsequent runs explicitly pin
  the observed level.
- Pilot `b`: an overly strict harness suppressed repair after an ordinary
  `read(directory)` error. Removed that suppression. Scope drift still stops
  further repair. One otherwise passing worker created extra self-check files;
  the strict scope check correctly records that separately from code correctness.
- Pilot `c`: three background reads failed, so the inherited seed was shorter than
  intended. Subsequent preparation supplies exact frozen source bytes directly in
  the parent prompt. Earlier pilot timings are retained but not pooled into the
  primary comparison.
- An interrupted `d` directory is preserved without complete result receipts. The
  harness refused to overwrite it; `e` is the separate primary run.
- Execution-pin changes stopped superseded pilot matrices before another worker
  was spawned. Failed runs and their reported costs remain in the private evidence.
- An actual offline probe showed that loading a custom extension alone was not
  sufficient under an explicit CLI tool list. The follow-up explicitly selects the
  custom pair; a second actual-Pi probe confirmed that the provider sees exactly
  `fork_read` and `fork_submit`, with built-ins absent.

## Follow-up design

Six-way fanout uses two independent three-module deliveries. A separate custom-tool
condition provides `fork_read` and `fork_submit`: the latter writes one assigned
module and immediately returns actual acceptance feedback, with up to four
submissions. This changes both tool ergonomics and repair timing, so it is a recipe
test, not an isolated causal estimate of fanout size.

The interface-v2 scenario forks old v1 context, supplies the current v2 contract,
and runs the producer alongside independent overview work. A verified producer
artifact releases the attention consumer without waiting for unrelated work. The
contract already states the new interface; this measures current-contract handling
and an imposed verified-release barrier, not whether artifact communication was
necessary to discover the interface.

## Reproduce and inspect

Run from the repository root with Node 24, installed Pi and configured OpenRouter
credentials. Supply a new experiment directory whose parent already exists. The
live commands spend model credits, create owned worktrees and preserve artifacts.

```powershell
$piEntry = 'C:/nvm4w/nodejs/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js'
node integrations/pi/fork-smoke-offline.mjs $piEntry
node integrations/pi/fork-smoke-tools.mjs $piEntry
node integrations/pi/fork-smoke.mjs $piEntry C:/experiments/fork-primary 3
node integrations/pi/fork-smoke-report.mjs C:/experiments/fork-primary C:/experiments/summary.json
node integrations/pi/fork-smoke-stress.mjs $piEntry C:/experiments/fork-primary C:/experiments/fork-followup
```

Private run roots are under `%USERPROFILE%/.edda-pi-sessions/experiments/`.
They contain frozen Git bases, per-worker worktrees and diffs, assignment/identity/
progress/submission/acceptance receipts, resource samples and session files. Raw
conversations and reasoning are not published in this directory. Only aggregate
measurement and necessary source/acceptance fingerprints are publishable.

## Verification

- `npm --prefix integrations/pi test`: 72/72 passed locally on Node 24.14.0,
  including real failure-feedback and fixed-output tests for the submission tool.
- `fork-smoke-offline.mjs`: actual Pi native fork/context independence, duplicate,
  changed/busy/incomplete checkpoint rejection, and hung-provider timeout/stop passed.
- `fork-smoke-tools.mjs`: actual provider-visible tool set is exactly the two custom
  tools; built-in read/write/edit/bash are absent in that condition.
- Both experiment baseline repositories remained clean. Every spawned coding
  session is an owned experiment session; the harness stops its actual child
  processes and preserves all worktrees/source/evidence.
- The primary measured runtime predates the optional custom-tool selector only;
  its default tool list is unchanged. Exact measured script bytes are retained in
  the private `harness-source` directories, with fingerprints in the public JSON.
- No local Cargo build: this change adds JavaScript experiments and documentation,
  with no Rust product change. Exact-head CI and independent PR review follow.
