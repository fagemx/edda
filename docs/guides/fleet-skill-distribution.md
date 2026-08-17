# Fleet skill distribution

The repository directory `skills/fleet-orchestrate/` is the sole canonical
source. Global installation is explicit: ordinary `edda init`, including its
skill setup, does not copy or update `fleet-orchestrate` in a user's profile.

On Windows, inspect both supported installs without writing anything:

```powershell
pwsh -NoProfile -File scripts/skills/sync-fleet-orchestrate.ps1 `
  -Action Status -Target All
```

The defaults are:

- Codex: `%USERPROFILE%\.agents\skills\fleet-orchestrate`
- Claude: `%USERPROFILE%\.claude\skills\fleet-orchestrate`

Use `-Target Codex`, `-Target Claude`, or `-Target All`. `Status` never writes.
`Sync -DryRun` reports the intended operation without writing, and `-Json`
returns an array of structured results suitable for release checks:

```powershell
pwsh -NoProfile -File scripts/skills/sync-fleet-orchestrate.ps1 `
  -Action Sync -Target All -DryRun -Json

pwsh -NoProfile -File scripts/skills/sync-fleet-orchestrate.ps1 `
  -Action Sync -Target All
```

Each target is classified before mutation:

- `missing`: no installed directory exists;
- `current`: its complete manifest equals canonical;
- `stale`: its bytes match the recorded installed digest or an exact historical
  canonical tree in the current Git repository;
- `locally-modified`: no trusted provenance or historical canonical match
  proves that replacement is safe.

The manifest sorts every relative file path ordinally and records its byte
length and a .NET SHA-256. The script does not depend on `Get-FileHash`.
Repository history is read only for safe bootstrap of older unmarked installs;
when Git or relevant history is unavailable, a differing target is classified
`locally-modified`. Historical comparison folds CRLF to LF only for valid
UTF-8 text, covering Git checkout normalization while retaining exact matching
for binary files. Marked installs always use their exact recorded byte digest.

Sync stages the complete canonical tree in a unique sibling directory, checks
manifest parity, then renames it into place. The adjacent
`fleet-orchestrate.edda-provenance.json` marker records the canonical and
installed digests without changing the installed directory manifest. A
locally modified target is refused unless `-Force` is explicit; forced sync
renames the previous directory to a reported sibling backup before installing.

Fixture callers can override `-CanonicalPath`, `-CodexPath`, and `-ClaudePath`.
`-StagingPath` is limited to one target and must name a unique
`fleet-orchestrate.edda-staging-*` sibling; the self-test uses it only to prove
that staging failure preserves the existing target.

Run the offline acceptance check with:

```powershell
pwsh -NoProfile -File scripts/skills/self-test-fleet-orchestrate-sync.ps1
```

The test creates only temporary fixtures, including a tiny local Git history;
it never reads from or syncs to the real Codex or Claude install directories.
