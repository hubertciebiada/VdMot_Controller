// Stub: contract in vdm/stm_codec.h; implemented by the core implementer.
#include "vdm/stm_codec.h"

namespace vdm {

const char* cmdName(Cmd) { return ""; }
Cmd cmdFromName(const char*, size_t) { return Cmd::None; }
bool cmdIsV2(Cmd) { return false; }
bool cmdIsIdempotent(Cmd) { return false; }

bool motorCharsValid(const MotorChars&) { return false; }
bool breakawayValid(const Breakaway&) { return false; }
bool learnMovementsValid(uint32_t) { return false; }

bool buildSetTarget(uint8_t, uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetTarget(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildValveData(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildValveStates(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildTempCount(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildTempList(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildTempData(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildValveSensors(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildVoltCount(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildVoltList(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildVoltData(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildScanOneWire(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildSetValveSensors(uint8_t, const OneWireId&, const OneWireId&, RequestLine& out) {
  return (out = RequestLine{}, false);
}
bool buildMatchSensors(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildAssembly(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildCalibrate(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildDetect(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildSetLearnMovements(uint32_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetLearnMovements(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildSetMotorChars(const MotorChars&, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetMotorChars(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetVersion(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetHwId(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildEepromState(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildSoftReset(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetProto(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildValveEx(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildProfile(uint8_t, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildServiceMove(uint8_t, MoveDir, uint16_t, uint8_t, RequestLine& out) {
  return (out = RequestLine{}, false);
}
bool buildSetBreakaway(const Breakaway&, RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetBreakaway(RequestLine& out) { return (out = RequestLine{}, false); }
bool buildGetStatus(RequestLine& out) { return (out = RequestLine{}, false); }

const char* parseStatusName(ParseStatus) { return ""; }
const char* stopReasonName(StopReason) { return ""; }

ParseStatus parseReply(const char*, size_t, Reply& out) {
  out = Reply{};
  return ParseStatus::Empty;
}

bool replyMatches(const RequestLine&, const Reply&) { return false; }

uint8_t resolveTempSlot(const OneWireId&, const OneWireId*, uint8_t) { return 0; }

const char* stmChipName(uint16_t) { return "Unknown Chip"; }

}  // namespace vdm
