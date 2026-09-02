# gh594 micro-test — wiring verdict slot (issue #594, PR #629)

Control vs variant review of the same fake diff (`fake.diff`), prompts
identical except the review-skill text: control embeds fleet-review from
`origin/main` (`git show origin/main:.claude/skills/fleet-review/SKILL.md`),
variant embeds this branch's (`git show HEAD:.claude/skills/fleet-review/SKILL.md`,
content-identical to the worktree file modulo line endings). Model
`z-ai/glm-5.3-flash`, read-only (`--exclude-tools edit,write`), N=3 runs per
arm, session ids `microtest-control-<ts>-<i>` / `microtest-variant-<ts>-<i>`.

Replay: `sh tests/microtests/gh594/run.sh` — overwrites
`out-control-1..3.md` / `out-variant-1..3.md`. The committed outputs are from
run `20260902-151034`.

The fake diff is modeled on the real miss the slot exists for (commit
`5abbfb7`, `with_model`): surfaces that look wired — a stored field, a
plausible setter, a unit test asserting the field — but that reach no reader
on the production path (the unchanged context shows `spawn_command()` pushing
`--agent`/`--cwd`/`--budget-usd`/`--heartbeat` and never `--model`), plus a
swallowed write on a receipt path (`fs::write(...)?` → `if let Err(e) ... {
tracing::debug!(...) }`, debug-level only, no comment).

## Scoring rule (applied verbatim to every run)

- A run **catches A** only if it states that `model` / `with_model` is never
  read on the spawn/command path or has no production consumer. A generic
  "add docs/tests" remark is a miss.
- A run **catches B** only if it flags the swallowed write error as a defect
  that must fail or signal (P1-level). Merely mentioning the log line is a
  miss.

## Result (run 20260902-151034, quoted line scored on per run)

| Run | Defect A (no production reader / layer reach) | Defect B (swallowed write on report path) |
|---|---|---|
| control-1 | caught — "spawn_command() 只轉發 `--agent`/`--cwd`/`--budget-usd`/`--heartbeat`，沒有任何 `--model` 或等價參數" | caught — "receipt 寫入失敗被吞掉，仍回傳 `Ok(從未寫入的路徑)` — 靜默證據遺失"（P0） |
| control-2 | caught — "`model` 欄位加了但完全沒接上 `spawn_command()`…完全忽略 `self.model`——沒有對應的 `--model` 旗標" | caught — "receipt 寫入失敗被靜默吞掉，回報成功卻回傳不存在的路徑"（P0） |
| control-3 | caught — "`model` 欄位加了但沒接線…卻沒有輸出 `--model`" | caught — "收據寫入失敗被靜默吞掉，且仍回傳 `Ok(path)`"（P0） |
| variant-1 | caught — "`with_model` 是靜默 no-op——旗標從未到達 spawn 層…未輸出 `--model` argv"（P0）＋ P1「no consumer → dead on arrival」依判定規則 | caught — "receipt 寫入失敗被吞…這是 ledger/coordination 路徑上的吞錯（wiring 規則下限即 P1）"（P0） |
| variant-2 | caught — "`model` 新功能整體未接線…argv 與未設 model 時逐 byte 相同"（P0）＋ P1「no consumer＋無後續 issue → dead on arrival」 | caught — "round receipt 寫入失敗被吞，回傳謊言般的 `Ok(path)`…同時踩中 wiring 條款「ledger 路徑上吞錯」"（P0） |
| variant-3 | caught — "`model` 欄位與 `with_model` builder 是 dead on arrival…`spawn_command()` 完全沒讀 `self.model`"（P1，依寫死規則） | caught — "round receipt 寫入失敗被靜默吞掉，且以成功姿態回傳"（P0） |

**Control 3/3 · Variant 3/3 (A), Control 3/3 · Variant 3/3 (B).**

## Honest reading

**Control caught both defects 3/3.** Per the fix brief, this outcome is
reported and the diff was not retuned after the first N=3 run; the controller
rules on what it means. This experiment does **not** establish that detection
comes from the slot: glm-5.3-flash finds both defects on this diff with or
without it.

What was observed, without claiming more:

- **3/3 variant runs emitted the Wiring table** (the mandatory four-question
  per-surface section); 0/3 control runs did — the control has no such
  section to emit. The variant's findings were additionally tied to the fixed
  severity rules by name (「no consumer → P1 dead on arrival」、「ledger 路徑
  吞錯 → P1」、layer-reach 斷鏈), which is the structure/consistency the slot
  buys; the control reached similar conclusions by free-form reasoning.

Single-run or small-N repeatability claims are not made here; N=3 per arm
with one cheap model is not evidence of a detection differential, only that
this diff does not separate the arms.

Earlier runs against the giveaway first version of the diff (a `//
best-effort` comment and an orphan setter) are superseded by this directory.
