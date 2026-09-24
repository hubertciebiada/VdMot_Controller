/**HEADER*******************************************************************
  project : VdMot Controller
  Comments: health figures for gstat (uptime, reset cause, reset counter)
***************************************************************************/

#include "sysstat.h"

// not cleared by the start-up code, so it survives every reset except a power loss
static vdm::ResetCounterCell reset_cell __attribute__((noinit));

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
}


void sysstat_loop (void) {
	uptime.update(millis());
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
