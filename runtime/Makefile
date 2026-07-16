# Oscan Runtime — Makefile
#
# Targets:
#   all                  — build static library (libosc_runtime.a), sanitized dev build
#   test                 — build and run the test suite
#   archive-hosted            — build a release-quality, per-target hosted (libc) runtime archive
#   archive-freestanding      — build a release-quality, per-target freestanding (no-libc) runtime archive
#   archive-freestanding-core — freestanding sibling without gfx/img/svg/tt (see osc_runtime_freestanding_core.c)
#   archives                  — all of the above
#   clean                — remove build artefacts (dev build + archives)
#
# Cross-target / release archives:
#   The archive-* targets delegate to scripts/release_tools.py's
#   `build-runtime-archive` command, the canonical cross-platform builder.
#   TARGET must be (or auto-detects to) a concrete release tag. Override CC/AR
#   with a matching cross toolchain, e.g.:
#     make archives CC=x86_64-linux-musl-gcc AR=x86_64-linux-musl-ar TARGET=linux-x86_64
#   Output goes to build/runtime-archives/$(TARGET)/ (gitignored — archives are
#   build artefacts, not committed sources).

CC      ?= gcc
AR      ?= ar
CFLAGS  := -Wall -Wextra -Werror -std=c99 -pedantic
LDFLAGS :=

# Sanitizers (enabled for debug/test builds, can be disabled with SANITIZE=0)
SANITIZE ?= 1
ifeq ($(SANITIZE),1)
  CFLAGS  += -fsanitize=address,undefined -fno-omit-frame-pointer -g
  LDFLAGS += -fsanitize=address,undefined
endif

LIB     := libosc_runtime.a
SRC     := osc_runtime.c
OBJ     := osc_runtime.o
TEST_SRC:= test_runtime.c
TEST_BIN:= test_runtime

# --- Release archive settings. The Python builder owns compile flags,
#     manifests, clean object staging, and atomic publication. ---
ifeq ($(OS),Windows_NT)
  PYTHON     ?= python
else
  PYTHON     ?= python3
endif
TARGET       ?= $(shell cd .. && $(PYTHON) scripts/release_tools.py detect-host-target)
ARCHIVE_DIR  := ../build/runtime-archives/$(TARGET)

# GNU make defines built-in CC/AR defaults even when the caller supplied
# nothing. Pass them through only when they came from the environment or
# command line; otherwise let the canonical builder discover a matching pair.
ARCHIVE_CC_ARG = $(if $(filter command line environment environment%,$(origin CC)),--cc "$(CC)",)
ARCHIVE_AR_ARG = $(if $(filter command line environment environment%,$(origin AR)),--ar "$(AR)",)

.PHONY: all test clean check-archive-target archive-hosted archive-freestanding archive-freestanding-core archives clean-archives

all: $(LIB)

$(OBJ): $(SRC) osc_runtime.h
	$(CC) $(CFLAGS) -c $(SRC) -o $(OBJ)

$(LIB): $(OBJ)
	$(AR) rcs $(LIB) $(OBJ)

$(TEST_BIN): $(TEST_SRC) $(LIB) osc_runtime.h
	$(CC) $(CFLAGS) $(TEST_SRC) -L. -losc_runtime -lm $(LDFLAGS) -o $(TEST_BIN)

test: $(TEST_BIN)
	./$(TEST_BIN)

check-archive-target:
	@$(PYTHON) -c "import sys; target = '$(TARGET)'; allowed = ('windows-x86_64', 'linux-x86_64', 'macos-x86_64'); sys.exit(0 if target in allowed else 'TARGET must be a concrete supported tag (got %r)' % target)"

archive-hosted: check-archive-target
	cd .. && $(PYTHON) scripts/release_tools.py build-runtime-archive \
	    --target "$(TARGET)" --mode hosted --out-dir "build/runtime-archives/$(TARGET)" \
	    $(ARCHIVE_CC_ARG) $(ARCHIVE_AR_ARG)

archive-freestanding: check-archive-target
	cd .. && $(PYTHON) scripts/release_tools.py build-runtime-archive \
	    --target "$(TARGET)" --mode freestanding --out-dir "build/runtime-archives/$(TARGET)" \
	    $(ARCHIVE_CC_ARG) $(ARCHIVE_AR_ARG)

archive-freestanding-core: check-archive-target
	cd .. && $(PYTHON) scripts/release_tools.py build-runtime-archive \
	    --target "$(TARGET)" --mode freestanding_core --out-dir "build/runtime-archives/$(TARGET)" \
	    $(ARCHIVE_CC_ARG) $(ARCHIVE_AR_ARG)

archives: archive-hosted archive-freestanding archive-freestanding-core

clean-archives:
	rm -rf ../build/runtime-archives

clean: clean-archives
	rm -f $(OBJ) $(LIB) $(TEST_BIN)
