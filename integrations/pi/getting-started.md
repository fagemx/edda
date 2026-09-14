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

A managed project assistant is launched with a stable owner reference:

```text
edda-pi launch --project <absolute-project-path> --provider <provider> --model <model> --owner assistant/<project> --prompt-file <absolute-task-file>
```

The runtime records that owner reference and its current holder automatically; no
session id is shown to the user. `--owner-root <absolute-dir>` optionally pins
one shared owner mailbox root; it defaults to `<registry>/owner-mailbox`. The
assistant and every controller it launches under the same registry share that
mailbox, so the delegated `assistant/` and `controllers/<job>/` directories do
not need a common `.edda`/`.git` workspace root. A controller launch passes
`--return-owner assistant/<project>` (the owner its done/failed return is posted
against) and receives it as `EDDA_RETURN_OWNER`; if the assistant used a custom
`--owner-root`, the controller launch repeats it (or passes
`--owner-root "$EDDA_RETURN_ROOT"`) so both use the same mailbox.

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

`<sessionId>` is the Pi **channel session id**, read from `edda-pi run-status <runId>`
(the `sessionId` field) or the launch output — not the managed run id that a child sees
as `EDDA_SESSION_ID`. When the target session lives in another registry, address it explicitly with
`--registry <dir>` (or set `EDDA_PI_CHANNEL_DIR` for the call); a missing session names the registry used
instead of a bare "no reachable registered owner".

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

## 5. Owner-bound results across assistant replacement

A delegated job's completion is posted against the stable owner reference, not a
session id:

```text
edda return post --owner assistant/<project> --work <job> --status done|failed --result "<one line>" --message-file <report.md> --session <controller-session>
```

The managed runtime binds the owner reference at launch. When the assistant is
replaced, the replacement launch rebinds explicitly from the persisted owner
record, and pending returns are claimed on the **next natural live turn**, exactly
once. There is no offline wake, timer or scheduler: a claimed return is presented
once and a superseded holder cannot claim it again. The launcher pins one mailbox
root through `EDDA_RETURN_ROOT` (default `<registry>/owner-mailbox`), inherited
by every controller it launches, so the post and the claim meet even though the
assistant and controller are different project directories. An older installed
`edda` without the `return` verb exits non-zero, where the session-addressed path
above still works for the unchanged same-session case.

A managed run launched **without** `--owner` starts **session-addressed** only: `edda-pi run-status` reports
`owner: null` and `continuity: 'session-addressed'`. Adopt it into the owner lifecycle without relaunching:

```text
edda-pi owner adopt --run <runId> --owner assistant/<project> [--return-owner REF] [--owner-root DIR]
```

The runner persists the owner and the runtime claims pending returns on the next natural live turn — the same
run and session are preserved (no restart, no replayed prompt). Until adoption, `edda-pi send` is the only
continuity and it is session-addressed, not owner-bound.

## Hand-opened sessions are not wired automatically

A session that Edda did not launch has no managed runner, so nothing records an owner holder or claims
returns for it. To use the owner mailbox or dependency observation there:

1. Load the **installed** extension by its exact path, printed by `edda-pi runtime-info` as
   `extension.path` (with its `digest` and `version`) — for example `pi -e <extension.path>`.
2. Set `EDDA_OWNER_REF=<owner-reference>` in the session environment so the runtime can record the holder
   and claim pending returns on the next natural live turn.

`edda-pi runtime-info` also reports the installed `channel` module and the digest-verified installed
`releases` (`id`, `version`, `channel`, `extension`, `verified`). A live session reports its loaded channel
as `integration.modulePath` in `edda-pi list`; if it matches neither `runtime-info.channel.path` nor a
verified `runtime-info.releases[].channel`, the session loaded a stale or foreign copy and its owner-mailbox
pickup fails silently. Re-open it with the installed extension path, or reinstall so a single installed copy
is loaded.

## 6. Stop and resume without replay

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
replayed. The raw `restore` envelope is read under a 512 KiB bound (a larger one
fails as `capsule_too_large`), and the repository-membership listing under a 16 MiB
bound (a larger history fails as `capsule_unavailable`, membership unverifiable).
Native repository scoping always includes `legacy_partial` projections regardless
of repository id, so a legacy-partial capsule is surfaced with its warning rather
than refused as wrong-repository. `--edda-bin <path>` (or `EDDA_BIN`) selects the native executable.

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
can notify a **running** Pi about explicitly selected Edda task changes. `<sessionId>` is the Pi
**channel session id** from `edda-pi run-status <runId>` (the `sessionId` field) or the launch
output, not the managed run id a child sees as `EDDA_SESSION_ID`. It is
opt-in, may use model turns, and an owner-bound subscription survives session
replacement without a manual refollow: the subscription is stored under the
stable owner/work identity, a replacement holder adopts it and resumes
observation. Producer revision/done signals are read through the supported
`edda task show` API, never by reading a sibling `.edda/returns` mailbox file.
No scheduler or polling was added; observation still checks on its existing
polling interval and before delivery. It is not arbitrary child discovery or
automatic restart. No supervisor is required for launch, read, send or resume.

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
