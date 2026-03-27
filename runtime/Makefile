# Babel-C Runtime — Makefile
#
# Targets:
#   all     — build static library (libbc_runtime.a)
#   test    — build and run the test suite
#   clean   — remove build artefacts

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

LIB     := libbc_runtime.a
SRC     := bc_runtime.c
OBJ     := bc_runtime.o
TEST_SRC:= test_runtime.c
TEST_BIN:= test_runtime

.PHONY: all test clean

all: $(LIB)

$(OBJ): $(SRC) bc_runtime.h
	$(CC) $(CFLAGS) -c $(SRC) -o $(OBJ)

$(LIB): $(OBJ)
	$(AR) rcs $(LIB) $(OBJ)

$(TEST_BIN): $(TEST_SRC) $(LIB) bc_runtime.h
	$(CC) $(CFLAGS) $(TEST_SRC) -L. -lbc_runtime -lm $(LDFLAGS) -o $(TEST_BIN)

test: $(TEST_BIN)
	./$(TEST_BIN)

clean:
	rm -f $(OBJ) $(LIB) $(TEST_BIN)
