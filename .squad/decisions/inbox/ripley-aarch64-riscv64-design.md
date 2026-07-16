# AArch64/RISC-V64 ElfDirect + --extra-obj/--extra-lib — Design Decision

**Author:** Ripley (Lead)
**Date:** 2026-07-16
**Status:** SPEC COMPLETE — ready for implementation
**Design doc:** `docs/design/native-link-embedding.md` §11–§14

---

## Scope Decision

**Single-target-per-build** — the existing embedded-asset model is extended
without structural change. Each oscan binary embeds linker assets for exactly
one target (same as today). The standard linux-x86_64 release binary keeps its
own x86_64 linker; cross-linker binaries for aarch64/riscv64 ship as sidecar
files in the release archive, usable via `OSCAN_NATIVE_LINKER` +
`OSCAN_NATIVE_LINKER_FLAVOR=elf`.

**Why not multi-target bundling:** it would require refactoring security-audited
code paths (build.rs asset generation, native_assets.rs extraction/cache,
elevation policy) for a ~6 MB binary size increase. Every Rust control-flow path
stays unchanged with single-target-per-build; only data (2 new EMBED_ASSET_SPECS,
2 new toolchain manifests, 2 new runtime-archive-contract targets) is added.
Multi-target bundling is an acceptable follow-up if user demand warrants.

---

## Implementation Ownership — Zero File Overlap

### Bishop (Rust / toolchain integration)

| File | Work |
|---|---|
| `src/backend/link/mod.rs` | Extend `is_elf_eligible` to include `LinuxAarch64`/`LinuxRiscv64`. Replace host-only gate with target-aware embedded-asset check. Add `elf_emulation()` helper; change `build_elf_plan`'s `emulation` from hardcoded `"elf_x86_64"` to `elf_emulation(target)`. |
| `src/backend/link/plan.rs` | Add `extra_libs: Vec<PathBuf>` field to `LinkPlan`. Render it in `render_mingw_direct`/`render_elf_direct`/`render_compiler_driver` after archives/builtins. |
| `src/backend/link/mod.rs` (`NativeLinkOptions`) | Add `extra_objects: &'a [String]`, `extra_libs: &'a [String]` fields. Thread through `build_mingw_plan`/`build_elf_plan`/`build_compiler_driver_plan`. |
| `src/main.rs` | Add `--extra-obj`/`--extra-lib` CLI parsing (mirror `--extra-c`). Thread `extra_obj_files`/`extra_lib_files` through `run_native_backend`, `compile_to_executable`, `invoke_c_compiler`, cross-compile functions. Update `print_usage`. |
| `tests/cli_help.rs` | Assert `--extra-obj` and `--extra-lib` appear in help output. |
| Unit tests in `link/mod.rs` | Add `is_elf_eligible` tests for aarch64/riscv64. Add `elf_emulation` tests. |
| Unit tests in `plan.rs` | Add render snapshot tests for aarch64/riscv64 `ElfDirect` plans. Add render tests with `extra_libs` populated. |

Bishop does NOT touch: `runtime-archive-contract.json`, `release_tools.py`,
`build.rs`, any workflow file, `packaging/toolchains/*.json`, or
`scripts/*.sh`/`scripts/*.ps1`.

### Hicks (release engineering / Python / CI)

| File | Work |
|---|---|
| `packaging/toolchains/linux-aarch64.json` | Create per §11.6. |
| `packaging/toolchains/linux-riscv64.json` | Create per §11.6. |
| `packaging/toolchains/runtime-archive-contract.json` | Add `linux-aarch64`/`linux-riscv64` target entries (§11.7). Add both to `freestanding`/`freestanding_core` `supported_targets`. |
| `scripts/release_tools.py` | Add `EMBED_ASSET_SPECS["linux-aarch64"]` and `["linux-riscv64"]` entries (§11.8). |
| `.github/workflows/build-bearssl.yml` | Add matrix for cross-compiling BearSSL with aarch64/riscv64 musl-cross GCC (§11.9). |
| `.github/workflows/ci.yml` | Add `native-link-embedding-smoke-linux-aarch64` and `native-link-embedding-smoke-linux-riscv64` jobs (§13.1). Include QEMU user-mode smoke test steps. |
| `.github/workflows/release.yml` | Extend Linux matrix entry to fetch all 3 toolchains, build all 3 runtime archives, package cross-linker sidecars (§13.5). |
| `scripts/build-runtime-archive.sh` / `.ps1` | No structural change needed — already parameterized by `--target`/`--cc`/`--ar`. |
| `scripts/prepare-embed-assets.sh` / `.ps1` | No structural change needed — already parameterized by `--target`. |
| `packaging/prebuilt/linux-aarch64/` | `.gitkeep` + `.gitignore` for staged assets. |
| `packaging/prebuilt/linux-riscv64/` | `.gitkeep` + `.gitignore` for staged assets. |

Hicks does NOT touch: any file under `src/`, `Cargo.toml`, or `tests/`.

### Shared field-name contract (coupling between Bishop and Hicks)

Unchanged from §8.3. The only new data values flowing across the boundary:

- `linker.emulation` values: `"aarch64linux"`, `"elf64lriscv"` (in
  `EMBED_ASSET_SPECS` → `native-link-assets.json` → `LinkPlan.emulation`)
- Target tags: `"linux-aarch64"`, `"linux-riscv64"` (already in
  `NativeTarget::archive_tag()` and `NativeTarget::parse()` — no new Rust
  string introduced)
- `linker.flavor`: `"elf"` (same value as x86_64, already wired)

---

## Key Verified Facts (do not re-derive)

| Fact | Source |
|---|---|
| aarch64 ld emulation: `aarch64linux` | binutils 2.37 `lib/ldscripts/aarch64linux.x` |
| riscv64 ld emulation: `elf64lriscv` | binutils 2.37 `lib/ldscripts/elf64lriscv.x` |
| aarch64 toolchain SHA-256 | `c909817856d6ceda86aa510894fa3527eac7989f0ef6e87b5721c58737a06c38` |
| riscv64 toolchain SHA-256 | `db0bc413bd4a93f2012cc74b9ba0c4af29d8bc18b88e9c61998738ccb918604b` |
| Both tarballs live on `toolchains` GitHub release | Re-downloaded and SHA-256 confirmed |
| Both linker binaries are static (no .so deps) | Same musl.cc vendor/generation as x86_64 |
| QEMU user-mode emulation works on GH ubuntu-latest | `qemu-user-static` apt package |
| `qemu-aarch64-static`, `qemu-riscv64-static` binaries | In `qemu-user-static` package |
