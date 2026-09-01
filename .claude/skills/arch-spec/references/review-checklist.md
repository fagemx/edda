# Review Checklist — Complete Flow for arch-spec review

## When to Use

```
arch-spec review <path to spec directory>
```

---

## Per-File Check

Check each spec file for:

### Structural Completeness
- [ ] Has a "one-liner definition"? (Section 1)
- [ ] Has a "what it's NOT / most common mistakes" section? (Section 2)
- [ ] Has canonical form / flow / schema? (Sections 4-5)
- [ ] Has at least 2 concrete examples? (Section 7)
- [ ] Has explicit boundaries (in-scope / out-of-scope)? (Section 8)
- [ ] Has a closing line? (final sentence)

### Type Quality
- [ ] TypeScript types (not prose descriptions)?
- [ ] Enums use typed unions (not generic `string`)?
- [ ] v0 uses structured verdicts (not 0-100 scores)?
- [ ] Shared types reference shared-types.md (not redefined)?

### Content Quality
- [ ] Each file answers only one core question?
- [ ] Examples are concrete JSON/TypeScript (not "imagine X")?
- [ ] ASCII diagrams used for flow / state machines (not just prose)?

---

## Cross-File Check

Check across all spec files:

### Type Consistency
- [ ] Same concept uses the same field name in all files?
- [ ] No type is defined separately in 2+ files (should be in shared-types)?
- [ ] shared-types.md covers all cross-file types?
- [ ] No orphan types (defined but never used)?

### Terminology Stability
- [ ] First-class entity names are consistent across files?
- [ ] No synonym drift (e.g., `missingFields` vs `keyUnknowns`)?

### Boundary Clarity
- [ ] No spec is doing another spec's job?
- [ ] Adjacent specs cross-reference each other ("see X.md")?
- [ ] No dangling references (referencing non-existent files or types)?

### Flow Alignment
- [ ] Canonical form and API routes align?
- [ ] Schema lifecycle and API state transitions align?
- [ ] Demo path covers the main artifacts?
- [ ] Canonical slice dot-paths and schema diff operations align?

### Promotion Readiness
- [ ] Terminology is stable (no name changes in last 2 discussions)?
- [ ] Canonical form exists?
- [ ] Canonical slice exists?
- [ ] At least one closure can be demonstrated?
- [ ] See `promotion-rules.md` for full checklist

---

## Output Format

```markdown
## Spec Stack Review: <name>

### Per-File Health
| File | Structure | Types | Examples | Boundaries |
|------|-----------|-------|----------|------------|
| overview.md | ok | ok | ok | ok |
| schema-v0.md | ok | conflict | missing | ok |

### Cross-File Issues
| # | Type | Files | Detail |
|---|------|-------|--------|
| 1 | type conflict | A.md, B.md | `fieldX` is string in A, number in B |
| 2 | orphan type | shared-types.md | `FooType` defined but never referenced |
| 3 | boundary leak | router.md | doing path-check's job (likelyNextStep field) |

### Promotion Status
| Criterion | Status |
|-----------|--------|
| Names stable | yes / no |
| Canonical form exists | yes / no |
| Canonical slice exists | yes / no |
| One closure demonstrated | yes / no |
| shared-types consolidated | yes / no |

### Verdict
**<HEALTHY / NEEDS FIXES / MAJOR GAPS>** — <one sentence>
```
