# Release Packaging

Release builds are handled by GitHub Actions workflows. Two manual workflows must be run **once** (and again whenever their upstream dependencies change) before creating a release.

## Mirror musl toolchain (one-time setup)

The Linux release bundle ships a musl cross-compiler so users can compile freestanding programs without installing gcc. The toolchain comes from [musl.cc](https://musl.cc/) but that site blocks GitHub Actions, so we self-host it as a GitHub release asset.

**Run once from your local machine** (musl.cc blocks GitHub Actions, so the workflow won't work):

```bash
curl -fSL -o x86_64-linux-musl-cross.tgz https://musl.cc/x86_64-linux-musl-cross.tgz
gh release create toolchains --title "Toolchains" --notes "Pre-downloaded musl cross-compilation toolchains" x86_64-linux-musl-cross.tgz
```

Re-run if the musl.cc toolchain is updated.

## Build BearSSL (when BearSSL submodule changes)

TLS support on Linux uses [BearSSL](https://www.bearssl.org/), compiled as a static library. Rather than rebuilding all 293 source files on every release, the library is pre-built and committed.

**Run** from Actions → "Build BearSSL" → Run workflow. This compiles BearSSL with system gcc (freestanding flags) and commits `packaging/prebuilt/linux-x86_64/libbearssl.a`.

Re-run whenever `deps/laststanding/bearssl/` is updated.

## Creating a release

After both prerequisites are in place, tag a version and push:

```bash
git tag v0.0.12
git push origin v0.0.12
```

The Release workflow automatically builds oscan for Windows and Linux, assembles bundles with the toolchain and libbearssl.a, runs smoke tests, and publishes to GitHub Releases.
