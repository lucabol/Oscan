# Tank — History

## Project Context
- **Project:** Babel-C — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Test suite, compiler conformance testing, C sanitizers (ASan, UBSan)
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Tests cover: type system, scoping, mutability, control flow, error handling, FFI
- Negative tests: shadowing rejection, non-exhaustive match rejection, unhandled errors
- Generated C tested with GCC + Clang for portability
- Sanitizers verify zero UB in generated code
- Each requirement in the spec maps to at least one test case

## Learnings
