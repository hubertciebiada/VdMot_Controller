// HTTP routes of the legacy firmware: the aliases /valves, /temps, /volts and
// /setvalve with their legacy JSON documents, and the 410 table of the legacy
// paths that have a replacement in /api. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/json_api.h"
#include "vdm/json_writer.h"

namespace vdm {

enum class LegacyRoute : uint8_t { None, Valves, Temps, Volts, SetValve, Gone, MethodNotAllowed };

struct LegacyMatch {
  LegacyRoute route = LegacyRoute::None;
  const char* replacement = "";  // Gone: the "detail" of the 410 answer
  bool needsAuth = false;
};

// Exact, case-sensitive path (no query). MethodNotAllowed: an alias path
// with another method (GET for /valves, /temps, /volts; POST for
// /setvalve). Gone: any method on a path of the 410 table. needsAuth:
// Valves/Temps/Volts like /api/valves (true only with protectRead);
// SetValve true; Gone and MethodNotAllowed false.
LegacyMatch matchLegacyRoute(HttpMethod m, const char* path, size_t len, bool protectRead);

// {"valves":[{"idx":n,"name":"..","state":s,"pos":..,"meanCur":..,
//  "targetPos":t,"link":0,"moves":..,"oc":..,"cc":..,"dc":..,"cr":..,
//  ["tIdxName1":"..","temp1":21.5|"failed",]["tIdxName2":..,"temp2":..,]
//  "controlActive":0[,"calibration":1]},...]}
// One entry per valve whose status is neither 0 (no data) nor 6 (no valve);
// idx = array position + 1. targetPos = desired when valid, else the STM
// target when known, else pos. A sensor position k is listed when
// sensorSlot[k] != 0. "calibration" only while calibrating. Returns jw.ok().
bool writeLegacyValvesJson(JsonWriter& jw, const ValveView* views, uint8_t count);
// [{"id":"28-..","name":"..","temp":21.5|"failed"},...] for configured
// (slot != 0), active slots with an id that no valve uses (every one with
// allTemps). Returns jw.ok().
bool writeLegacyTempsJson(JsonWriter& jw, const SensorView* temps, uint8_t count, bool allTemps);
// [{"id":"..","name":"..","unit":"..","value":12.080|"failed"},...] for
// configured, active slots with an id; value = SensorView::value
// (milli-units) with 3 decimals. Returns jw.ok().
bool writeLegacyVoltsJson(JsonWriter& jw, const SensorView* volts, uint8_t count);

}  // namespace vdm
