# Reference: TRACKS.md — Single Track Entry

> This is a real track definition from a production project.
> Use this as the quality bar when generating each track in TRACKS.md.

---

## Track A: Ledger + Blob Store

**Layer**: L0
**Goal**: Build gctx's core infrastructure — Event model + canonical JSON + hash chain + append-only ledger + blob store + workspace lock

**Input**:
- EVENT_SCHEMA_v0.1 (Event fields, hash rules, blob store rules)
- RUN_LAYOUT_v0.1 (`.gctx/` directory structure)

**Output**:
- `gctx-core` crate (types, canon, hash, event builders: note/cmd)
- `gctx-ledger` crate (paths, lock, ledger append, blob put/get)
- `gctx-cli` crate (init/note/run — 3 basic commands)
- `gctx init` can initialize `.gctx/` workspace
- `gctx note/run` can write to events.jsonl + blobs

**Dependencies**:
- blocks: B, C, D, E, F, G, H
- blocked-by: none (can start immediately)

**DoD**:
- [ ] `cargo build` zero errors
- [ ] `gctx init` creates complete `.gctx/` directory structure
- [ ] `gctx note "test" --tag todo` writes to events.jsonl with correct hash chain
- [ ] `gctx run -- echo hello` writes cmd event + stdout/stderr blob
- [ ] hash chain is continuous (each parent_hash = previous event's hash)
- [ ] blob content-addressing is correct (sha256(bytes) = filename)

**Smoke Test**:
```bash
cargo build
cargo run -p gctx-cli -- init
cargo run -p gctx-cli -- note "test" --tag todo
cargo run -p gctx-cli -- run -- echo hello
cat .gctx/ledger/events.jsonl | head -3
ls .gctx/ledger/blobs/
```

**Task Count**: 3

---

> **Pattern notes for generation:**
> - Every track has exactly these sections: Layer, Goal, Input, Output, Dependencies, DoD, Smoke Test, Task Count
> - **Goal** is one sentence describing what this track delivers (not how)
> - **Input** lists what must exist before this track starts (specs, other tracks' output)
> - **Output** lists concrete artifacts — crates/modules/files/commands that will exist when done
> - **Dependencies** uses two sub-fields: `blocks` (what this unblocks) and `blocked-by` (what must finish first)
> - **DoD** is a checkbox list of verifiable checks — each with a specific command or observable result
> - **Smoke Test** is a copy-pasteable bash script that proves the track works end-to-end
> - No vague DoD items like "works correctly" — every item has a verification method
