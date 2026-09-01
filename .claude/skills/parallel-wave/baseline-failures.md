# RED: baseline failures (observed, not synthetic)

Real controller behavior on this workstation WITHOUT this skill, 2026-08-31 ~ 09-01:

1. **Serial by default.** cleanup-wave-b.yaml was authored as a 5-phase chain with
   declarative `depends_on` (b1→b2→b3→b4→b5) although the file surfaces were
   disjoint. Rationalization: "序列跑，先 B 線" — chain order felt safer; nobody
   asked whether the dependency was real.
2. **Review line idle.** PR #553 and #554 sat 7/7-CI-green with zero reviews while
   the implementation line kept running. Review dispatch waited for "the rhythm"
   instead of the PR-open event.
3. **Gate assumes attached controller.** wave1: 2 of 3 verdict gates timed out at
   7200s (defects #551, #552). Parallel plans must not carry verdict gates.
4. **Ad-hoc build dirs.** Historical: 15 ad-hoc CARGO_TARGET_DIRs, ~194 GB
   (audited; CLAUDE.md Build lanes). Pressure: "fresh session should build in its
   own directory to be safe."
5. **Duplicate-work window when splitting.** When b4/b5 were split out of the
   running serial plan, forgetting `conduct skip` first would have had the serial
   runner re-execute them after b3. The safe order (skip → launch) is not obvious.
6. **Same-file overlap rationalized away.** Temptation observed: "codex_rpc.rs is
   touched by both, but they'll probably not conflict — just brief both." Correct
   handling is conditional parallel with FORBIDDEN symbol lists, or serial.

GREEN criterion: a fresh agent given a mixed wave (disjoint + same-file +
same-symbol + vague issues) must produce: correct 4-way judgment, worktree+lane
isolation, no gates, skip-before-launch when splitting, review dispatch on
PR-open, serial merge tail with rebase-only-on-intersection.
