/**HEADER*******************************************************************
  project : VdMot Controller
  Comments: health figures for gstat (uptime, reset cause, reset counter)
***************************************************************************/

#ifndef _SYSSTAT_H
	#define _SYSSTAT_H

#include <Arduino.h>
#include "vdm/system_stats.h"

// call first thing after reset, before anything clears the RCC reset flags
void sysstat_capture_reset (void);
// call from the main loop at least once per 49 days
void sysstat_loop (void);

uint32_t sysstat_uptime_s (void);
uint32_t sysstat_resets (void);
vdm::BootReason sysstat_boot_reason (void);

#endif //_SYSSTAT_H
