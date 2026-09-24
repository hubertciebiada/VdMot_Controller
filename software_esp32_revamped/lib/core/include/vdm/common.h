// Shared constants, value types and bounded string/number helpers for the
// VdMot Revamped ESP core. Hardware-free: no Arduino or ESP-IDF headers.
//
// Conventions used by every core module:
//  - No exceptions, no RTTI, no heap. Fixed-size storage only.
//  - Valve and sensor indices are 0-based inside the core. The 1-based numbers
//    seen by users (web, MQTT fallback segments) are converted at the edges.
//  - Time is passed in explicitly: `nowMs` is a monotonic millisecond counter
//    that wraps at 2^32 (Arduino millis()). Always compare with elapsedMs() /
//    timeReached(), never with `<` on raw timestamps.
//  - Output text buffers are always NUL-terminated when capacity >= 1, also on
//    failure.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

constexpr uint8_t kValveCount = 12;       // ACTUATOR_COUNT
constexpr uint8_t kTempSlotCount = 34;    // TEMP_SENSORS_COUNT (config slots and STM bus max)
constexpr uint8_t kVoltSlotCount = 8;     // VOLT_SENSORS_COUNT (config slots and STM bus max)
constexpr uint8_t kAllValves = 255;       // "all valves" selector on the STM protocol
constexpr uint8_t kNoValve = 0xFE;        // "request/event is not about a valve"

constexpr size_t kStationNameMax = 20;    // chars, without NUL (legacy char[21])
constexpr size_t kItemNameMax = 10;       // valve/sensor names (legacy char[11])
constexpr size_t kUnitMax = 8;            // volt sensor unit (legacy char[9])
constexpr size_t kOneWireIdTextLen = 23;  // "28-84-37-94-97-ff-03-23"

// Temperatures are int16 tenths of a degree Celsius, exactly as the STM sends
// them. These raw values are sentinels, never real readings.
constexpr int16_t kTempUnassigned = -500;  // not assigned / never read
constexpr int16_t kTempReadError = -1270;  // DS18 DEVICE_DISCONNECTED_C
constexpr int16_t kTempPowerOn = 850;      // DS18 power-on reset value (85.0)
// Voltage sensors: int32 in 10 mV units; this and below means CRC failure.
constexpr int32_t kVadFailed = -1000;

// ---------------------------------------------------------------- time

// Milliseconds elapsed from `since` to `now`, correct across one wrap.
uint32_t elapsedMs(uint32_t now, uint32_t since);
// True when `now` is at or after `deadline` (wrap-safe, horizon 2^31 ms).
// Only for deadlines that are always checked soon after they are set; a
// deadline that may sit unchecked for 24.8 days reads as "in the future"
// again. Use Backoff (or elapsedMs() from a start time) for those.
bool timeReached(uint32_t now, uint32_t deadline);

// Retry pacing with exponential back-off (reconnects). Keeps the time of the
// last attempt, not a deadline, so an attempt that is due stays due however
// long nobody asked (e.g. a connection that was up for 30 days).
class Backoff {
 public:
  // minMs >= 1; maxMs < minMs is raised to minMs.
  Backoff(uint32_t minMs, uint32_t maxMs);
  // True when armed (no failed attempt since reset()) or when the current
  // delay has passed since the last failed attempt.
  bool due(uint32_t nowMs) const;
  // An attempt at nowMs failed: the next is due after the current delay,
  // which then doubles up to maxMs.
  void onFailure(uint32_t nowMs);
  // Success, or a forced retry: due at once, delay back to minMs.
  void reset();
  uint32_t delayMs() const { return delay_; }  // wait after the next failure

 private:
  uint32_t min_;
  uint32_t max_;
  uint32_t delay_;      // wait after the next failure
  uint32_t waitMs_ = 0; // wait after the last failure
  uint32_t lastMs_ = 0;
  bool armed_ = true;
};

// Broken-down local wall-clock time, filled by glue from localtime_r() after
// the POSIX TZ string has been applied. `valid` is false until SNTP has set
// the clock (year >= 2020).
struct LocalTime {
  bool valid = false;
  uint16_t year = 0;    // e.g. 2026
  uint8_t month = 0;    // 1..12
  uint8_t mday = 0;     // 1..31
  uint8_t wday = 0;     // 0 = Sunday .. 6 = Saturday (tm_wday)
  uint8_t hour = 0;     // 0..23
  uint8_t minute = 0;   // 0..59
  uint8_t second = 0;   // 0..60
  int64_t epoch = 0;    // UTC seconds since 1970 of the same instant
};

// ---------------------------------------------------------------- strings

// Copies `src` into `dst` (capacity `cap` including NUL).
// Returns false, and leaves the truncated prefix in dst, when src does not
// fit; returns false with dst = "" when src is null. cap == 0 writes nothing.
bool copyString(char* dst, size_t cap, const char* src);

// Length of `s` bounded by `max` (like strnlen; s may be null -> 0).
size_t boundedLength(const char* s, size_t max);

// Length (2..4) of the well-formed UTF-8 sequence starting at s[0] >= 0x80,
// looking at no more than `avail` bytes; 0 when it is not one (stray
// continuation byte, overlong form, surrogate, above U+10FFFF, truncated) or
// when it encodes a C1 control (U+0080..U+009F).
size_t utf8SequenceLength(const char* s, size_t avail);

// Exactly `len` bytes of printable text: ASCII 0x20..0x7E and well-formed
// UTF-8 without control characters (the JSON writer passes it through).
bool isPrintableText(const char* s, size_t len);

// Character policy for every user-supplied name that ends up in MQTT topics,
// HA discovery or HTTP JSON (station, valve, sensor names, units):
// printable text (isPrintableText, so UTF-8 like "Küche" or "°C" is fine, as
// in the legacy firmware) except '+', '#', '/', '"', '\\'. Length 0..maxLen
// bytes (0 only when allowEmpty). Spaces are allowed anywhere; topic segments
// map them to '_' like the legacy firmware (buildSegment).
bool isSafeName(const char* s, size_t maxLen, bool allowEmpty);

// Network host name (DHCP, mDNS, syslog) derived from the station name:
// ASCII letters, digits, '-' and '_' are kept, every run of other bytes
// (spaces, UTF-8, punctuation) becomes one '-', leading/trailing '-' are
// dropped, at most cap - 1 chars. "VdMot" when nothing is left. Returns the
// length (0 only for cap < 6).
size_t buildHostname(const char* station, char* out, size_t cap);

// Hostname/broker-host policy: 1..maxLen chars of [A-Za-z0-9.-], not starting
// or ending with '-' or '.'. Used for the MQTT broker host and NTP server.
bool isHostName(const char* s, size_t maxLen);

// ---------------------------------------------------------------- numbers

// Strict decimal parsers over exactly `len` bytes (no NUL needed):
// optional '-' (parseInt only), then 1..10 digits, nothing else (no spaces,
// no '+', no hex, no exponent). Values outside [min,max] are rejected.
// On failure `out` is unchanged.
bool parseUint(const char* s, size_t len, uint32_t max, uint32_t& out);
bool parseInt(const char* s, size_t len, int32_t min, int32_t max, int32_t& out);

// Dotted IPv4 "a.b.c.d" (each 0..255, no leading '+', 1..3 digits) to the
// legacy uint32 layout used in NVS: first octet in the LOW byte
// ("192.168.1.2" -> 0x0201A8C0), i.e. what Arduino IPAddress casts to.
bool parseIpv4(const char* s, size_t len, uint32_t& out);
// Inverse of parseIpv4. `out` needs >= 16 bytes; returns chars written.
size_t formatIpv4(uint32_t ip, char* out, size_t cap);

// ---------------------------------------------------------------- 1-Wire

// 1-Wire ROM id, family byte first (b[0]) and CRC last (b[7]).
struct OneWireId {
  uint8_t b[8] = {0, 0, 0, 0, 0, 0, 0, 0};
};

bool operator==(const OneWireId& a, const OneWireId& b);
bool operator!=(const OneWireId& a, const OneWireId& b);
bool isZero(const OneWireId& id);
// Dallas/Maxim CRC8 (poly 0x31 reflected) over b[0..6] equals b[7].
bool crcValid(const OneWireId& id);
// Parses exactly 23 chars "hh-hh-hh-hh-hh-hh-hh-hh", hex case-insensitive.
bool parseOneWireId(const char* s, size_t len, OneWireId& out);
// Writes the 23-char lowercase form; `out` needs >= 24 bytes.
size_t formatOneWireId(const OneWireId& id, char* out, size_t cap);

}  // namespace vdm
