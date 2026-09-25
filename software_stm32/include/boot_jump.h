/**HEADER*******************************************************************
  project : VdMot Controller
  Comments: jump into the ROM bootloader of the STM32 (update over the UART)
***************************************************************************/

#ifndef _BOOT_JUMP_H
	#define _BOOT_JUMP_H

// does not return: the ROM bootloader takes over USART1
void JumpToBootloader (void);

#endif //_BOOT_JUMP_H
