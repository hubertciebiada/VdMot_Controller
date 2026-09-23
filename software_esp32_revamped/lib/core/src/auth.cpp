// Stub: contract in vdm/auth.h; implemented by the core implementer.
#include "vdm/auth.h"

namespace vdm {

bool constantTimeEquals(const char*, const char*) { return false; }
bool checkBasicAuth(const char*, size_t, const char*, const char*) { return false; }

AuthLimiter::AuthLimiter(uint8_t maxFailures, uint32_t windowMs, uint32_t lockoutMs)
    : maxFailures_(maxFailures), windowMs_(windowMs), lockoutMs_(lockoutMs) {}
bool AuthLimiter::locked(uint32_t) const { return false; }
void AuthLimiter::onResult(bool, uint32_t) {}

}  // namespace vdm
