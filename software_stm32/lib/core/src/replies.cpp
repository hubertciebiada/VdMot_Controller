#include "vdm/replies.h"

namespace vdm {

bool formatValveData(BufWriter& out, const char* prefix, const ValveDataReply& r) {
  const size_t start = out.length();
  const int32_t fields[] = {r.actualPosition, r.meanCurrent,  r.status,        r.temperature1,
                            r.temperature2,   r.movements,    r.openingCount,  r.closingCount,
                            r.deadzoneCount,  r.calibRetries};
  bool ok = out.append(prefix) && out.append(' ') && out.appendUnsigned(r.index);
  for (size_t i = 0; ok && i < sizeof(fields) / sizeof(fields[0]); ++i) {
    ok = out.append(' ') && out.appendSigned(fields[i]);
  }
  ok = ok && out.append(' ');
  if (!ok) out.truncate(start);
  return ok;
}

bool formatStatusList(BufWriter& out, const char* prefix, const uint8_t* status, size_t n) {
  const size_t start = out.length();
  bool ok = (status != nullptr || n == 0) && out.append(prefix) &&
            out.append(' ') && out.appendUnsigned(static_cast<uint32_t>(n)) && out.append(' ');
  for (size_t i = 0; ok && i < n; ++i) {
    ok = (i == 0 || out.append(',')) && out.appendUnsigned(status[i]);
  }
  ok = ok && out.append(' ');
  if (!ok) out.truncate(start);
  return ok;
}

}  // namespace vdm
