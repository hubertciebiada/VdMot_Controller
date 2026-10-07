*** Settings ***
Documentation     The boot stage of one Rust STM32 image in Renode (docs/rust/GLUE-DESIGN-STM.md §5.7,
...               scenarios E1-E10, E12). tools/rust/renode.sh runs this suite once per image; the
...               machine, the models and the variables are in vdm.resource. What Renode cannot
...               show is in the test documentation.
Resource          vdm.resource

*** Variables ***
${DEADBEEF_LF}    44 45 41 44 42 45 45 46 0A
${DEADBEEF_CRLF}  44 45 41 44 42 45 45 46 0D 0A
${DEADBEEF}       44 45 41 44 42 45 45 46

*** Keywords ***
Send ESP 2.1 Handshake Until BEEFIT
    [Documentation]    The ESP 2.1 flasher after its NRST release: DEADBEEF LF at ${first_us}, then
    ...                every 100 ms for 2.5 s. Returns the number of sends and the BEEFIT line.
    ...                The bytes counter tells when the 8 bytes of BEEFIT are out, so no wait for
    ...                the line times out (Renode's tester can leave the emulation running after
    ...                a timed-out wait and drops the line that follows it).
    [Arguments]    ${first_us}=20000
    FOR    ${i}    IN RANGE    25
        ${at}=    Evaluate    int(${first_us}) + 100000 * ${i}
        Run Until Us    ${at}
        Send Bytes    ${DEADBEEF_LF}
        Run Until Us    ${at + 99000}
        ${tx}=    Read Word    ${TX_COUNT}
        IF    ${tx} >= 8
            ${line}=    Wait For Line On Uart    BEEFIT    timeout=0.05
            RETURN    ${i + 1}    ${line}
        END
    END
    Fail    no BEEFIT within 2.5 s

Expect Jump Into The ROM Bootloader
    Wait For Log Entry    ROM entered    timeout=0.3    pauseEmulation=true
    ${sp}=    Execute Command    cpu SP
    Should Be Equal As Integers    ${sp.strip()}    0x20002FF0
    ${memrmp}=    Read Word    0x40013800
    Should Be Equal As Integers    ${memrmp}    1    MEMRMP = 01: system memory at address 0
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be Equal As Integers    ${t_iwdg}    0    the IWDG was written before the jump (B6)

Expect Window Clock And Framing
    [Arguments]    ${brr}
    ${p}=    Execute Command    sysbus.usart1 ParityBit
    ${p}=    Get Line    ${p.strip()}    0
    Should Be Equal    ${p}    Even
    ${b}=    Read Word    0x40011008
    Should Be Equal As Integers    ${b}    ${brr}
    # 8E1 as written by the firmware: UE, M (9 bits with parity), PCE, PS even, TE, RE.
    # Renode keeps no M bit, so the written value is taken from the CR1 hook.
    ${cr1}=    Read Word    ${UART_CR1}
    Should Be Equal As Integers    ${cr1}    0x340C

Expect Window Opens Early
    [Documentation]    E10: USART1 receives at most 10 ms (+5 ms HSE probe) after reset.
    ${t}=    Read Word    ${T_UART}
    Should Be True    0 < ${t} <= 15000    USART1 RE set at ${t} us after reset

Expect PLL As System Clock
    ${cr}=    Read Word    0x40023800
    Should Be Equal As Integers    ${cr & 0x1000000}    0x1000000    PLLON
    ${cfgr}=    Read Word    0x40023808
    Should Be Equal As Integers    ${cfgr & 0x3}    2    SYSCLK = PLL
    Should Be Equal As Integers    ${cfgr & 0xFCF0}    0x1000    AHB /1, APB1 /2, APB2 /1

Expect Valve Outputs Off
    ${a}=    Read Word    0x40020014
    ${b}=    Read Word    0x40020414
    Should Be Equal As Integers    ${a & 0x80E0}    0    ENA0..2, ENA4 (PA5-7, PA15) off
    Should Be Equal As Integers    ${b & 0x0009}    0    ENA3, ENA5 (PB0, PB3) off
    Should Be Equal As Integers    ${b & 0x0200}    0x200    valve PSU off (PB9 high)

Switch Valve Outputs On
    Execute Command    sysbus WriteDoubleWord 0x40020018 0x000080E0
    Execute Command    sysbus WriteDoubleWord 0x40020418 0x02000009

Read Noinit Words
    [Arguments]    ${cells}
    @{words}=    Create List
    FOR    ${pair}    IN    @{cells}
        ${w}=    Execute Command    sysbus ReadDoubleWord ${pair}[0]
        Append To List    ${words}    ${w}
    END
    RETURN    ${words}

Software Reset From The CPU
    [Documentation]    AIRCR.SYSRESETREQ written by the CPU (as SCB::sys_reset), from code in
    ...                RAM_LO: ldr r0, =AIRCR; ldr r1, =0x05FA0004; str r1, [r0]; dsb; b .
    Execute Command    sysbus WriteWord 0x20000200 0x4802
    Execute Command    sysbus WriteWord 0x20000202 0x4903
    Execute Command    sysbus WriteWord 0x20000204 0x6001
    Execute Command    sysbus WriteWord 0x20000206 0xF3BF
    Execute Command    sysbus WriteWord 0x20000208 0x8F4F
    Execute Command    sysbus WriteWord 0x2000020A 0xE7FE
    Execute Command    sysbus WriteDoubleWord 0x2000020C 0xE000ED0C
    Execute Command    sysbus WriteDoubleWord 0x20000210 0x05FA0004
    Execute Command    cpu PC 0x20000200

*** Test Cases ***
E1 ESP 2.1 Handshake Enters The ROM Bootloader
    [Tags]    E1
    [Documentation]    NRST release at 0, DEADBEEF LF at 20 ms and every 100 ms: the first send
    ...                is aligned, BEEFIT within 15 ms, the jump with the ROM's SP, MEMRMP = 01,
    ...                no IWDG write before it. USART1 8E1 at 115200 on the HSE (BRR 0xD9).
    Create VdMot Machine
    Run Until Us    20000
    Expect Window Clock And Framing    0xD9
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT
    Should Be Equal As Integers    ${sends}    1
    Should Be Equal    ${line}[Line]    BEEFIT
    Should Be True    ${line}[Timestamp] <= 35.0    BEEFIT at ${line}[Timestamp] ms
    Expect Jump Into The ROM Bootloader
    Expect Window Opens Early
    ${t0}=    Read Word    ${T_UART}
    Evidence    E1 8E1 (CR1 0x340C) BRR 0xD9 from ${t0} us; DEADBEEF LF at 20 ms -> BEEFIT at ${line}[Timestamp] ms; ROM entered, SP 0x20002FF0, MEMRMP 1, no IWDG write

E2 A Stray Byte Realigns Within 8 Sends
    [Tags]    E2
    [Documentation]    One byte at 15 ms (after the 10 ms drop), then the ESP 2.1 pattern: the
    ...                fixed 8-byte blocks meet DEADBEEF on the 8th send ((1 + 9 x 7) mod 8 = 0).
    Create VdMot Machine
    Run Until Us    15000
    Send Bytes    00
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT
    Should Be Equal As Integers    ${sends}    8
    Expect Jump Into The ROM Bootloader
    Evidence    E2 stray byte at 15 ms -> BEEFIT after send ${sends} at ${line}[Timestamp] ms, ROM entered

E3 Legacy ESP 1.x
    [Tags]    E3
    [Documentation]    DEADBEEF CR LF once, 1.5 s after reset.
    Create VdMot Machine
    Run Until Us    1500000
    Send Bytes    ${DEADBEEF_CRLF}
    ${line}=    Wait For Line On Uart    BEEFIT    timeout=0.05
    Expect Jump Into The ROM Bootloader
    Evidence    E3 DEADBEEF CR LF at 1500 ms -> BEEFIT at ${line}[Timestamp] ms, ROM entered

E4 DEADBEEF In The Last Call Of The Window
    [Tags]    E4
    [Documentation]    The window polls in calls of 1 ms after the 10 ms drop; the USART1 write
    ...                marks the start t0 of the drop, call 3001 starts at t0 + 3010 ms. DEADBEEF
    ...                that is complete during call 3000 is read in call 3001 and still answered.
    Create VdMot Machine
    Run Until Us    20000
    ${t0}=    Read Word    ${T_UART}
    Run Until Us    ${t0 + 3009500}
    Send Bytes    ${DEADBEEF}
    ${line}=    Wait For Line On Uart    BEEFIT    timeout=0.05
    Expect Jump Into The ROM Bootloader
    Evidence    E4 DEADBEEF at t0 + 3009.5 ms (call 3000) -> BEEFIT at ${line}[Timestamp] ms, ROM entered

E4 DEADBEEF One Call After The Window Starts The Application
    [Tags]    E4
    Create VdMot Machine
    Run Until Us    20000
    ${t0}=    Read Word    ${T_UART}
    Run Until Us    ${t0 + 3010500}
    Send Bytes    ${DEADBEEF}
    Run Until Us    ${t0 + 3100000}
    ${tx}=    Read Word    ${TX_COUNT}
    Should Be Equal As Integers    ${tx}    0
    Expect Application Answers
    Evidence    E4 DEADBEEF at t0 + 3010.5 ms (call 3001) -> no BEEFIT, the application answers

E5 Without Handshake The Application Starts
    [Tags]    E5
    [Documentation]    Nothing for 3.1 s: no byte sent in the window, then USART1 at 8N1 with the
    ...                application clock (BRR ${BRR_APP}), gvers/gproto/ghwin as C++ 2.1.7 (version
    ...                of D8), the IWDG started only after the window (B6) and fed: no watchdog reset
    ...                in 10 s.
    Create VdMot Machine
    Run Until Us    3100000
    ${tx}=    Read Word    ${TX_COUNT}
    Should Be Equal As Integers    ${tx}    0    no byte sent in the window
    ${cr1}=    Read Word    0x4001100C
    # TXEIE (0x80) is on while a byte waits in the transmit ring
    Should Be Equal As Integers    ${cr1 & 0xFF7F}    0x202C    8N1: UE, TE, RE, RXNEIE, no PCE
    ${brr}=    Read Word    0x40011008
    Should Be Equal As Integers    ${brr}    ${BRR_APP}
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be True    ${t_iwdg} >= 3010000    first IWDG write at ${t_iwdg} us
    Expect PLL As System Clock
    ${pll}=    Read Word    0x40023804
    Should Be Equal As Integers    ${pll & 0x400000}    0x400000    PLLSRC = HSE
    Should Be Equal As Integers    ${pll & 0x3F}    25    PLLM = 25
    Expect Application Answers
    Run Until Us    13200000
    Should Not Be In Log    Watchdog reset triggered    timeout=0
    Expect Application Answers
    Expect Window Opens Early
    Evidence    E5 no TX in the window; PLL from HSE (M 25) as SYSCLK, CR1 0x200C (8N1), BRR ${BRR_APP}; first IWDG write at ${t_iwdg} us; "gvers ${VERSION}_${TAG} 1 ", "gproto 3", "ghwin ${DEV_ID} "; no IWDG reset in 10 s

E6 Dead HSE The Window Runs On HSI
    [Tags]    E6
    [Documentation]    HSERDY never sets: after 5 ms the boot stage stays on HSI 16 MHz (BRR 0x8B),
    ...                the handshake works as in E1.
    Create VdMot Machine    hse=dead    systick_hz=16000000
    Run Until Us    20000
    Expect Window Clock And Framing    0x8B
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT
    Should Be Equal As Integers    ${sends}    1
    Expect Jump Into The ROM Bootloader
    Expect Window Opens Early
    ${cr}=    Read Word    0x40023800
    Should Be Equal As Integers    ${cr & 0x10000}    0    HSEON switched off again
    ${t0}=    Read Word    ${T_UART}
    Evidence    E6 HSERDY held 0: HSEON off, window on HSI from ${t0} us (BRR 0x8B), BEEFIT at ${line}[Timestamp] ms, ROM entered

E6 Dead HSE The Application Runs On The PLL From HSI
    [Tags]    E6
    [Documentation]    D5: the application probes the HSE for 100 ms and sets the PLL up from the
    ...                HSI (PLLSRC 0, PLLM 16) with the same frequencies.
    Create VdMot Machine    hse=dead    systick_hz=16000000
    Run Until Us    3300000
    Expect PLL As System Clock
    ${cr}=    Read Word    0x40023800
    Should Be Equal As Integers    ${cr & 0x10000}    0    HSEON off
    # F411 from HSI (M 16, N 192, P 2, Q 4) is the reset value of PLLCFGR; SW = PLL and the
    # application BRR show that it runs
    ${pll}=    Read Word    0x40023804
    Should Be Equal As Integers    ${pll & 0x400000}    0    PLLSRC = HSI
    Should Be Equal As Integers    ${pll & 0x3F}    16    PLLM = 16
    ${brr}=    Read Word    0x40011008
    Should Be Equal As Integers    ${brr}    ${BRR_APP}
    Expect Application Answers
    ${pll_hex}=    Convert To Hex    ${pll}    prefix=0x
    Evidence    E6 application: PLLCFGR ${pll_hex} (PLLSRC HSI, M 16), BRR ${BRR_APP}, gvers/gproto/ghwin answered

E7 A Fault In The Application Ends In An IWDG Reset
    [Tags]    E7
    [Documentation]    UDF in the running application: valve outputs off, a valid FaultRecord, no
    ...                reload, the IWDG (8 s) resets the chip within 15 s, the boot window works
    ...                again. Renode keeps the RCC_CSR flags at their reset value, so the next boot
    ...                cannot be seen as an IWDG reset here.
    Create VdMot Machine
    Run Until Us    3100000
    Expect Application Answers
    Switch Valve Outputs On
    Execute Command    sysbus WriteWord ${UDF_AT} 0xDE00
    Execute Command    cpu PC ${UDF_AT}
    # the answers came from the main loop, after the set-up: the fault time is now
    ${t_fault}=    Now Us
    Run Until Us    ${t_fault + 50000}
    Expect Valve Outputs Off
    # the FaultRecord is the only object in .uninit
    ${record}=    Execute Command    python "print(self.Machine.SystemBus.GetSymbolAddress('__suninit'))"
    ${record}=    Strip String    ${record}
    @{words}=    Create List
    FOR    ${i}    IN RANGE    10
        ${addr}=    Evaluate    int("${record}".rstrip("L")) + 4 * ${i}
        ${w}=    Execute Command    sysbus ReadDoubleWord ${addr}
        Append To List    ${words}    ${w}
    END
    ${valid}    ${kind}    ${pc}    ${cfsr}=    Fault Record    ${words}
    Should Be True    ${valid}    FaultRecord check
    # the hardware escalates the UsageFault (disabled in SHCSR) to HardFault; Renode takes the
    # UsageFault vector. Either way CFSR.UNDEFINSTR names the UDF.
    Should Be True    ${kind} in (1, 5)    record kind ${kind}
    Should Be Equal As Integers    ${cfsr & 0x10000}    0x10000    CFSR.UNDEFINSTR
    IF    ${kind} == 1
        Should Be Equal As Integers    ${pc}    ${UDF_AT}
    END
    Wait For Log Entry    Watchdog reset triggered    timeout=15    pauseEmulation=true
    Forget The Previous Boot
    ${t}=    Now Us
    Should Be True    ${t} < ${t_fault} + 15000000
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT    first_us=${t + 20000}
    Expect Jump Into The ROM Bootloader
    ${after}=    Evaluate    (${t} - ${t_fault}) / 1000000.0
    ${at}=    Evaluate    ${t_fault} / 1000000.0
    Evidence    E7 UDF at ${at} s: outputs off, FaultRecord kind ${kind} CFSR.UNDEFINSTR; IWDG reset ${after} s after the fault; BEEFIT and ROM entered after it

E8 Code After The Window Runs Under The IWDG
    [Tags]    E8
    [Documentation]    D9 residual: after an interrupted flash of sectors 1..n the old boot stage
    ...                calls new or erased code at the old address of app::run. The boot stage
    ...                starts the IWDG (8 s) at the end of the window, so code that hangs at the
    ...                entry of app::run (here a "b ." in RAM, before the application stage sets
    ...                the IWDG up itself) still ends in a watchdog reset and the next window.
    Create VdMot Machine
    Execute Command    sysbus WriteWord ${UDF_AT} 0xE7FE
    # the hook stops the machine at the entry of app::run; the hang starts from there
    Execute Command    cpu AddHook ${APP_RUN} "self.InfoLog('application entry'); machine.PauseAndRequestEmulationPause()"
    Wait For Log Entry    application entry    timeout=3.2    pauseEmulation=true
    ${pc}=    Execute Command    cpu PC
    Should Be Equal As Integers    ${pc.strip()}    ${APP_RUN}
    ${t0}=    Read Word    ${T_UART}
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be True    ${t_iwdg} >= ${t0} + 3010000    IWDG started at ${t_iwdg} us, after the window (B6)
    Execute Command    cpu RemoveHooksAt ${APP_RUN}
    Execute Command    cpu PC ${UDF_AT}
    ${t_hang}=    Now Us
    Run Until Us    ${t_hang + 5000}
    Expect Valve Outputs Off
    Wait For Log Entry    Watchdog reset triggered    timeout=9    pauseEmulation=true
    Forget The Previous Boot
    ${t}=    Now Us
    ${after}=    Evaluate    ${t} - ${t_iwdg}
    # 8 s (prescaler /64, reload 3999 at 32 kHz), not the 512 ms of the reset values
    Should Be True    7900000 <= ${after} <= 8100000    reset ${after} us after the IWDG start
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT    first_us=${t + 20000}
    Expect Jump Into The ROM Bootloader
    Evidence    E8 hang at the entry of app::run: IWDG started at ${t_iwdg} us (end of the window), reset ${after} us after it, BEEFIT and ROM entered after it

E8 A Fault Before The IWDG Runs Ends In A Reset Within 1 s
    [Tags]    E8
    [Documentation]    A fault at the entry of the boot stage (B3 excludes one; this proves the
    ...                handler): the IWDG is stopped, the fault handler starts it with its reset
    ...                values (512 ms) and the next boot opens the window.
    Create VdMot Machine
    Execute Command    sysbus WriteWord ${UDF_AT} 0xDE00
    # the hook stops the machine at the entry of boot_hw::run; the fault is raised from there
    Execute Command    cpu AddHook ${BOOT_RUN} "self.InfoLog('boot stage entry'); machine.PauseAndRequestEmulationPause()"
    Wait For Log Entry    boot stage entry    timeout=0.1    pauseEmulation=true
    ${pc}=    Execute Command    cpu PC
    Should Be Equal As Integers    ${pc.strip()}    ${BOOT_RUN}
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be Equal As Integers    ${t_iwdg}    0    the IWDG is still stopped
    Execute Command    cpu RemoveHooksAt ${BOOT_RUN}
    Execute Command    cpu PC ${UDF_AT}
    ${t_fault}=    Now Us
    Wait For Log Entry    Watchdog reset triggered    timeout=1    pauseEmulation=true
    Forget The Previous Boot
    ${t}=    Now Us
    ${after}=    Evaluate    ${t} - ${t_fault}
    Should Be True    ${after} < 1000000    reset ${after} us after the fault
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT    first_us=${t + 20000}
    Expect Jump Into The ROM Bootloader
    Evidence    E8 UDF at the entry of boot_hw::run (IWDG stopped): IWDG reset ${after} us after the fault, BEEFIT and ROM entered after it

E9 The No-Init Cells Survive A Warm Reset In The C++ Format
    [Tags]    E9
    [Documentation]    The C++ 2.1.7 cells (warm state area, guard with a valid CRC, counter 41) are
    ...                in RAM before the start; a pin reset (CSR hook) and then a software reset
    ...                (AIRCR.SYSRESETREQ) follow. The Rust capture continues the C++ cells (counter
    ...                42, 43; guard window summed, sealed) and leaves the warm state and the
    ...                padding byte-equal. The warm restore by the application: app.robot A4.
    Create VdMot Machine
    Execute Command    sysbus SetHookAfterPeripheralRead sysbus.rcc "if offset == 0x74: value = 0x04000000"
    ${cells}=    Cpp Noinit Words    41    2    100    20
    FOR    ${pair}    IN    @{cells}
        Execute Command    sysbus WriteDoubleWord ${pair}[0] ${pair}[1]
    END
    Run Until Us    50000
    ${after}=    Read Noinit Words    ${cells}
    ${first}=    Check Noinit After Warm Boot    ${cells}    ${after}    42    120
    Run Until Us    3100000
    Software Reset From The CPU
    Run Until Us    3200000
    ${after}=    Read Noinit Words    ${cells}
    # the reset comes in the set-up of the application (its 500 ms delay), before its first
    # sysstat loop: last_uptime_s stays 0, the window stays 120 s
    ${second}=    Check Noinit After Warm Boot    ${cells}    ${after}    43    120
    Evidence    E9 C++ cells (counter 41, guard 2/100+20 s) -> pin reset: ${first}; software reset: ${second}; warm state area and padding byte-equal

E10 The Window Opens Within 10 ms Of Reset
    [Tags]    E10
    [Documentation]    Regression fence: code put before the window fails this (and E1, E6).
    Create VdMot Machine
    Run Until Us    20000
    Expect Window Opens Early
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be Equal As Integers    ${t_iwdg}    0
    ${sp0}=    Read Word    0x08000000
    Should Be Equal As Integers    ${sp0}    ${SP0}    initial SP of the image = RAM top of the chip
    ${t0}=    Read Word    ${T_UART}
    Evidence    E10 USART1 RE set ${t0} us after reset (limit 15000), no IWDG write

E12 A Half-Flashed Image Keeps The Window And Starts No Application
    [Tags]    E12
    [Documentation]    D9: the old sector 0 in front of sectors 1..n that are not its own (a flash
    ...                that failed or stopped before sector 0; here one word of sector 2
    ...                changed). The boot stage checks the record of the application part in the
    ...                idle time of the window, starts neither the application nor the IWDG and
    ...                opens the next window, for ever; the ESP's handshake in the third window
    ...                enters the ROM bootloader. With the flash complete again, the next reset
    ...                starts the application.
    Create VdMot Machine
    ${word}=    Read Word    0x08008000
    ${changed}=    Evaluate    ${word} ^ 0x00010000
    Execute Command    sysbus WriteDoubleWord 0x08008000 ${changed}
    Execute Command    cpu AddHook ${APP_RUN} "self.InfoLog('application entry')"
    Run Until Us    6500000
    Should Not Be In Log    application entry    timeout=0
    Should Not Be In Log    Watchdog reset triggered    timeout=0
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be Equal As Integers    ${t_iwdg}    0    no IWDG write (B6)
    ${tx}=    Read Word    ${TX_COUNT}
    Should Be Equal As Integers    ${tx}    0    no byte sent
    # the third window listens: 8E1 on the boot clock, not the 8N1 of the application
    Expect Window Clock And Framing    0xD9
    Expect Valve Outputs Off
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT    first_us=6500000
    Expect Jump Into The ROM Bootloader
    # the ESP's next attempt completes the flash, then its NRST
    Execute Command    sysbus WriteDoubleWord 0x08008000 ${word}
    Execute Command    machine Reset
    Forget The Previous Boot
    ${t}=    Now Us
    Run Until Us    ${t + 3500000}
    Expect Application Answers
    Wait For Log Entry    application entry    timeout=1    pauseEmulation=true
    Evidence    E12 sector 2 changed: no application and no IWDG for 6.5 s (windows 1-3), BEEFIT at ${line}[Timestamp] ms and ROM entered; restored: the application answers after the next reset
