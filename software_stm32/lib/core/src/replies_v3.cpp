#include "vdm/replies_v3.h"

namespace vdm {

bool formatValveExtV3(BufWriter& out, const ValveExtV3Reply& r) {
  ReplyLine line(out, "gvlvy");
  return appendValveExtFields(line, r.base)
      .u(r.flags)
      .u(r.fault)
      .u(r.failsafePct)
      .u(r.drive)
      .u(r.retryS)
      .u(r.retries)
      .done();
}

bool formatStatV3(BufWriter& out, const StatV3Reply& r) {
  ReplyLine line(out, "gstax");
  return appendStatFields(line, r.base)
      .u(r.lease)
      .u(r.leaseRemainS)
      .u(r.leaseClient)
      .u(r.leaseTimeoutMin)
      .u(r.failsafeMask)
      .u(r.safeMode)
      .u(r.wdgResets)
      .u(r.uartOre)
      .u(r.uartFe)
      .u(r.uartNe)
      .u(r.rxDropped)
      .u(r.cfgFlags)
      .u(r.cfgEvents)
      .u(r.eepWrites)
      .u(r.tempAgeS)
      .u(r.owScanAgeS)
      .u(r.sysFlags)
      .done();
}

bool formatHeartbeat(BufWriter& out, uint8_t lease, uint32_t remainS) {
  return ReplyLine(out, "slhbt").u(lease).u(remainS).done();
}

bool formatLeaseConfig(BufWriter& out, uint16_t timeoutMin, const uint8_t (&failsafePct)[kValveCount]) {
  ReplyLine line(out, "glcfg");
  line.u(timeoutMin);
  for (uint8_t pct : failsafePct) line.u(pct);
  return line.done();
}

bool formatLearnTime(BufWriter& out, uint32_t seconds) { return ReplyLine(out, "gtlnt").u(seconds).done(); }

}  // namespace vdm
