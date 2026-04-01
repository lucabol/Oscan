---
name: "interpolation-example-review"
description: "Review and modernize Oscan examples that should showcase string interpolation without breaking purity or literal-brace handling."
domain: "examples"
confidence: "high"
source: "earned"
---

## Context

Use this when reviewing new or existing Oscan examples that build human-readable output, protocol text, HTML, CSS, or JSON. It applies both to dedicated interpolation demos and to older examples that still use nested `str_concat(...)` formatting chains.

## Patterns

1. Read team context first:
   - `.squad/agents/tank/history.md`
   - `.squad/decisions.md`
   - `.squad/identity/wisdom.md`
   - `.squad/identity/now.md`
2. Prefer interpolation for presentation-only strings:
   - console output like `"count={count}"`
   - protocol/request text like `"GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n"`
   - UI/status labels like `"Volume: {volume}"`
3. Keep hole expressions pure. If data comes from an allocating or impure helper (`str_from_chars`, `str_slice`, `dir_current`, `arg_count`, etc.), compute it first in a binding and interpolate the variable instead.
4. Escape literal braces in string literals as `{{` / `}}`. This is required for CSS, JSON, templated HTML, and any literal `{` or `}` text after interpolation support shipped.
5. After any HTML/CSS interpolation refactor, compile the example immediately. Brace escaping is necessary but not sufficient; lexer-sensitive literals can still break the file. If a CSS fragment trips the parser, prefer a semantically equivalent literal that avoids embedded apostrophes before backing out interpolation.
6. Keep `str_concat(...)` where incremental buffer assembly is still clearer, especially when appending many static fragments over time.

## Examples

- `examples\web_server.osc`: escape CSS braces and use interpolation for table rows, footer text, HTTP response framing, and request logs.
- `examples\web_server.osc`: if the CSS `font-family` fragment causes parser trouble, keep the interpolation upgrade and switch from `'Segoe UI'` to `Segoe UI` rather than reverting the broader cleanup.
- `examples\http_client.osc`: replace nested request-building concatenation with one interpolated request string.
- `examples\gfx\ui_demo.osc`: convert status labels to interpolation, but precompute `str_from_chars(text_buf)` before using it in a hole.
- `examples\env_info.osc`: precompute impure values like `dir_current()` before interpolation; interpolate simple numeric/date output directly.
- `build-examples.ps1`: use this as the repo-wide regression gate after touching examples; it catches both missed interpolation opportunities and literal-brace/parser regressions.

## Anti-Patterns

- Leaving lone `{` or `}` inside example strings after interpolation support exists.
- Calling impure helpers directly inside interpolation holes.
- Rewriting every concatenation mechanically; only update cases where interpolation improves readability or discoverability.
