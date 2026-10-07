*** Settings ***
Documentation     The boot stage of one Rust STM32 image in Renode (docs/rust/GLUE-DESIGN-STM.md §5.7,
...               scenarios E1-E10). tools/rust/renode.sh runs this suite once per image with the
...               variables below. What Renode cannot show is in the test documentation.
...
...               Renode models used: STM32_UART (bytes arrive at once, CR1.M not modelled),
...               STM32F4_RCC (HSERDY follows HSEON, CSR reset flags fixed at their reset value),
...               STM32_IndependentWatchdog, the Cortex-M SysTick at a fixed frequency (the boot
...               clock), MappedMemory that keeps its content over a machine reset. The CPU runs at
...               ${MIPS} MIPS: every wait of the firmware counts SysTick or TIM5 time, so a slower
...               core only shortens the host time of the run.
Library           String
Library           Collections
Library           vdm_renode.py

*** Variables ***
${REPO}           /src
${IMAGES}         ${REPO}/software_stm32_rust/firmware/images
${IMAGE}          STM32F401_C2
${CHIP}           f401
${TAG}            C2
${VERSION}        2.2.0-revamped
${APP_RUN}        0x0
${MIPS}           4
# RAM_LO (0x20000000-0x20003233) is reserved and unused by the firmware (memory/vdm.x): the
# hooks keep their time stamps there
${T_UART}         0x20000000
${UART_CR1}       0x20000004
${T_IWDG}         0x20000008
${TX_COUNT}       0x2000000C
${UDF_AT}         0x20000100
${DEADBEEF_LF}    44 45 41 44 42 45 45 46 0A
${DEADBEEF_CRLF}  44 45 41 44 42 45 45 46 0D 0A
${DEADBEEF}       44 45 41 44 42 45 45 46

*** Keywords ***
Chip Facts
    IF    '${CHIP}' == 'f401'
        Set Test Variable    ${DEV_ID}      1059
        Set Test Variable    ${BRR_APP}     0x2D9
        Set Test Variable    ${SP0}         0x20010000
    ELSE
        Set Test Variable    ${DEV_ID}      1073
        Set Test Variable    ${BRR_APP}     0x341
        Set Test Variable    ${SP0}         0x20020000
    END

Create VdMot Machine
    [Arguments]    ${hse}=ready    ${systick_hz}=25000000
    Chip Facts
    Execute Command    mach create "vdm"
    Execute Command    machine LoadPlatformDescription @${REPO}/software_stm32_rust/renode/stm32${CHIP}.repl
    Execute Command    sysbus LoadBinary @${IMAGES}/${IMAGE}.bin 0x08000000
    Execute Command    sysbus LoadSymbolsFrom @${IMAGES}/${IMAGE}.elf
    # no flash alias at 0 in the platform: the vector table offset, also after every reset
    # (IWDG, SYSRESETREQ), when Renode runs the machine's reset macro
    Execute Command    cpu VectorTableOffset 0x08000000
    Execute Command    macro reset "cpu VectorTableOffset 0x08000000"
    Execute Command    cpu PerformanceInMips ${MIPS}
    # SysTick counts at the boot clock: 25 MHz HSE, 16 MHz HSI
    Execute Command    nvic Frequency ${systick_hz}
    # stand-in for the ROM bootloader: vector [SP 0x20002FF0, PC 0x1FFF0101], "b ." at 0x1FFF0100
    Execute Command    sysbus WriteDoubleWord 0x1FFF0000 0x20002FF0
    Execute Command    sysbus WriteDoubleWord 0x1FFF0004 0x1FFF0101
    Execute Command    sysbus WriteWord 0x1FFF0100 0xE7FE
    Execute Command    cpu AddHook 0x1FFF0100 "self.InfoLog('ROM entered')"
    # time stamps: the first USART1 CR1 write with RE (the window listens), the first IWDG_KR
    # write, the number of USART1 DR writes (bytes sent)
    ${cr1_hook}=    Catenate    SEPARATOR=\n
    ...    b = cpu.GetMachine().SystemBus
    ...    if (value & 0x4) and b.ReadDoubleWord(${T_UART}) == 0:
    ...    ${SPACE*4}b.WriteDoubleWord(${T_UART}, int(cpu.GetMachine().ElapsedVirtualTime.TimeElapsed.TotalMicroseconds))
    ...    ${SPACE*4}b.WriteDoubleWord(${UART_CR1}, value)
    Execute Command    sysbus AddWatchpointHook 0x4001100C 4 Write """${cr1_hook}"""
    ${kr_hook}=    Catenate    SEPARATOR=\n
    ...    b = cpu.GetMachine().SystemBus
    ...    cpu.InfoLog('IWDG_KR <- 0x%X' % value)
    ...    if b.ReadDoubleWord(${T_IWDG}) == 0:
    ...    ${SPACE*4}b.WriteDoubleWord(${T_IWDG}, int(cpu.GetMachine().ElapsedVirtualTime.TimeElapsed.TotalMicroseconds))
    Execute Command    sysbus AddWatchpointHook 0x40003000 4 Write """${kr_hook}"""
    Execute Command    sysbus AddWatchpointHook 0x40011004 4 Write "b = cpu.GetMachine().SystemBus; b.WriteDoubleWord(${TX_COUNT}, b.ReadDoubleWord(${TX_COUNT}) + 1)"
    IF    '${hse}' == 'dead'
        # no 25 MHz crystal: RCC_CR.HSERDY never sets
        Execute Command    sysbus SetHookAfterPeripheralRead sysbus.rcc "if offset == 0: value = value & ~0x20000"
    END
    Create Terminal Tester    sysbus.usart1    defaultPauseEmulation=true
    Create Log Tester    0

Now Us
    ${t}=    Execute Command    python "print(int(self.Machine.ElapsedVirtualTime.TimeElapsed.TotalMicroseconds))"
    ${t}=    Convert To Integer    ${t.strip()}
    RETURN    ${t}

Run Until Us
    [Arguments]    ${t_us}
    ${now}=    Now Us
    ${d}=    Evaluate    (${t_us} - ${now}) / 1000000.0
    IF    ${d} > 0
        Execute Command    emulation RunFor "${d}"
    END

Read Word
    [Arguments]    ${address}
    ${v}=    Execute Command    sysbus ReadDoubleWord ${address}
    ${v}=    Convert To Integer    ${v.strip()}
    RETURN    ${v}

Send Bytes
    [Documentation]    Bytes on the ESP line (USART1 RX), all at the current virtual time.
    [Arguments]    ${hex}
    @{bytes}=    Split String    ${hex}
    FOR    ${b}    IN    @{bytes}
        Execute Command    sysbus.usart1 WriteChar 0x${b}
    END

Send ESP 2.1 Handshake Until BEEFIT
    [Documentation]    The ESP 2.1 flasher after its NRST release: DEADBEEF LF at ${first_us}, then
    ...                every 100 ms for 2.5 s. Returns the number of sends and the BEEFIT line.
    [Arguments]    ${first_us}=20000
    FOR    ${i}    IN RANGE    25
        ${at}=    Evaluate    int(${first_us}) + 100000 * ${i}
        Run Until Us    ${at}
        Send Bytes    ${DEADBEEF_LF}
        ${ok}    ${line}=    Run Keyword And Ignore Error    Wait For Line On Uart    BEEFIT    timeout=0.099
        IF    '${ok}' == 'PASS'    RETURN    ${i + 1}    ${line}
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

Ask
    [Arguments]    ${request}    ${reply}
    Write Line To Uart    ${request}    waitForEcho=false
    ${line}=    Wait For Line On Uart    ${reply}    timeout=0.2
    Should Be Equal    ${line}[Line]    ${reply}

Expect PLL As System Clock
    ${cr}=    Read Word    0x40023800
    Should Be Equal As Integers    ${cr & 0x1000000}    0x1000000    PLLON
    ${cfgr}=    Read Word    0x40023808
    Should Be Equal As Integers    ${cfgr & 0x3}    2    SYSCLK = PLL
    Should Be Equal As Integers    ${cfgr & 0xFCF0}    0x1000    AHB /1, APB1 /2, APB2 /1

Expect Application Answers
    Ask    gvers    gvers ${VERSION}_${TAG} 1${SPACE}
    Ask    gproto    gproto 3
    Ask    ghwin    ghwin ${DEV_ID}${SPACE}

Expect Valve Outputs Off
    ${a}=    Read Word    0x40020014
    ${b}=    Read Word    0x40020414
    Should Be Equal As Integers    ${a & 0x80E0}    0    ENA0..2, ENA4 (PA5-7, PA15) off
    Should Be Equal As Integers    ${b & 0x0009}    0    ENA3, ENA5 (PB0, PB3) off
    Should Be Equal As Integers    ${b & 0x0200}    0x200    valve PSU off (PB9 high)

Switch Valve Outputs On
    Execute Command    sysbus WriteDoubleWord 0x40020018 0x000080E0
    Execute Command    sysbus WriteDoubleWord 0x40020418 0x02000009

Evidence
    [Documentation]    One line per scenario in the console output of tools/rust/renode.sh.
    [Arguments]    ${text}
    Log To Console    EVIDENCE ${IMAGE} ${text}

Forget The Previous Boot
    [Documentation]    After a reset: the IWDG writes and bytes of the boot before do not count.
    Execute Command    sysbus WriteDoubleWord ${T_IWDG} 0
    Execute Command    sysbus WriteDoubleWord ${TX_COUNT} 0

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
    Should Be Equal As Integers    ${cr1}    0x200C    8N1: UE, TE, RE, no PCE
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
    Run Until Us    3150000
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
    Should Be True    ${t} < 3100000 + 15000000
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT    first_us=${t + 20000}
    Expect Jump Into The ROM Bootloader
    ${after}=    Evaluate    (${t} - 3100000) / 1000000.0
    Evidence    E7 UDF at 3.1 s: outputs off, FaultRecord kind ${kind} CFSR.UNDEFINSTR; IWDG reset ${after} s after the fault; BEEFIT and ROM entered after it

E8 A Fault Before The IWDG Runs Ends In A Reset Within 1 s
    [Tags]    E8
    [Documentation]    A fault at the first instruction of the application stage, before it starts
    ...                the IWDG: the fault handler starts the IWDG with its reset values (512 ms).
    Create VdMot Machine
    Execute Command    sysbus WriteWord ${UDF_AT} 0xDE00
    # the hook stops the machine at the entry of app::run; the fault is raised from there
    Execute Command    cpu AddHook ${APP_RUN} "self.InfoLog('application entry'); machine.PauseAndRequestEmulationPause()"
    Wait For Log Entry    application entry    timeout=3.2    pauseEmulation=true
    ${pc}=    Execute Command    cpu PC
    Should Be Equal As Integers    ${pc.strip()}    ${APP_RUN}
    ${t_iwdg}=    Read Word    ${T_IWDG}
    Should Be Equal As Integers    ${t_iwdg}    0    the IWDG is still stopped
    Execute Command    cpu RemoveHooksAt ${APP_RUN}
    Execute Command    cpu PC ${UDF_AT}
    ${t_fault}=    Now Us
    Run Until Us    ${t_fault + 5000}
    Expect Valve Outputs Off
    Wait For Log Entry    Watchdog reset triggered    timeout=1    pauseEmulation=true
    Forget The Previous Boot
    ${t}=    Now Us
    ${after}=    Evaluate    ${t} - ${t_fault}
    Should Be True    ${after} < 1000000    reset ${after} us after the fault
    ${sends}    ${line}=    Send ESP 2.1 Handshake Until BEEFIT    first_us=${t + 20000}
    Expect Jump Into The ROM Bootloader
    Evidence    E8 UDF at the entry of app::run (IWDG stopped): outputs off, IWDG reset ${after} us after the fault, BEEFIT and ROM entered after it

E9 The No-Init Cells Survive A Warm Reset In The C++ Format
    [Tags]    E9
    [Documentation]    The C++ 2.1.7 cells (warm state area, guard with a valid CRC, counter 41) are
    ...                in RAM before the start; a pin reset (CSR hook) and then a software reset
    ...                (AIRCR.SYSRESETREQ) follow. The Rust capture continues the C++ cells (counter
    ...                42, 43; guard window summed, sealed) and leaves the warm state and the
    ...                padding byte-equal. The warm restore itself (gvlvy, no presence test) needs
    ...                the glue port: not shown here.
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
    # the skeleton runs no sysstat loop: last_uptime_s stays 0, the window stays 120 s
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
