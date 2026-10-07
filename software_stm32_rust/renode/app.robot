*** Settings ***
Documentation     The application of one Rust STM32 image in Renode (docs/rust/GLUE-DESIGN-STM.md
...               §5.7, scenarios A1-A6) against the C++ 2.1.7 goldens of the glue_system suites
...               (glue/tests/golden): the requests of a golden boot go to USART1 at their times
...               after the reset and every reply line must be the golden's (the C++ identity
...               2.1.7-revamped_C2 replaced by the image's), the debug terminal (USART6) must
...               print the golden's lines in the same order, the EEPROM model must hold the
...               golden's bytes at the end of a boot. The valve motors are VdmValves.cs with the
...               defaults of the C++ valve sim, the 24LC64 is VdmEeprom.cs. tools/rust/renode.sh
...               runs this suite after boot.robot; the machine and the variables are in
...               vdm.resource.
Resource          vdm.resource

*** Variables ***
${TIM2_CR1}       0x40000000

*** Keywords ***
Replay Golden Boot
    [Documentation]    The requests of one boot of a golden at their times after the reset at
    ...                ${t0} us; each reply line must be the golden's. Returns the reply count.
    [Arguments]    ${slug}    ${index}    ${t0}
    @{events}=    Golden Boot Events    ${slug}    ${index}    ${VERSION}_${TAG}
    ${replies}=    Set Variable    ${0}
    FOR    ${e}    IN    @{events}
        IF    '${e}[0]' == 'rx'
            ${at}=    Evaluate    ${t0} + ${e}[1] * 1000
            Run Until Us    ${at}
            Send Bytes    ${e}[2]
        ELSE
            ${line}=    Wait For Next Line On Uart    timeout=0.5    testerId=${ESP}
            Should Be Equal    ${line}[Line]    ${e}[2]    reply of the golden at ${e}[1] ms
            ${replies}=    Evaluate    ${replies} + 1
        END
    END
    RETURN    ${replies}

Terminal Lines
    [Documentation]    The next ${count} lines of the debug terminal (fewer when one does not come
    ...                within 10 ms). Renode's tester drops the first line after a wait that timed
    ...                out, so the tests never wait for a line that may not come.
    [Arguments]    ${count}
    @{lines}=    Create List
    FOR    ${i}    IN RANGE    ${count}
        ${ok}    ${line}=    Run Keyword And Ignore Error
        ...    Wait For Next Line On Uart    timeout=0.01    testerId=${DBG}
        IF    '${ok}' != 'PASS'    BREAK
        Append To List    ${lines}    ${line}
    END
    RETURN    ${lines}

Expect Golden Terminal
    [Documentation]    The terminal lines of a boot (reset at ${t0} us) are the golden's; returns
    ...                the largest time difference of a golden chunk start, in ms.
    [Arguments]    ${slug}    ${index}    ${t0}
    ${golden}=    Golden Terminal Lines    ${slug}    ${index}    ${VERSION}_${TAG}
    ${n}=    Get Length    ${golden}
    ${lines}=    Terminal Lines    ${n}
    ${t0_ms}=    Evaluate    ${t0} / 1000.0
    ${worst}=    Compare Terminal    ${golden}    ${lines}    ${t0_ms}
    RETURN    ${worst}

Expect Golden EEPROM
    [Documentation]    The EEPROM model holds the golden's bytes at the end of a boot; returns the
    ...                number of rows (32 bytes) that are not erased.
    [Arguments]    ${slug}    ${index}
    ${dump}=    Execute Command    sysbus.i2c1.eeprom Hex 0 8192
    ${rows}=    Eeprom Rows    ${dump}
    ${golden}=    Golden Eeprom Rows    ${slug}    ${index}
    Lists Should Be Equal    ${rows}    ${golden}
    ${n}=    Get Length    ${rows}
    RETURN    ${n}

Load Golden State
    [Documentation]    What C++ 2.1.7 left at the end of a golden boot: the no-init region in RAM
    ...                and the EEPROM content.
    [Arguments]    ${slug}    ${index}
    @{words}=    Golden Noinit Words    ${slug}    ${index}
    FOR    ${pair}    IN    @{words}
        Execute Command    sysbus WriteDoubleWord ${pair}[0] ${pair}[1]
    END
    @{rows}=    Golden Eeprom Load    ${slug}    ${index}
    FOR    ${row}    IN    @{rows}
        Execute Command    sysbus.i2c1.eeprom LoadHex ${row}[0] "${row}[1]"
    END

Arm Reset Mark
    [Documentation]    The next start of the reset handler writes its time to ${T_RESET}.
    Execute Command    sysbus WriteDoubleWord ${T_RESET} 0

Wait For Reset
    [Documentation]    Runs in steps of 50 ms until the reset handler started again (Arm Reset Mark
    ...                before); returns the time of that reset in us.
    [Arguments]    ${steps}=60
    FOR    ${i}    IN RANGE    ${steps}
        Execute Command    emulation RunFor "0.05"
        ${t}=    Read Word    ${T_RESET}
        IF    ${t} > 0    RETURN    ${t}
    END
    Fail    no reset within ${steps} x 50 ms

Power Cycle
    [Documentation]    Power off and on: the no-init region random (0xA5 as in the C++ bench), the
    ...                power-on flags in RCC_CSR; the EEPROM and the valves keep their state.
    ...                Returns the time of the reset in us.
    Set Next Reset Flags    ${CSR_POWER_ON}
    FOR    ${i}    IN RANGE    53
        ${a}=    Evaluate    0x20003234 + 4 * ${i}
        Execute Command    sysbus WriteDoubleWord ${a} 0xA5A5A5A5
    END
    Arm Reset Mark
    Execute Command    machine Reset
    ${t}=    Wait For Reset
    RETURN    ${t}

Stall The Valve Loop
    [Documentation]    TIM2 (the valve loop, 10 ms) stops counting; the main loop goes on. Returns
    ...                the time in us.
    Execute Command    sysbus WriteDoubleWord ${TIM2_CR1} 0
    ${t}=    Now Us
    RETURN    ${t}

Expect Golden Terminal Head
    [Documentation]    The first terminal lines of the last start, up to its EEPROM load, are the
    ...                golden's: from the first banner line in the tester on (the lines of the
    ...                boot before are dropped).
    [Arguments]    ${slug}    ${index}
    @{head}=    Golden Terminal Head    ${slug}    ${index}    ${VERSION}_${TAG}
    ${banner}=    Wait For Line On Uart    VdMot Controller    timeout=0.01    testerId=${DBG}
    ${n}=    Get Length    ${head}
    @{rest}=    Terminal Lines    ${n - 1}
    @{texts}=    Create List    ${banner}[Line]
    FOR    ${line}    IN    @{rest}
        Append To List    ${texts}    ${line}[Line]
    END
    Lists Should Be Equal    ${texts}    ${head}
    RETURN    ${head}

Valve Enables
    [Documentation]    The enables of every valve since the machine was created.
    @{enables}=    Create List
    FOR    ${v}    IN RANGE    12
        ${n}=    Execute Command    sysbus.valves GetEnables ${v}
        ${n}=    Convert To Integer    ${n.strip()}
        Append To List    ${enables}    ${n}
    END
    RETURN    ${enables}

*** Test Cases ***
A1 A New Controller Answers Like C++ 2.1.7
    [Tags]    A1
    [Documentation]    Erased EEPROM, every valve connected (C++ boot suite: 0.2 pulses/ms): the
    ...                requests of the golden (gvers, gproto, gtgtp, gvlvd under the presence test,
    ...                gstat, stgtp, gtgtp) at its times, the replies and the terminal lines equal;
    ...                then the terminal's own gvers on USART6.
    ${slug}=    Set Variable    boot__a_new_controller_answers_gvers_gproto_gtgtp_gvlvd_gstat_and_stgt
    Create VdMot Machine    terminal=${TRUE}
    ${replies}=    Replay Golden Boot    ${slug}    0    0
    Run Until Us    4000000
    ${worst}=    Expect Golden Terminal    ${slug}    0    0
    Send Bytes    67 76 65 72 73 0A    uart=usart6
    ${line}=    Wait For Next Line On Uart    timeout=0.5    testerId=${DBG}
    Should Be Equal    ${line}[Line]    Version: ${VERSION}
    Evidence    A1 ${replies} replies equal to the golden (gvers ${VERSION}_${TAG}, gproto, gtgtp, gvlvd, gstat, stgtp, gtgtp); terminal lines equal, chunk starts within ${worst} ms; terminal gvers on USART6 answered

A2 The Presence Test Finds Every Connected Valve
    [Tags]    A2
    [Documentation]    Valve 5 open, the others connected (0.2 pulses/ms): the presence test runs
    ...                over the twelve valves in the golden's order and times (EXTI4 pulses of
    ...                VdmValves, the motor current through ADC1), gvlst at 43.5 s as the golden;
    ...                never two L293 enables at once.
    ${slug}=    Set Variable    boot__the_presence_test_finds_every_connected_valve_an_open_one_is_rep
    Create VdMot Machine    terminal=${TRUE}
    Execute Command    sysbus.valves SetConnected 5 false
    ${replies}=    Replay Golden Boot    ${slug}    0    0
    ${worst}=    Expect Golden Terminal    ${slug}    0    0
    ${conflicts}=    Execute Command    sysbus.valves Conflicts
    Should Be Equal As Integers    ${conflicts.strip()}    0
    @{enables}=    Valve Enables
    FOR    ${v}    IN RANGE    12
        Should Be True    ${enables}[${v}] > 0    valve ${v} never enabled
        IF    ${v} != 5
            ${p}=    Execute Command    sysbus.valves GetPulses ${v}
            Should Be True    ${p.strip()} > 0    valve ${v} gave no pulse
        END
    END
    ${summary}=    Execute Command    sysbus.valves Summary
    Evidence    A2 gvlst 12 8,8,8,8,8,6,8,8,8,8,8,8 as the golden; 12 presence tests in its order, chunk starts within ${worst} ms; enables/pulses/position per valve ${summary.strip()}; no enable conflict

A3 The Configuration Survives A Reset And A Power Cycle
    [Tags]    A3
    [Documentation]    K1-6/S3-2 (2.0 pulses/ms): slcfg, sfspo, stlnt; the reset command (a real
    ...                SYSRESETREQ, the boot stage, the warm start); glcfg/gtlnt and new values;
    ...                a power cycle; glcfg, gtlnt, gstax. Replies and terminal lines of every boot
    ...                as the golden's, the EEPROM bytes after every boot as the golden's.
    ${slug}=    Set Variable    config__k1_6_s3_2_slcfg_sfspo_and_stlnt_survive_a_reset_and_a_power_cycl
    Create VdMot Machine    terminal=${TRUE}
    Execute Command    sysbus.valves SetSpeed 2.0
    Run Until Us    100000
    Set Next Reset Flags    ${CSR_SOFTWARE}
    Arm Reset Mark
    ${r0}=    Replay Golden Boot    ${slug}    0    0
    ${t1}=    Wait For Reset
    ${w0}=    Expect Golden Terminal    ${slug}    0    0
    ${e0}=    Expect Golden EEPROM    ${slug}    0
    ${r1}=    Replay Golden Boot    ${slug}    1    ${t1}
    # the C++ case powers the board off 5 s after its last request
    Run Until Us    ${t1 + 8761000}
    ${w1}=    Expect Golden Terminal    ${slug}    1    ${t1}
    ${e1}=    Expect Golden EEPROM    ${slug}    1
    ${t2}=    Power Cycle
    ${r2}=    Replay Golden Boot    ${slug}    2    ${t2}
    Run Until Us    ${t2 + 3700000}
    ${w2}=    Expect Golden Terminal    ${slug}    2    ${t2}
    ${e2}=    Expect Golden EEPROM    ${slug}    2
    ${at1}=    Evaluate    ${t1} / 1000000.0
    Evidence    A3 boot 0: ${r0} replies, reset command -> SYSRESETREQ at ${at1} s; boot 1 (software): ${r1} replies; boot 2 (power-on): ${r2} replies, all equal to the golden; terminal lines equal (chunk starts within ${w0}/${w1}/${w2} ms); EEPROM rows equal after each boot (${e0}/${e1}/${e2} rows)

A4 A Warm Reset Continues The C++ 2.1.7 State
    [Tags]    A4
    [Documentation]    K1-8/W2: the no-init region and the EEPROM as C++ 2.1.7 left them at its
    ...                reset command (expired lease, lease 5 min, valves 0-7 present, 8-11 open),
    ...                then a software reset into the Rust image: the warm start keeps the lease
    ...                state and the valve states, no presence test (no enable in 10 s).
    ${slug}=    Set Variable    motion__k1_8_w2_a_warm_reset_keeps_the_expired_lease_and_the_valve_state
    Create VdMot Machine    terminal=${TRUE}
    Execute Command    sysbus.valves SetSpeed 2.0
    FOR    ${v}    IN RANGE    8    12
        Execute Command    sysbus.valves SetConnected ${v} false
    END
    Load Golden State    ${slug}    0
    Set Next Reset Flags    ${CSR_SOFTWARE}
    Run Until Us    3600000
    ${worst}=    Expect Golden Terminal    ${slug}    1    0
    ${gstax}=    Exchange    gstax
    ${lease}=    Reply Field    ${gstax}    7
    ${timeout}=    Reply Field    ${gstax}    10
    Should Be Equal As Integers    ${lease}    2    lease state (expired)
    Should Be Equal As Integers    ${timeout}    5    lease timeout (min)
    ${gvlst}=    Exchange    gvlst
    Should Be Equal    ${gvlst}    gvlst 12 8,8,8,8,8,8,8,8,6,6,6,6${SPACE}
    Run Until Us    13600000
    @{enables}=    Valve Enables
    FOR    ${v}    IN RANGE    12
        Should Be Equal As Integers    ${enables}[${v}]    0    valve ${v} enabled
    END
    ${gstax}=    Exchange    gstax
    ${lease}=    Reply Field    ${gstax}    7
    Should Be Equal As Integers    ${lease}    2    lease state after 10 s
    ${rows}=    Expect Golden EEPROM    ${slug}    1
    Evidence    A4 C++ 2.1.7 no-init + EEPROM, software reset: banner and cfgFlags 0 as the golden (within ${worst} ms); gstax lease 2, timeout 5; "${gvlst}"; no valve enabled in 10 s; EEPROM unchanged (${rows} rows)

A5 Three Stalled Valve Loops End In The Safe Mode
    [Tags]    A5
    [Documentation]    S9-2 with real watchdog resets (valves 4-11 open, 2.0 pulses/ms): 2 s into
    ...                the application TIM2 stops, the main loop runs on but no longer feeds the
    ...                IWDG, which resets the chip 8 s after its last reload; three times. Every
    ...                start prints the golden's lines up to its EEPROM load ("reset by watchdog",
    ...                at the fourth "safe mode"). The fourth start answers gstax as the golden but
    ...                for uptime and lease remaining (time) and eepState and cfgFlags: the C++
    ...                case resets 2 s into the application, before the first-start EEPROM write,
    ...                the real IWDG 8 s later, after it. No valve moves in the safe mode; ssafe 1
    ...                is refused, ssafe 0 ends it and the presence test finds valves 0-3. Renode
    ...                does not set RCC_CSR.IWDGRSTF: the CSR hook gives the flags of a watchdog
    ...                reset.
    ${slug}=    Set Variable    proto3__s9_2_after_three_watchdog_resets_no_valve_moves_until_ssafe_0
    Create VdMot Machine    terminal=${TRUE}
    Execute Command    sysbus.valves SetSpeed 2.0
    FOR    ${v}    IN RANGE    4    12
        Execute Command    sysbus.valves SetConnected ${v} false
    END
    Run Until Us    100000
    Set Next Reset Flags    ${CSR_WATCHDOG}
    ${t0}=    Set Variable    ${0}
    @{after}=    Create List
    FOR    ${boot}    IN RANGE    3
        Run Until Us    ${t0 + 5511000}
        ${stall}=    Stall The Valve Loop
        Arm Reset Mark
        Wait For Log Entry    Watchdog reset triggered    timeout=9    pauseEmulation=true
        ${t}=    Now Us
        ${s}=    Evaluate    round((${t} - ${stall}) / 1000000.0, 3)
        Should Be True    7.0 < ${s} < 8.6    IWDG reset ${s} s after the stall
        Append To List    ${after}    ${s}
        Expect Golden Terminal Head    ${slug}    ${boot}
        ${t0}=    Wait For Reset
    END
    ${golden}=    Golden Boot Events    ${slug}    3    ${VERSION}_${TAG}
    Run Until Us    ${t0 + 3511000}
    @{head}=    Expect Golden Terminal Head    ${slug}    3
    ${gstax}=    Exchange    gstax
    ${want}=    Fields Except    ${golden}[1][2]    1,6,8,18
    ${got}=    Fields Except    ${gstax}    1,6,8,18
    Lists Should Be Equal    ${got}    ${want}
    @{before}=    Valve Enables
    Run Until Us    ${t0 + 13511000}
    @{now}=    Valve Enables
    Lists Should Be Equal    ${now}    ${before}    a valve moved in the safe mode
    ${r}=    Exchange    ssafe 1
    Should Be Equal    ${r}    ssafe err
    ${r}=    Exchange    ssafe 0
    Should Be Equal    ${r}    ssafe ok
    ${gstax2}=    Exchange    gstax
    ${safe}=    Reply Field    ${gstax2}    12
    Should Be Equal As Integers    ${safe}    0
    Run Until Us    ${t0 + 36511000}
    ${gvlst}=    Exchange    gvlst
    @{status}=    Valve Statuses    ${gvlst}
    @{now}=    Valve Enables
    FOR    ${v}    IN RANGE    4
        Should Be Equal As Integers    ${status}[${v}]    8    valve ${v} present
        Should Be True    ${now}[${v}] > ${before}[${v}]    valve ${v} not tested
    END
    Evidence    A5 IWDG resets ${after} s after TIM2 stopped; 4th start: ${head}; "${gstax}" (golden "${golden}[1][2]", equal but uptime, eepState, lease remaining, cfgFlags); no valve moved in 10 s; ssafe 1 err, ssafe 0 ok; then "${gvlst}"

A6 The Fault Record Of The Start Before Is On The Terminal
    [Tags]    A6
    [Documentation]    UDF in the running application, the IWDG reset (watchdog flags by the CSR
    ...                hook): the next start prints the record right after its banner, count 1,
    ...                then "reset by watchdog". The chip escalates the UsageFault (disabled in
    ...                SHCSR) to HardFault, whose handler records the exception frame; Renode takes
    ...                the UsageFault vector, whose handler has no frame: pc, lr, xpsr 0 there.
    Create VdMot Machine    terminal=${TRUE}
    Run Until Us    3600000
    Set Next Reset Flags    ${CSR_WATCHDOG}
    Arm Reset Mark
    Execute Command    sysbus WriteWord ${UDF_AT} 0xDE00
    Execute Command    cpu PC ${UDF_AT}
    Wait For Log Entry    Watchdog reset triggered    timeout=10    pauseEmulation=true
    ${t1}=    Wait For Reset
    Run Until Us    ${t1 + 3600000}
    # the banner of the first start, then the one of the start after the fault
    Wait For Line On Uart    VdMot Controller    timeout=0.01    testerId=${DBG}
    ${banner}=    Wait For Line On Uart    VdMot Controller    timeout=0.01    testerId=${DBG}
    @{next}=    Terminal Lines    2
    Should Match Regexp    ${next}[0][Line]
    ...    ^last fault: (HardFault pc 0x20000100 lr 0x[0-9A-F]+ xpsr 0x[0-9A-F]+|UsageFault pc 0x0 lr 0x0 xpsr 0x0) cfsr 0x10000 hfsr 0x[0-9A-F]+ bfar 0x[0-9A-F]+ count 1$
    Should Be Equal    ${next}[1][Line]    reset by watchdog
    Evidence    A6 after the UDF and the IWDG reset: "${banner}[Line]", "${next}[0][Line]", "${next}[1][Line]"
