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

// safe mode after a loop of watchdog resets: no valve moves until it is left
bool sysstat_safe_mode (void);
uint8_t sysstat_wdg_resets (void);			// watchdog resets in the current window
void sysstat_leave_safe_mode (void);		// ssafe 0

#endif //_SYSSTAT_H
