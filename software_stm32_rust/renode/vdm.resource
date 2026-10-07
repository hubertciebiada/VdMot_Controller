*** Settings ***
Documentation     The VdMot machine of the Renode suites (boot.robot, app.robot) and the keywords
...               they share (docs/rust/GLUE-DESIGN-STM.md §5.7). tools/rust/renode.sh sets the
...               variables of the image.
...
...               Renode models used: STM32_UART (bytes arrive at once, CR1.M not modelled),
...               STM32F4_RCC (HSERDY follows HSEON, CSR reset flags fixed at their reset value
...               unless a test hooks the read), STM32_IndependentWatchdog, STM32F4_I2C, the
...               Cortex-M SysTick at a fixed frequency (the boot clock), MappedMemory that keeps
...               its content over a machine reset; the board models VdmValves.cs (ADC1 and the
...               valve motors) and VdmEeprom.cs (the 24LC64). The CPU runs at ${MIPS} MIPS:
...               every wait of the firmware counts SysTick, TIM5 or DWT time, so a slower core
...               only shortens the host time of the run.
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
${T_RESET}        0x20000010
${UDF_AT}         0x20000100
# RCC_CSR as the boot stage reads it (RM0368 6.3.21): the flags of a reset kind
${CSR_POWER_ON}   0x0E000000
${CSR_PIN}        0x04000000
${CSR_SOFTWARE}   0x14000000
${CSR_WATCHDOG}   0x24000000

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
    [Documentation]    The machine of the image, stopped at its reset. With terminal=True a second
    ...                tester reads the debug terminal (USART6): ${ESP} and ${DBG} are the tester
    ...                ids, ${ESP} stays the default. ${bin}: another image in the flash (E11:
    ...                a C++ release image), without symbols.
    [Arguments]    ${hse}=ready    ${systick_hz}=25000000    ${terminal}=${FALSE}    ${bin}=${EMPTY}
    Chip Facts
    Execute Command    mach create "vdm"
    # the C# models of the board (valves behind ADC1, the 24LC64), compiled by Renode
    Execute Command    include @${REPO}/software_stm32_rust/renode/VdmValves.cs
    Execute Command    include @${REPO}/software_stm32_rust/renode/VdmEeprom.cs
    Execute Command    machine LoadPlatformDescription @${REPO}/software_stm32_rust/renode/stm32${CHIP}.repl
    IF    '${TAG}' == 'C1'
        Execute Command    sysbus.valves MuxOnHigh true
    END
    IF    '${bin}' == ''
        Execute Command    sysbus LoadBinary @${IMAGES}/${IMAGE}.bin 0x08000000
        Execute Command    sysbus LoadSymbolsFrom @${IMAGES}/${IMAGE}.elf
    ELSE
        Execute Command    sysbus LoadBinary @${bin} 0x08000000
    END
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
    # write, the number of USART1 DR writes (bytes sent), the last reset (the entry of the
    # reset handler)
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
    ${vector}=    Read Word    0x08000004
    ${entry}=    Evaluate    ${vector} & ~1
    Execute Command    cpu AddHook ${entry} "machine.SystemBus.WriteDoubleWord(${T_RESET}, int(machine.ElapsedVirtualTime.TimeElapsed.TotalMicroseconds))"
    IF    '${hse}' == 'dead'
        # no 25 MHz crystal: RCC_CR.HSERDY never sets
        Execute Command    sysbus SetHookAfterPeripheralRead sysbus.rcc "if offset == 0: value = value & ~0x20000"
    END
    ${esp}=    Create Terminal Tester    sysbus.usart1    defaultPauseEmulation=true
    Set Test Variable    ${ESP}    ${esp}
    IF    ${terminal}
        ${dbg}=    Create Terminal Tester    sysbus.usart6    defaultPauseEmulation=true
        Set Test Variable    ${DBG}    ${dbg}
        Set Default Tester    ${esp}
    END
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
    [Arguments]    ${hex}    ${uart}=usart1
    @{bytes}=    Split String    ${hex}
    FOR    ${b}    IN    @{bytes}
        Execute Command    sysbus.${uart} WriteChar 0x${b}
    END

Ask
    [Documentation]    A request of the ESP and its reply line. The application answers from its
    ...                main loop: a request sent during its set-up (500 ms delay, EEPROM, 1-Wire)
    ...                waits in the receive ring, so the timeout covers the set-up.
    [Arguments]    ${request}    ${reply}    ${timeout}=1.0
    Write Line To Uart    ${request}    waitForEcho=false    testerId=${ESP}
    ${line}=    Wait For Line On Uart    ${reply}    timeout=${timeout}    testerId=${ESP}
    Should Be Equal    ${line}[Line]    ${reply}

Exchange
    [Documentation]    A request of the ESP (LF as the ESP sends it) and the next reply line.
    [Arguments]    ${request}    ${timeout}=0.5
    ${hex}=    Evaluate    " ".join("%02X" % b for b in $request.encode() + b"\\n")
    Send Bytes    ${hex}
    ${line}=    Wait For Next Line On Uart    timeout=${timeout}    testerId=${ESP}
    RETURN    ${line}[Line]

Expect Application Answers
    Ask    gvers    gvers ${VERSION}_${TAG} 1${SPACE}
    Ask    gproto    gproto 3
    Ask    ghwin    ghwin ${DEV_ID}${SPACE}

Set Next Reset Flags
    [Documentation]    RCC_CSR as the next boots read it (Renode keeps the reset value).
    [Arguments]    ${flags}
    Execute Command    sysbus SetHookAfterPeripheralRead sysbus.rcc "if offset == 0x74: value = ${flags}"

Evidence
    [Documentation]    One line per scenario in the console output of tools/rust/renode.sh.
    [Arguments]    ${text}
    Log To Console    EVIDENCE ${IMAGE} ${text}

Forget The Previous Boot
    [Documentation]    After a reset: the IWDG writes and bytes of the boot before do not count.
    Execute Command    sysbus WriteDoubleWord ${T_IWDG} 0
    Execute Command    sysbus WriteDoubleWord ${TX_COUNT} 0
