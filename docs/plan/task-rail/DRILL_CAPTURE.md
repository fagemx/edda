# Drill process capture

Use `scripts/drill/capture.ps1` for real-process drill evidence. It uses only
PowerShell and .NET, starts children with `ProcessStartInfo.ArgumentList`, and
never kills a process.

Run the offline gate first:

```powershell
pwsh -NoProfile -File scripts/drill/self-test.ps1
```

The test uses disposable files and local child shells only. It does not invoke
Edda, Scheduler, Codex, a network, PID termination, or a repository command.

## Single process

```powershell
. ./scripts/drill/capture.ps1
$capture = Invoke-DrillCapture `
    -CaptureId 'D1-controller' `
    -Owner 'task-84-attempt-1' `
    -Executable 'C:\absolute\path\controller.exe' `
    -WorkingDirectory 'C:\absolute\path\fresh drill repo' `
    -ArgumentList @('reconcile', '--max-workers', '1') `
    -OutputDirectory 'C:\absolute\path\raw'
```

`CaptureId`, `Owner`, literal executable, and existing cwd are required before
the child can start. Treat PID plus process creation time, executable, argv,
cwd, and owner as the ownership tuple; PID alone never authorizes a process
action.

Each stage is a new atomically renamed file. Existing stage files are never
overwritten:

- `*.00-planned.json` is durable before any child starts.
- `*.10-started.json` records PID and the OS creation-time query. A fast exit is
  recorded as `exited_before_identity_query_recovered` or
  `exited_before_identity_query_unavailable`, not treated as missing evidence.
- `*.stdout.txt` and `*.stderr.txt` are bounded; the terminal record carries
  total characters, truncation, and .NET SHA-256 hashes.
- `*.20-terminal.json` retains owner, timing, signed exit, identity, and output
  evidence.
- `*.30-optional*.json` is written only after the required terminal record.
  UInt32 hex is optional metadata and is converted from the Int32 bytes, so a
  signed `-1` is safe without making hex an acceptance requirement.

An optional formatter may be supplied with `-OptionalMetadata`. If it throws,
the terminal record remains intact and the helper records an
`OPTIONAL_METADATA` failure.

## Concurrent processes

Pass all child specifications to one call:

```powershell
$result = Invoke-DrillCaptureSet -SetId 'D4-controllers' -Spec @(
    @{ capture_id='A'; owner='task-84'; executable=$exe; cwd=$repo; argv=$argv },
    @{ capture_id='B'; owner='task-84'; executable=$exe; cwd=$repo; argv=$argv }
) -OutputDirectory $raw
```

The helper writes every planned record, launches every child, and only then
queries either identity. The set terminal record proves overlap when the
latest OS process start is earlier than the earliest OS process exit. Missing
OS interval data or `overlap=false` is not concurrency proof.

## Receipt classification

Use one exact label for every non-pass result:

| Label | Meaning and action |
| --- | --- |
| `PRODUCT_RED` | Trustworthy evidence contradicts product acceptance; stop the product path. |
| `SAFETY_RED` | Ownership, cleanup, or another safety invariant is not proven; stop before any PID action. |
| `HARNESS_RED` | Required capture or evidence failed; do not relabel it as product RED. |
| `OPTIONAL_METADATA` | Nonessential formatting or redundant metadata failed; it is nonblocking when planned, started, and terminal ownership/timing evidence remains. |

## Phase isolation

Default to a fresh disposable repository and ledger for each drill phase when
planner state can affect the result. In particular, do not reuse a ledger when
earlier terminal attempts, failed tasks, leases, or stale heartbeats can change
later `max-attempts`, capacity, or eligibility checks. Preserve each phase's
repo/ledger as evidence, but start the next contaminated phase from a fresh
pair.
