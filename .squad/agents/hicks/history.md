# Hicks History

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Oscan needs a new release pipeline that manufactures bundled Windows/Linux distributions.
- Windows freestanding release bundles built with llvm-mingw need `-nostartfiles` instead of `-nostdlib`, plus explicit Win32/GDI libs, or packaged smoke tests fail at link time.
