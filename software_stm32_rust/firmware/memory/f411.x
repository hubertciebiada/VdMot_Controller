/* STM32F411CE: 512 KiB flash, 128 KiB RAM (docs/rust/GLUE-DESIGN-STM.md §3.2, §5.6).
   build.rs copies this file to memory.x, which cortex-m-rt's link.x includes. */
MEMORY
{
  FLASH  : ORIGIN = 0x08000000, LENGTH = 512K
  RAM_LO : ORIGIN = 0x20000000, LENGTH = 0x3234   /* reserved, unused (headroom) */
  NOINIT : ORIGIN = 0x20003234, LENGTH = 0xD4     /* C++ 2.1.7 .noinit: warm, guard, counter */
  RAM    : ORIGIN = 0x20003308, LENGTH = 0x1CCF8  /* .data .bss .uninit; stack from the top */
}

INCLUDE vdm.x
