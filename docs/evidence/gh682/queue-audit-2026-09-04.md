# GH682 queue audit — 2026-09-04

This audit follows the actual reader in `scripts/fleet-claim-issue.sh`: fetch
every repository issue comment, normalize CRLF, split it into lines, trim each
line's leading whitespace, and select an issue when any resulting line starts
with `taking:`. A separate `RELEASED` line does not suppress a selected line;
a struck-through `~~taking: ...~~` line does not match.

The GitHub REST scan returned **65** candidate issues. It is intentionally not
limited to `fleet:claimed`, because the defect is a surviving `taking:` comment
after that label has been removed.

## Disposition

- **56 closed issues:** #558, #564, #565, #574, #584, #585, #593, #599, #601,
  #606, #608, #609, #617, #631, #633, #647, #649, #650, #661, #663, #665,
  #666, #667, #669, #672, #678, #680, #701, #703, #704, #705, #706, #710,
  #712, #714, #715, #729, #730, #731, #734, #741, #744, #747, #750, #751,
  #757, #775, #776, #779, #783, #784, #789, #792, #797, #801, and #820.
  Their claims are closed-issue history and were not rewritten.
- **8 confirmed live open claims:** #671 (4090/worker-1), #682
  (4090/economy-fleet-terra), #693 (4090/economy-fleet-terra), #746
  (worker-2), #761 (docs/worker-1), #764 (docs/worker-1), #765
  (docs/worker-2), and #800 (4090/infra-controller). They were not changed.
  `lane:*` remains routing-only, including #761 and #764's `lane:4090`.
- **1 open claim with unproven present ownership:** #690 has an older
  `taking: 4090/lane-gh690` comment but no current `fleet:claimed` label. The
  audit neither calls it released from age nor withdraws it; its owner must
  reconcile it before a future dispatch.

The separately known released-without-delivery claims #591, #634, #650, and
#651 already contain explicit `RELEASED` withdrawals. They no longer match the
active-line selector and were preserved. The only proven current label
contradictions were `fleet:pending` plus `fleet:claimed` on #746 and #800;
only `fleet:pending` was removed from those two issues.
