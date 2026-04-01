## Web Server Example Repair

- **Author:** Neo
- **Date:** 2026-04-01
- **Status:** Proposed

### Context

`examples\web_server.osc` was the only rejected artifact in the interpolation example sweep. The accepted changes in the other examples were to remain untouched, and the blocker was a compile failure reported on the CSS `font-family: 'Segoe UI'` fragment.

### Decision

Apply the smallest possible source fix in `examples\web_server.osc`: keep the interpolation and escaped CSS braces, but replace the single-quoted `Segoe UI` family name with the unquoted CSS form `Segoe UI`.

### Rationale

- It repairs only the rejected artifact.
- It preserves the accepted interpolation improvements already made in the example.
- Unquoted `Segoe UI` is valid CSS and avoids the parser-sensitive apostrophe sequence that tripped the build gate.

### Validation

- Compile `examples\web_server.osc`.
- Re-run the examples compilation sweep to confirm the repaired file does not regress the accepted example updates.
