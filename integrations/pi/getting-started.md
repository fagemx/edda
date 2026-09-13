# Start and continue an Edda-managed Pi

This is the installed user entry, not a development delegation. No private
worktree, prior conversation or supervisor setup is required. The `edda-pi`
client is separate from the Rust `edda` executable. Node 24+ and an installed,
authenticated Pi are required for real model work. Use your existing Pi login;
do not put credentials in commands, prompts, receipts or this document.

## 1. Install once from an Edda checkout

From the checkout's root (PowerShell or Bash):

```text
npm pack ./integrations/pi --ignore-scripts
npm install --global --ignore-scripts ./edda-pi-session-channel-0.8.0.tgz
edda-pi --version
edda-pi runtime-info
```

Install the **tarball**, not the local directory: npm can link a directory back
to the development checkout. The tarball installs a copy and a normal npm binary
shim. This package is not yet published to npm; `npm install -g @edda/...` is not
an alternative. A distributed tarball can be installed without the source repo.
No lifecycle scripts, Pi settings edits, model calls or service restarts occur.
On Windows with PowerShell script restrictions, use `edda-pi.cmd`.
If the command is not found, check `npm prefix --global`: its directory must be
on PATH on Windows, or its `bin` subdirectory on Unix. Open a new shell after a
PATH change. Do not invent a private checkout path as the permanent entry.

`runtime-info` prints the installed client version, guide path, registry root,
Pi discovery and supported/unsupported capabilities. It creates nothing. If Pi
is not discovered, set `EDDA_PI_ENTRY` to your installed `dist/bundle/cli.js` or
supply `--pi-entry` at launch. `needs_pi` exits 2; installing this client alone
is not proof that Pi or a model is usable.

## 2. Choose the work and launch

Choose an existing project/worktree with no competing writer. Prepare a UTF-8
`task.txt` with the real goal, allowed scope, exclusions and expected result.
This example uses Flash; model availability/authentication are Pi's responsibility.
Replace the project and task paths; the client can run from **any** directory.

```text
edda-pi launch --project <absolute-project-path> --provider openrouter --model deepseek/deepseek-v4.1-flash --thinking high --prompt-file <absolute-task-file>
```

Launch may incur model usage. It creates one owned background runner, a Pi
session, an immutable integration release and an initial-message intent. It
prints a run ID before the launch effect; keep that ID if launch times out.
`--run-id <UUID>` allows a caller to retain the identity before invoking launch.
Identical same-ID launch inputs reconnect; different inputs conflict. Never
create another ID merely because the result is uncertain.

The returned **runId** selects lifecycle operations. The separate **sessionId**
selects replies/messages. `ready` means the runner is ready, not that work
succeeded. Read `initialReceipt` and Pi state before claiming work started.

## 3. A new shell or new controller finds the work

```text
edda-pi runtime-info
edda-pi runs
edda-pi run-status <runId>
edda-pi run-conversation <runId> --limit 20
```

`runs` reads the existing managed registry, including stopped runs. It is not a
live probe: `recordedPhase` can be stale and `observedLive` is deliberately null.
Use `run-status` for an authenticated current observation. Unreadable records
appear as unknown without hiding healthy entries or printing raw contents.
Use `runs --limit 100 --after <nextAfter>` to continue a page when `hasMore` is true.
No command in this section starts a session or calls a model.

Default registry: `~/.edda-pi-sessions`. If a deployment uses
`EDDA_PI_CHANNEL_DIR`, set the **same value** in later shells/controllers. It is
an explicit storage selection, not a search across every project or old runtime.
`runtime-info` and `runs` print the effective root so a wrong selection is visible.
An empty list is not proof that there are no sessions in other registries.

`run-status` shows the loaded integration release/version, selected model,
last progress, Pi state, initial receipt and recorded model errors. A historical
error may remain in the record after later activity; inspect current receipts
and public conversation. Heartbeats prove connectivity, not product progress.
`run-conversation` reads this run's validated persisted session, even while stopped;
it does not start Pi or search default transcript directories. It excludes private
reasoning and raw tool arguments/results. It reports the last persisted branch,
not unpersisted in-memory navigation. For a live in-memory branch use
`conversation <sessionId> --limit 20` instead. Both return a `conversation`
object with `entries`, `cursor`, `headCursor` and `hasMore`. Drain `hasMore` with
`--after <cursor>`; missing cursors refuse rather than silently switching branches.

## 4. Send a concrete next instruction

```text
edda-pi send <sessionId> --message-file <absolute-next-step-file> --sender controller
edda-pi receipt <sessionId> --id <messageId>
```

This continues the same conversation. Default `followUp` queues behind busy work;
`--mode steer` requests the next safe steering point. Sending may incur model
usage. Keep the printed message ID. On uncertainty query its receipt; do not mint
another ID and blindly repeat the instruction.

`started` means Pi observed the instruction. `settled` means that turn ended.
Neither is independent review, acceptance, or a merge grant. `failed`, `unknown`
and `unconfirmed` need inspection, not optimistic completion labels.

## 5. Stop and resume without replay

After confirming the run is idle:

```text
edda-pi run-stop <runId>
```

In a **new session**, use `runs` to recover the run/session IDs, then:

```text
edda-pi run-resume <runId>
edda-pi run-status <runId>
edda-pi run-conversation <runId> --limit 20
edda-pi send <sessionId> --message-file <absolute-next-step-file> --sender controller
```

Resume restores the same validated Pi session and pinned integration release.
It does **not** replay the initial task or send a generic "continue". The last
command is the explicit decision to continue after inspecting existing results.
If a run is already live, reconnect instead of making another writer. Idle stop
refuses busy work; `run-stop --abort` intentionally interrupts only this owned
child and must not be used as a routine polling/recovery action.

After an intentional stop, `run-status` reports `runner_unreachable` (exit 2)
with `lastRecordedPhase: stopped`; this is not a failed task. Its initial receipt
is read back from the channel record rather than the old launch snapshot.
If a runner is unexpectedly unreachable, inspect its run and persisted evidence. Resume refuses
when an old process may still be alive, a lock is ambiguous or the session is
missing/corrupt. Preserve the record. Do not delete locks or reset the registry
as an onboarding shortcut. Same-session recovery is not automatic crash recovery.

## Continue a session from a saved native capsule

When the necessary context was saved as an Edda native continuity capsule, adopt
it without restoring prose by hand or creating an intermediate context file:

```text
edda-pi adopt <sessionId> --task <taskId> --capsule <capsuleId> --scope "<declared scope>"
edda-pi adopt <sessionId> --task <taskId> --capsule <capsuleId> --context <declared-metadata-file> --scope "<declared scope>"
```

The client reads the capsule with the installed `edda continuity restore` and
merges the data-only restored context into the same bounded context path as
`--context`. Warnings, repository/Git provenance, the native read-back digest and
the `data_only` authority stay visible, and none of that text becomes execution
authority. The recorded prompt and initial task are never replayed. Add
`--context <file>` only when the declared `role`/`doneWhen`/`scope` metadata is
not already inside the capsule. A first adoption of a session that has no prior
enrollment also needs `--scope` (the capsule's `edda-management` block does not
substitute for it); later adoptions reuse the recorded scope. Unavailable, stale, wrong-repository, malformed
or oversize capsules fail with a `capsule_*` status (exit 2) before any
enrollment, handoff, message or model launch, so uncertain delivery is never
replayed. `--edda-bin <path>` (or `EDDA_BIN`) selects the native executable.

## What happens across lifecycle boundaries

| Boundary | Current supported behavior | Not implied |
|---|---|---|
| Launching shell exits | Owned runner/Pi remain; `runs` finds their IDs | Task completed |
| Worker finishes or errors | Public conversation, message receipts and Pi inbox retain evidence | Parent notified or result accepted |
| Controller changes | New controller uses this entry and the same registry; inspects existing work | Old controller authority transferred |
| Pi intentionally stopped | `run-resume`, inspect, then explicit message | Initial prompt replay or automatic work restart |
| Controller/Pi crashes | Recorded state visible; explicit same-session recovery where safe | Guaranteed automatic restart or power-loss recovery |
| Session file unusable | Preserve it; use `edda-pi adopt --capsule` or native Edda continuity when previously saved | Silent fresh worker or reconstructed private history |
| Working client upgraded | Existing runs keep their pinned runtime | In-place upgrade of running sessions |
| Workbench unavailable | These commands continue independently | Workbench is a required gate |

The controller owns decomposition, fixes, review and acceptance. The client owns
transport/lifecycle operations, not a fixed business workflow. A new controller
must read the actual task/brief and existing evidence; this guide is not a grant.
For portable necessary-context save/restore, use the existing Rust
`edda continuity --help` and its native carrier, not a new JSON schema.
Cross-machine process migration is not supported by this client.

## Optional observation, not another manager

`edda-pi inbox` / `inbox-read <eventId>` read durable Pi stopping evidence.
`follow <sessionId> --project <path> --tasks <ids> --scope <existing-scope> --notify`
can notify a **running** Pi about explicitly selected Edda task changes. It is
opt-in, may use model turns, and needs explicit refollow after Pi replacement.
It is not arbitrary child discovery, lossless owner wake or automatic restart.
No supervisor is required for launch, read, send or resume.

Managed fork, automatic owner wake, automatic process recovery and automatic
workbench registration remain unsupported by this entry. They are not steps the
user is expected to implement with a private shell script. Report a concrete gap
if your delivery requires them; do not claim them enabled by installing this CLI.

## Upgrade or remove

Install a new versioned tarball with the same npm command; `--version` and
`runtime-info` confirm the client. `run-status` confirms each actual loaded
runtime. Keep old releases and session evidence while runs depend on them.

```text
npm uninstall --global @edda/pi-session-channel
```

Removing the client neither stops managed processes nor deletes the registry.
Stop owned runs first if that is intended. Reinstalling the client lets you
inspect those runs again. No current project or service is migrated automatically.
