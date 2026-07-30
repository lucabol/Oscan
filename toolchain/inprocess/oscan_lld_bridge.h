#ifndef OSCAN_LLD_BRIDGE_H
#define OSCAN_LLD_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct OscanLldMemoryInput {
  const char *name;
  const uint8_t *data;
  size_t size;
} OscanLldMemoryInput;

typedef struct OscanLldResult {
  int32_t return_code;
  uint8_t can_run_again;
  char *stdout_data;
  size_t stdout_size;
  char *stderr_data;
  size_t stderr_size;
} OscanLldResult;

int32_t oscan_lld_link(const char *const *args, size_t arg_count,
                       const OscanLldMemoryInput *inputs, size_t input_count,
                       OscanLldResult *result);
void oscan_lld_dispose_result(OscanLldResult *result);

#ifdef __cplusplus
}
#endif

#endif
