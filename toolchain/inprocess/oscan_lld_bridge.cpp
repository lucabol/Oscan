#include "oscan_lld_bridge.h"

#include "lld/COFF/MemoryInput.h"
#include "lld/Common/Driver.h"
#include "llvm/ADT/ArrayRef.h"
#include "llvm/Support/raw_ostream.h"
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

LLD_HAS_DRIVER(mingw)

namespace {

std::mutex linkMutex;

char *copyOutput(const std::string &value) {
  char *copy = static_cast<char *>(std::malloc(value.size() + 1));
  if (!copy)
    return nullptr;
  std::memcpy(copy, value.data(), value.size());
  copy[value.size()] = '\0';
  return copy;
}

void resetResult(OscanLldResult *result) {
  result->return_code = -1;
  result->can_run_again = 0;
  result->stdout_data = nullptr;
  result->stdout_size = 0;
  result->stderr_data = nullptr;
  result->stderr_size = 0;
}

class MemoryInputGuard {
public:
  explicit MemoryInputGuard(llvm::ArrayRef<lld::coff::MemoryInput> inputs) {
    lld::coff::setMemoryInputs(inputs);
  }
  ~MemoryInputGuard() { lld::coff::clearMemoryInputs(); }

  MemoryInputGuard(const MemoryInputGuard &) = delete;
  MemoryInputGuard &operator=(const MemoryInputGuard &) = delete;
};

} // namespace

extern "C" int32_t oscan_lld_link(const char *const *args, size_t arg_count,
                                  const OscanLldMemoryInput *inputs,
                                  size_t input_count, OscanLldResult *result) {
  if (!result)
    return -1;
  resetResult(result);
  if (!args || arg_count == 0 || (!inputs && input_count != 0))
    return -1;

  std::scoped_lock lock(linkMutex);
  std::vector<lld::coff::MemoryInput> memoryInputs;
  memoryInputs.reserve(input_count);
  for (size_t index = 0; index < input_count; ++index) {
    const OscanLldMemoryInput &input = inputs[index];
    if (!input.name || input.name[0] == '\0' ||
        (!input.data && input.size != 0))
      return -1;
    memoryInputs.push_back(
        {input.name, llvm::ArrayRef<uint8_t>(input.data, input.size)});
  }

  std::string stdoutText;
  std::string stderrText;
  llvm::raw_string_ostream stdoutStream(stdoutText);
  llvm::raw_string_ostream stderrStream(stderrText);
  static constexpr lld::DriverDef drivers[] = {
      {lld::MinGW, &lld::mingw::link},
  };

  MemoryInputGuard inputGuard(memoryInputs);
  lld::Result linkResult =
      lld::lldMain(llvm::ArrayRef(args, arg_count), stdoutStream, stderrStream,
                   drivers);
  stdoutStream.flush();
  stderrStream.flush();

  result->stdout_data = copyOutput(stdoutText);
  result->stderr_data = copyOutput(stderrText);
  if (!result->stdout_data || !result->stderr_data) {
    oscan_lld_dispose_result(result);
    return -2;
  }
  result->return_code = linkResult.retCode;
  result->can_run_again = linkResult.canRunAgain ? 1 : 0;
  result->stdout_size = stdoutText.size();
  result->stderr_size = stderrText.size();
  return 0;
}

extern "C" void oscan_lld_dispose_result(OscanLldResult *result) {
  if (!result)
    return;
  std::free(result->stdout_data);
  std::free(result->stderr_data);
  resetResult(result);
}
