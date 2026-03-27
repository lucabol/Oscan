# Morpheus — History

## Project Context
- **Project:** Babel-C — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Runtime implementation in C, memory management, FFI, standard library
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Runtime is pure C, no external dependencies beyond libc
- Memory model must be deterministic and uniform (one approach for all allocation)
- Micro-lib provides only: basic I/O, math primitives, memory interfaces
- No domain-specific modules (no JSON, HTTP, etc.)
- C-FFI allows seamless external C function calls
- Error-as-value runtime support (Result-like composite types)
- UB guards: bounds checking, integer overflow detection, null pointer guards
- Runtime overhead must be minimal

## Learnings
