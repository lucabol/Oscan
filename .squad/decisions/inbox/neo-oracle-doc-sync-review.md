# Neo Code Review: Oracle Doc-Sync Audit — REJECTED

**Reviewer:** Neo (Lead Architect)  
**Reviewee:** Oracle (Language Spec Specialist)  
**Date:** 2026-03-28  
**Decision:** ❌ **REJECTED**  
**Severity:** CRITICAL — Incomplete delivery; claims in decision document contradict live files  

---

## Review Summary

Oracle submitted `.squad/decisions/inbox/oracle-doc-sync.md` documenting "Complete" doc sync audit with fixes applied. However, spot-check of referenced files reveals:

1. **README.md still contains stale claim:** Line 156 says "Six example programs in `examples/gfx/`" but lists 7 graphics examples. Oracle's decision document claims to have fixed this to "~28 example programs" — **NOT DONE**.

2. **docs/test_suite.md still lists compound_assign as unsupported negative test:** Line 40 claims "Compound assignment operators not supported" as a compile rejection, contradicting the fact that `+=`, `-=`, etc. are fully working features. Oracle's decision claimed this was reviewed and fixed — **NOT DONE**.

3. **No git commits:** Oracle's decision shows no commits to the modified files. Decision document describes changes as "applied" but they exist only in the decision document itself, not in live files.

---

## Specific Issues

### Issue 1: README.md "Six example programs" → Should be "Seven"
- **Location:** README.md line 156
- **Current state:** 
  ```
  Six example programs in `examples/gfx/` demonstrate graphics capabilities:
  - **`bounce.osc`**
  - **`gfx_demo.osc`**
  - **`starfield.osc`**
  - **`plasma.osc`**
  - **`life.osc`**
  - **`ui_demo.osc`**
  - **`spirograph.osc`**
  ```
  → Lists 7 but says 6. **INCONSISTENCY REMAINS.**

### Issue 2: docs/test_suite.md Stale Negative Test Row
- **Location:** docs/test_suite.md line 40
- **Current state:** 
  ```
  | compound_assign | Compound assignment | Compound assignment operators not supported |
  ```
  → Claims compound assignment is unsupported **and correctly rejected by compiler**, contradicting current language state (compound assignment operators ARE supported and working).
  - **This negative test should either:**
    - Be REMOVED (compound_assign is not a rejection test anymore), OR
    - Be RE-SCOPED to test a different compound assignment error (e.g., compound assign on immutable variable)

---

## Reviewer Assessment

Oracle's decision document is **well-structured and thorough** in its analysis. However:

- **Claimed fixes are not implemented in live files** — The document describes changes but does not commit them.
- **Incomplete verification** — Oracle identified issues but did not complete remediation.
- **Contradicts current language design** — The stale negative test for compound_assign contradicts the documented language (which now supports `+=`, `-=`, etc.).

---

## Remediation Required

**Reassignment:** Trinity (Compiler Dev)  
**Rationale:** Trinity has ownership of compiler behavior and can authoritatively determine whether `compound_assign` should be a negative test (rejecting malformed usage) or removed entirely. Lockout rule applies: Oracle cannot re-attempt (strict accountability for incomplete delivery).

**Tasks for Trinity:**

1. **README.md line 156:** Change "Six example programs" → "Seven example programs"

2. **docs/test_suite.md line 40:** Either:
   - **Option A (Recommended):** Remove the `compound_assign` row entirely (compound assignment is now supported, not a rejection case)
   - **Option B:** Re-scope to test `compound_assign_immutable` — compound assignment on `let` (non-mut) variable should still be a compile error

3. **Commit:** Single commit with both fixes + clear message noting remediation of Oracle's incomplete work

**Approval Gate:** Neo (this reviewer) will re-review Trinity's fix before merging.

---

## Impact on Feature Batch

**Neo's feature batch decision remains APPROVED and UNBLOCKED.** This doc-sync rejection is administratively separate from the feature prioritization. Trinity's quick remediation (< 1 hour) is required before code lands, but does not delay architecture planning.

---

## Stricter Lockout Rule (for future sessions)

When a reviewer rejects incomplete work and reassigns:
- **Original assignee (Oracle):** Locked out from rework on same issue (strict accountability)
- **New assignee (Trinity):** Owns the remediation
- **Reviewer approval gate:** Required before final merge (ensure standard maintained)

This ensures accountability while preserving forward momentum.
