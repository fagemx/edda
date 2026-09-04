# GH-609 signing spike — Ed25519 event signing with Node built-in crypto

Design + demonstrable spike for
[GH-609](https://github.com/fagemx/edda/issues/609). **Not a production
signature rollout** — see `docs/architecture/actor-signing.md` for the threat
model, the envelope extension spec (for #608), and the migration plan.

## Run

```bash
node scripts/spikes/identity/test.js    # 13 tests; exit 0 = all invariants hold
node scripts/spikes/identity/spike.js   # narrative demo (stages A–E)
```

Requires Node >= 20 (`node:test`, `node:crypto`). **Zero dependencies** —
nothing to install, nothing added to any Cargo crate.

## Safety properties of the spike itself

- All keys are **ephemeral**, generated in-process per run, never written to
  disk, never committed. No operator keys or credentials are involved.
- `fixtures/golden-events.json` contains only event envelopes and their
  SHA-256 hashes — no keys.
- `lib/rfc8032.js` contains the public test vectors from RFC 8032 §7.1 —
  published constants, not secrets.

## What each part proves

| Part | Proves |
|---|---|
| `lib/rfc8032.js` | Node's Ed25519 reproduces RFC 8032 §7.1 vectors byte-for-byte (primary-source check, fetched from rfc-editor.org 2026-09-04) |
| `lib/canon.js` | Node mirror of `edda-canon-v1` (`crates/edda-core/src/canon.rs`) |
| `fixtures/golden-events.json` | Events whose `hash` came from the **actual Rust algorithm** (edda 0.4.0 binary, isolated `EDDA_STORE_ROOT`) |
| `test.js` stage B / `spike.js` stage B | Node reproduces the Rust hashes exactly — the cross-language canonicalization guard |
| `test.js` stage C | **FAIL-FIRST**: a forged author string is self-consistent and ACCEPTED under the unsigned baseline (today's envelope) |
| `test.js` stage D | The same forgery is REJECTED with signed verification |
| `test.js` embedded-key test | An attacker re-signing with their own key and embedding it in the event is rejected — verification is keyring-first, never embedded-key-first |
| `test.js` sig-exclusion tests | `sig` is outside its own signing input and outside the hash; Ed25519 re-signing is deterministic |
| `test.js` actor/key binding | The hash binds `actor_id`/`key_id`; swapping identity breaks verification; the keyring resolves `(actor_id, key_id)` as a pair, fail-closed |
| `test.js` authority tests | A valid agent signature still cannot ratify — signature validity ≠ authority; only operator-role keys ratify |
| `test.js` legacy tests | Unsigned events remain ledger-legal as the legacy tier (hash-chain integrity, no authority); mixed signed/legacy chains verify |

## Layout

```
lib/canon.js        edda-canon-v1 canonical JSON (Node mirror of canon.rs)
lib/signing.js      event hash, sign, keyring-first verify, ratify authority, chain
lib/rfc8032.js      RFC 8032 §7.1 test vectors + conformance check
fixtures/golden-events.json   Rust-produced golden events + provenance
test.js             assertion runner (node:test)
spike.js            narrative demo
```
