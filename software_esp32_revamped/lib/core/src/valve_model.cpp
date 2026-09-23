// Stub: contract in vdm/valve_model.h; implemented by the core implementer.
#include "vdm/valve_model.h"

namespace vdm {

const char* valveStatusText(uint8_t) { return ""; }
const char* valveStatusKey(uint8_t) { return ""; }
const char* targetSourceName(TargetSource) { return ""; }
const char* targetSyncName(TargetSync) { return ""; }
uint32_t diffValve(const ValveState&, const ValveState&) { return 0; }

ValveModel::ValveModel(const ValveModelParams& params) : params_(params) {}

void ValveModel::setActiveMask(uint16_t) {}
bool ValveModel::setDesiredTarget(uint8_t, uint8_t, TargetSource, uint32_t) { return false; }
void ValveModel::applyValveData(const ValveData&, uint32_t) {}
void ValveModel::applyValveEx(const ValveEx&, uint32_t) {}
void ValveModel::applyValveStates(const ValveStates&, uint32_t) {}
void ValveModel::applyTarget(const TargetReply&, uint32_t) {}
void ValveModel::applyValveSensors(const ValveSensors&, const OneWireId*, uint8_t) {}
bool ValveModel::nextTargetPush(uint32_t, uint8_t&, uint8_t&) { return false; }
void ValveModel::onTargetAck(uint8_t, uint32_t) {}
void ValveModel::onTargetTimeout(uint8_t, uint32_t) {}
bool ValveModel::nextVerify(uint8_t&) const { return false; }
void ValveModel::onStmRebooted(uint32_t) {}
void ValveModel::tick(uint32_t) {}
const ValveState& ValveModel::valve(uint8_t i) const {
  static const ValveState kEmpty{};
  return i < kValveCount ? v_[i] : kEmpty;
}
bool ValveModel::anyCalibrating() const { return false; }
bool ValveModel::isBusy(uint8_t) const { return false; }
void ValveModel::updateHealth(uint8_t, uint32_t) {}

bool tempRawValid(int16_t) { return false; }
bool vadValid(int32_t) { return false; }

bool SensorModel::applyTempList(const OneWireList&, uint32_t) { return false; }
bool SensorModel::applyVoltList(const OneWireList&, uint32_t) { return false; }
void SensorModel::applyTempData(uint8_t, const TempData&, uint32_t) {}
void SensorModel::applyVoltData(uint8_t, const VoltData&, uint32_t) {}
void SensorModel::clear() {}
const TempReading& SensorModel::temp(uint8_t busIndex) const {
  static const TempReading kEmpty{};
  return busIndex < kTempSlotCount ? temps_[busIndex] : kEmpty;
}
const VoltReading& SensorModel::volt(uint8_t busIndex) const {
  static const VoltReading kEmpty{};
  return busIndex < kVoltSlotCount ? volts_[busIndex] : kEmpty;
}
int SensorModel::findTemp(const OneWireId&) const { return -1; }
int SensorModel::findVolt(const OneWireId&) const { return -1; }
bool SensorModel::tempFresh(uint8_t, uint32_t, uint32_t) const { return false; }

}  // namespace vdm
