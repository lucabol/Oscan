# Scoped Arenas — Design Document

## Problem

Long-running Oscan programs (servers, game loops) accumulate arena memory without reclamation. Oscan uses a single global arena that grows monotonically. The `arena_reset()` function was removed because it caused use-after-free bugs when dangling pointers to freed allocations were dereferenced. This leaves no safe mechanism for per-request or per-frame memory reclamation.

## Proposed Syntax

```oscan
fn! handle_request(client: i32) {
    arena {
        let text: str = try read_file("template.html");
        let response: str = "HTTP/1.0 200 OK\r\n\r\n{text}";
        socket_send(client, response);
    };  // child arena freed — all allocations reclaimed
}
```

## Semantics

- `arena { ... }` creates a child arena for all allocations inside the block
- When the block exits, the child arena is destroyed
- `defer` statements inside arena blocks run before destruction
- Arena blocks are only allowed in `fn!` functions (not `fn` functions)
- All allocations made inside the arena block are attributed to the child arena, not the global arena

## Escape Analysis — Option B (Recommended)

Arena blocks CANNOT return arena-allocated types. This is the key safety mechanism:

- **Prohibited escapes:** `str`, `[T]`, `map`, structs containing `str`/`[T]`
- **Allowed escapes:** primitives (`i32`, `i64`, `f64`, `bool`, unit), and types that don't contain arena-allocated pointers
- This is enforced at compile time by the type checker
- **Rationale:** simple, teachable rule that prevents use-after-free without a borrow checker. No complex lifetime tracking needed.

### Example: What Escapes and What Doesn't

```oscan
fn! process(client: i32) {
    let length: i32;
    arena {
        let text: str = try read_file("data.txt");
        length = len(text);  // ✓ OK: i32 escapes
        socket_send(client, text);  // ✓ OK: consumed within arena
    };  // arena freed; text is gone
    
    // This would NOT compile:
    // let saved: str = arena { ... };  // ✗ str cannot escape arena block
}
```

## Alternatives Considered

- **Option A: No enforcement** (rejected — reintroduces use-after-free)
- **Option C: Full escape analysis with lifetimes** (rejected — complexity of Rust-level borrow checker, too high a bar for Oscan)

## Implementation Outline

1. **Lexer:** Add `arena` keyword
2. **AST:** New expression variant for arena blocks (or statement variant if treating as statement)
3. **Parser:** Parse `arena { ... };` syntax
4. **Semantic analysis:** Enforce escape type checking (reject assignments/returns of arena-allocated types)
5. **Codegen:** 
   - Create a child arena on entry to the arena block
   - Run pending `defer` statements
   - Destroy the child arena on exit
6. **Runtime:** No changes needed — arena infrastructure already supports multiple arenas via reference counting or list management

## Interaction with defer

- `defer` statements inside an arena block run **before** the child arena is destroyed
- This enables cleanup patterns:
  ```oscan
  arena {
    let fd: i32 = try open_file("log.txt");
    defer close_file(fd);
    // ... work with file ...
  };  // defer runs, then arena freed
  ```
- `defer` statements that reference arena-allocated data work correctly because the arena is still live

## Interaction with try

- `try` can be used inside arena blocks
- If an error occurs, the arena is still cleaned up (error propagates as normal)
- The error type must be a primitive or copyable type to escape the arena block
- Rationale: errors are typically small integers or enums, which don't depend on arena lifetime

## Open Questions

1. **Nestability:** Should arena blocks be nestable? (Probably yes — each creates an independent child arena)
2. **Error propagation:** Should `try` always propagate out of arena blocks? (Yes, assuming error types are primitives)
3. **Explicit naming:** Should we allow explicit arena naming for future features? `arena a { ... }` (Probably not for v1)
4. **Interaction with closures:** Can closures capture variables from outer scopes if used inside an arena block? (Needs clarification — likely yes if they don't escape)
5. **Nested allocation patterns:** Should arena blocks support sub-arenas inside them? (Yes, each is independent)

## Status

This is a **design document only**. Implementation is deferred until:
1. `defer` is stable and battle-tested in the language
2. Team agrees on escape semantics and error handling
3. Test suite for arena safety is designed
