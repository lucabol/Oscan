---
title: Example interpolation review gate
author: Tank
date: 2026-04-01
status: superseded
---

## Summary

Initial reviewer pass rejected the sweep because `examples/web_server.osc` still failed the standard examples compile gate. That blocker has since been rechecked and cleared; this note is preserved as audit history and superseded by the approval update below.

## Reviewer verdict

- **Original verdict:** Reject for follow-up
- **Current state:** Superseded by approval after `web_server` revalidation

## Evidence

1. Initial failing state: `build-examples.ps1` compiled 24 examples and failed exactly 1: `web_server`.
2. Initial direct compile failure was:
   - `error in examples\web_server.osc:72:58: unexpected character '''`
3. The failing line was the CSS `font-family: 'Segoe UI'` fragment in the HTML style block.

## Approval update

- Re-review date: 2026-04-01
- Current verdict: **Approve**

### Validation

1. Direct compile of `examples\web_server.osc` now succeeds with the checked-in compiler.
2. `build-examples.ps1` now reports:
   - **25 compiled, 0 failed**
3. Interpolation regression gate still passes:
   - positive `tests\positive\*interpolation*.osc`
   - negative `tests\negative\*interpolation*.osc`

### Reviewer conclusion

The example sweep now meets the reviewer bar. The highest-value interpolation opportunities identified by both audits were addressed, and no compile/test blockers remain in the reviewed examples.

## Neo repair follow-up

- Reviewer mode rerun after Trinity lockout and Neo-only repair:
  - direct `examples\web_server.osc` compile: PASS
  - `build-examples.ps1`: PASS (**25 compiled, 0 failed**)
  - interpolation positive/negative compile-reject gate: PASS
- Fresh verdict on Neo's revision: **Approve**

## What passed

- Targeted interpolation conformance remains green:
  - `tests\positive\*interpolation*.osc`
  - `tests\negative\*interpolation*.osc`
- `examples\string_interpolation.osc --run` still produces the expected showcase output.
- Interpolation upgrades in these examples look good and should remain:
  - `examples\env_info.osc`
  - `examples\error_handling.osc`
  - `examples\file_checksum.osc`
  - `examples\http_client.osc`
  - `examples\word_freq.osc`
  - `examples\gfx\ui_demo.osc`

## Scope guidance

- Keep using interpolation where it clearly improves presentation strings, request text, and UI/status labels.
- Do **not** block on converting every remaining `print` sequence in streaming or columnar utilities (`wc`, `checksum`, `hexdump`, `sort`, `upper`, similar files) when manual output still reads naturally.
- `countlines.osc` and `grep.osc` still have small optional interpolation opportunities on error/reporting lines, but they are not reviewer-blocking compared with the `web_server` compile failure.
