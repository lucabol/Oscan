---
title: Example interpolation sweep follow-up
author: Trinity
date: 2026-04-01
status: proposed
---

## Summary

Examples should prefer string interpolation for human-readable formatting and protocol text when it meaningfully reduces nested `str_concat(...)` chains. After interpolation landed, any example strings that contain literal braces must escape them as `{{` and `}}`, especially for embedded CSS, HTML fragments, or JSON snippets.

## Why

- Keeps the example suite aligned with the language features we want users to discover first.
- Prevents regressions like CSS or JSON examples failing because `{` was interpreted as an interpolation opener.
- Reduces noisy formatting code without changing runtime behavior.

## Scope

- Favor interpolation in example output, request formatting, status labels, and other presentation-only strings.
- Keep plain concatenation where it is still the clearest choice for incremental buffer assembly.
- Treat brace escaping in example strings as required compatibility hygiene, not optional style.
