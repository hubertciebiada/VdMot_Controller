/* Layout shared by the four images (docs/rust/GLUE-DESIGN-STM.md §3.2, §5.6, D3, D9),
   included by f401.x and f411.x.

   Flash: vector table at 0x08000000, the ID block at 0x08000200, then the boot stage and
   everything it calls (.vdm_boot), then cortex-m-rt's .text with the rest. D9: the ESP flasher
   writes sector 0 (16 KiB) last, so as long as .vdm_boot ends inside sector 0, an interrupted
   flash leaves the old vector table, ID block and boot stage working. The image check
   (vdm-stm-image-check) follows every call and literal of the boot stage and fails when
   anything lies outside sector 0.

   RAM: the C++ 2.1.7 .noinit cells keep their addresses (D3); RAM_LO is not used. */

/* the no-init cells of C++ 2.1.7 (firmware/src/noinit.rs, vdm_stm_boot::capture) */
__vdm_noinit = ORIGIN(NOINIT);
__vdm_warm_state = ORIGIN(NOINIT);
__vdm_guard_cell = ORIGIN(NOINIT) + 0xB4;
__vdm_reset_cell = ORIGIN(NOINIT) + 0xC8;
__vdm_sector0_end = ORIGIN(FLASH) + 0x4000;

SECTIONS
{
  .vdm_id ORIGIN(FLASH) + 0x200 :
  {
    KEEP(*(.vdm_id));
  } > FLASH

  .vdm_boot ORIGIN(FLASH) + 0x240 :
  {
    __vdm_boot_start = .;
    /* cortex-m-rt start-up, entry and fault entries */
    *(.Reset);
    *(.HardFaultTrampoline);
    *(.HardFault.*);
    *(.text.main);
    *(.text.*__cortex_m_rt_main*);
    *(.text.DefaultPreInit .text.__pre_init);
    *(.text.DefaultHandler .text.DefaultHandler_ .text.HardFault_);
    *(.text.NonMaskableInt .text.MemoryManagement .text.BusFault .text.UsageFault);
    *(.text.*rust_begin_unwind*);
    /* cortex_m::asm::bootload (the jump), cortex_m::asm::delay */
    *(.text.*cortex_m3asm*);
    /* the boot stage: firmware registers, the boot crate, the core functions of the capture */
    *(.text.*vdm_stm_fw*boot_hw*);
    *(.text.*vdm_stm_fw*fault*);
    *(.text.*vdm_stm_fw*noinit*);
    *(.text.*vdm_stm_boot*);
    *(.text.*vdm_stm_core*system_stats*);
    *(.text.*vdm_stm_core*reset_guard*);
    *(.text.*vdm_stm_core*crc16_ccitt*);
    /* compiler intrinsics the boot stage may call */
    *(.text.memcpy .text.memmove .text.memset .text.memcmp .text.bcmp);
    *(.text.__aeabi_memcpy* .text.__aeabi_memmove* .text.__aeabi_memset* .text.__aeabi_memclr*);
    *(.text.*compiler_builtins*mem*);
    . = ALIGN(4);
    __vdm_boot_end = .;
  } > FLASH

  /* cortex-m-rt's .text follows the boot stage. ADDR + SIZEOF, not __vdm_boot_end: with the
     symbol lld lays .text out at a wrong address in its first pass and adds long-branch
     thunks between the two sections. */
  _stext = ADDR(.vdm_boot) + SIZEOF(.vdm_boot);
} INSERT AFTER .vector_table;

ASSERT(SIZEOF(.vdm_id) <= 0x40, "vdm: ID block above 64 bytes");
ASSERT(ADDR(.vdm_id) == ORIGIN(FLASH) + 0x200, "vdm: ID block not at 0x08000200");
ASSERT(__vdm_boot_end <= __vdm_sector0_end, "vdm: the boot stage does not fit into sector 0 (D9)");
ASSERT(_stext % ALIGNOF(.text) == 0, "vdm: .text needs more alignment than the end of .vdm_boot");
