/**HEADER*******************************************************************
  project : VdMot Controller
  Comments: health figures for gstat (uptime, reset cause, reset counter)
***************************************************************************/

#include "sysstat.h"
#include "vdm/reset_guard.h"

// not cleared by the start-up code, so it survives every reset except a power loss
static vdm::ResetCounterCell reset_cell __attribute__((noinit));
// watchdog resets in a row: safe mode (no valve moves) after a reset loop
static vdm::ResetGuardCell guard_cell __attribute__((noinit));
static bool safe_mode = false;

static vdm::UptimeCounter uptime;
static vdm::BootReason boot_reason = vdm::BootReason::Unknown;
static uint32_t reset_count = 0;


void sysstat_capture_reset (void) {
	const uint32_t csr = RCC->CSR;
	vdm::ResetFlags flags;

	flags.lowPower = (csr & RCC_CSR_LPWRRSTF) != 0;
	flags.windowWatchdog = (csr & RCC_CSR_WWDGRSTF) != 0;
	flags.independentWatchdog = (csr & RCC_CSR_IWDGRSTF) != 0;
	flags.software = (csr & RCC_CSR_SFTRSTF) != 0;
	flags.powerOn = (csr & RCC_CSR_PORRSTF) != 0;
	flags.pin = (csr & RCC_CSR_PINRSTF) != 0;
	flags.brownOut = (csr & RCC_CSR_BORRSTF) != 0;

	// the flags accumulate until they are cleared: without this a pin reset after a
	// power-on would still show the power-on flag
	RCC->CSR |= RCC_CSR_RMVF;

	boot_reason = vdm::classifyReset(flags);
	reset_count = vdm::countReset(reset_cell, boot_reason);
	safe_mode = vdm::resetGuardOnBoot(guard_cell, boot_reason);
}


void sysstat_loop (void) {
	const uint32_t last = uptime.seconds();
	uptime.update(millis());
	// once per second: the uptime of this boot for the reset window, the end of the safe mode
	if (uptime.seconds() != last) safe_mode = vdm::resetGuardAlive(guard_cell, uptime.seconds());
}


uint32_t sysstat_uptime_s (void) {
	return uptime.seconds();
}


uint32_t sysstat_resets (void) {
	return reset_count;
}


vdm::BootReason sysstat_boot_reason (void) {
	return boot_reason;
}


bool sysstat_safe_mode (void) {
	return safe_mode;
}


uint8_t sysstat_wdg_resets (void) {
	return guard_cell.count;
}


void sysstat_leave_safe_mode (void) {
	vdm::resetGuardClear(guard_cell);
	safe_mode = false;
}
