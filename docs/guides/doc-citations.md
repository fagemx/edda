# Verifiable source citations

Run `bash scripts/lint-doc-citations.sh --tree` for tracked working-tree
documents. The pre-commit hook runs `--staged` against a complete index
snapshot, including unchanged documents. A source-only edit or deletion can
invalidate a citation, and an unstaged repair must not conceal it.

Use repo-root paths in inline code:

```text
`crates/example/src/lib.rs:12`
`crates/example/src/lib.rs:12-18#pub fn execute(`
`crates/example/src/lib.rs#pub enum Outcome {`
```

The first form checks that the tracked target exists and the range is within
its line count. The second also requires the literal text after `#` on the
cited range. The last searches for literal text anywhere in the file,
so a named declaration survives unrelated insertions. Anchors are literal
substrings, not regular expressions or a Rust symbol resolver; use a specific
declaration rather than a common word. Bounds alone cannot detect in-bounds
drift, and no form verifies prose meaning or the end of a declaration.

The gate enumerates tracked Markdown and `///` / `//!` comments under
`crates/`. Numbered root paths under `crates/`, `scripts/`, `docs/`, `tests/`
and `.github/` are checked; `edda-<crate>/...` is explicitly crate-anchored
under `crates/`. Anchored paths are checked anywhere. Plain Rust source paths
starting with `crates/` or `tests/` in doc comments are checked from the repo
root too. Generated file names, Rust member names and contextual basename
shorthand in older prose are not treated as root citations. `COMPATIBILITY.md`
has no such shorthand exception: its numbered citations all use full paths
and anchors.

Fenced examples and plain paths in Rust paragraphs labeled `E.g.,` or
`Example:` are examples, not assertions. Markdown under `tests/` is recorded
test input/output, so only explicitly anchored citations there assert a live
target. These are syntax/content categories, not a baseline of waived
violations. File paths in the citation syntax use ASCII letters, numbers,
underscores, dots, slashes and hyphens. Anchors cannot include backticks or
span lines.

`bash scripts/test-doc-citations.sh` exercises missing files, bounds, literal
anchors, doc comments, and index/worktree divergence in disposable Git repos.
With `--history`, it additionally reads the actual objects at `93eceee`,
`0a94ecd` and `fb6ab1b` (full Git history required). Those documents predate
anchor syntax: the test preserves their numeric citation ranges and target
blobs, adding the semantic anchor used by today's contract. It isolates the
dispatch long-help citation when comparing the first two commits because
both contain separate dispatch citation drift. The third commit checks all
three reported dispatch ranges independently. Range containment matters:
the fixed `0a94ecd` range starts with a blank doc-comment line, followed by
the long-help anchor; the broken `93eceee` range ends before that anchor.

PR-body counts, old SHA receipts and prose that understates a test remain
review concerns. This gate makes no claim to verify them.
