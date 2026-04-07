# macOS phase 1 packaging note

macOS stays binary-only in phase 1.

## Why there is no bundled `toolchain/`

- Oscan's current macOS path is libc-only; the existing CI job explicitly does **not** support freestanding macOS builds.
- A self-contained Apple-native C toolchain would depend on Apple CLT/Xcode SDK contents, host SDK selection, and Apple-managed toolchain layout that we should not redistribute as a GitHub Release bundle.
- Requiring the host Apple Command Line Tools keeps SDK selection, codesigning expectations, and compiler updates on the machine that is actually building the user's native binary.

## Release contract

- Ship the `oscan` binary by itself for macOS phase 1.
- Do not publish a macOS toolchain vendoring manifest in this phase.
- Install and release scripts should treat Apple CLT (`xcode-select --install`) or an equivalent host compiler as a prerequisite.
