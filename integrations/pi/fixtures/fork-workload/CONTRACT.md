# Cross-project status overview — experiment contract v1

Implement three independent pure ESM modules, no dependencies, no side effects,
no mutation of input. This is an isolated prototype of a cross-project read-only
view over Edda supervisor records, not a production wire format change.
Only your assigned module may be edited. Tests and this contract are immutable.
The controller runs acceptance externally and supplies any failure for repair.

## cards.mjs
Export normalizeCard(config, state, now, staleAfterMs = 60000).
config is {id, project, tasks:[{id}]}. state can be null or an object containing
{phase, updatedAt, tasks:[{id,status}], needsAttention:boolean}.
now is finite milliseconds since epoch. Return exactly
{id, project, phase, stale, needsAttention, counts:{total,ready,running,done,unknown}}.
Accept phase only starting/running/paused/tasks_done/failed; otherwise unknown.
stale is true if updatedAt is not a valid ISO date string, is in the future, or
now - parsed time > staleAfterMs. Equality is fresh. Missing state is stale.
Only distinct config task IDs count, preserving JS strict identity. Last matching
state task for an ID wins. Status ready/running/done count in those categories;
anything else counts unknown. Ignore state tasks not selected by config.
needsAttention is true exactly if state.needsAttention === true OR phase === failed.
Never infer needsAttention or acceptance from stale or all tasks done.

## attention.mjs
Export rankCards(cards). Deduplicate by card.id, keeping the last occurrence.
Return a new array of those card objects (same references). Order ascending by:
needsAttention true first, then phase failed first, then stale true first,
then running count descending, then id by JavaScript code-unit string order.
Only boolean true activates boolean flags. Do not use locale-dependent ordering.

## overview.mjs
Export renderOverview(cards, maxBytes). Input is already ordered normalized cards.
Return a JSON STRING with exactly these keys in this order:
{total,shown,hasMore,cards}. total is original input length; shown is included count;
hasMore is shown < total; cards is the longest PREFIX fitting in maxBytes UTF-8
bytes, including envelope. Do not truncate fields, split a card, or skip a large
first card to include later cards. Empty cards is allowed. If even an empty
envelope cannot fit, throw RangeError. maxBytes must be a nonnegative safe integer,
otherwise throw RangeError. Do not mutate cards. No pretty-printing.

## Integration
normalize each project, rank the cards, render a bounded overview. A done task
count is execution status, not independent review/merge acceptance. No commands,
approvals, notifications or dispatching belong in these pure functions.

## Worker authority
Implement only the assigned file now. Operator already authorizes edits and
controller-run validation. Do not ask again, spawn agents, install packages,
commit/push, use the network, or change other files. Finish with a short factual
summary. Inherited history is background; current assigned scope takes priority.
