# Morpheus — Interpolation MVP runtime decision

- Runtime adds **no new interpolation-specific helper** for MVP.
- Compiler lowering should reuse the existing arena-backed primitives:
  - `str_concat`
  - `str_from_i32` / `i32_to_str` (same runtime path)
  - `str_from_i64`
  - `str_from_f64`
  - `str_from_bool`
- `str_from_i32` now delegates to `osc_i32_to_str` so i32 stringification has one runtime implementation.
- This keeps the runtime minimal, portable, and aligned with the current arena-only memory model.
