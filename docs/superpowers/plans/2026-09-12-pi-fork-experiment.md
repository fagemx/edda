# Pi fork feasibility experiment

**Goal:** Find a measured, usable recipe for splitting long coding work into small
parallel bundles with a cheap capable model, rather than assuming fork is faster.

## Experiment contract

Use the same frozen source, acceptance tests, DeepSeek model and thinking setting.
The coding workload builds a read-only cross-project status-card pipeline from the
existing supervisor JSON shape: card normalization, attention ordering, and bounded
overview rendering. Three coherent source-file bundles have a frozen interface.
Each condition uses fresh isolated Git worktrees and one integrator. No worker
publishes a PR or merges; final integration/validation cost is measured separately.

Primary arms: serial with inherited context; three-way parallel with inherited
context; serial with a compact brief; three-way parallel with compact briefs.
Prepare a real planning parent with repository/contract reading, then fork a fixed
completed checkpoint through Pi's native SessionManager API. Compact arms get the
same relevant contract and source files, without inherited conversation history.
Serial runs the same three narrow workers one at a time; parallel runs those
three concurrently, so session count and bundle size are controlled. An optional
single-session all-bundles run is a separately labeled practical baseline.
Run at least three repetitions when practical; vary order. Add fanout or a late
interface-update case after the pilot shows which comparison is informative.

Metrics: preparation/run/integration/repair wall time; model request/tool counts,
tokens/cache counts and reported cost; initial/final test success; scope drift;
conflicts; available process RSS. Report sample counts and limits. Do not claim
statistical generality, shared provider KV cache, or model superiority from one case.

## Runtime work

Native fork requires an idle checked parent checkpoint. Validate persisted source
session identity and leaf, create a private stable snapshot, then use native Pi
forkFrom to create a distinct child file/cwd. Start the child in its own
experimental RPC runtime. Do not use runtime.fork to replace the live parent. The child receives an
explicit narrow bundle and unchanged authority despite inherited parent history.
Fork intent/source digest/child identity are durable and retries never create twins.
No shell keystroke automation and no raw session/chain-of-thought publication.

## Execution

- Independent verifier reviews baseline contract and measurement validity before
  parallel coding; sources/branches remain frozen at each validation handoff.
- Build/run the fork prototype and offline identity/history tests.
- Create the workload/acceptance fixture and seed planning parent.
- Run pilot matrix, inspect outcomes, repeat or expand useful comparisons.
- Collect code/test receipts automatically; integrate disjoint bundles once per arm.
- Publish findings and a practical recipe, including failures and repair costs.
- Ship only validated runtime/harness changes after independent review/CI/R6.
  No local Cargo build lane is needed. Preserve experiment sources/branches and
  private session artifacts; stop only owned test processes.
