*** Settings ***
Documentation     Scenario E11 (docs/rust/GLUE-DESIGN-STM.md §5.7, D9, D11): the Rust ESP flasher
...               (vdm_esp_core::stm_flasher in the host program vdm-e11, VdmEsp.cs) replaces the
...               image through the boot window of the running one, the ROM bootloader stand-in
...               (VdmRom.cs) erases and writes the emulated flash, and after each flash the new
...               image answers gvers. With ${CPP_IMAGES} (a directory with the C++ 2.1.7 release
...               images, each checked against its SHA-256 in tools/rust/stm/cpp217.sha256):
...               C++ -> Rust -> C++; without: Rust -> Rust with another version string -> Rust.
...               tools/rust/renode.sh --e11 [--cpp <dir>] runs it.
Resource          vdm.resource
Library           e11.py

*** Variables ***
${CPP_IMAGES}     ${EMPTY}
${CPP_VERSION}    2.1.7-revamped
${HARNESS}        ${REPO}/software_stm32_rust/renode/e11/vdm-e11

*** Keywords ***
Create E11 Machine
    [Documentation]    SysTick and the CPU at the core clock of the application (84 / 96 MHz): the
    ...                C++ image counts its milliseconds on SysTick from there and its 1 ms timer
    ...                interrupt converts twice with the whole HAL ADC set-up, which needs the real
    ...                speed (at 4 MIPS it takes more than 1 ms, TIM2 at the same priority never
    ...                runs and the IWDG resets the chip). The Rust boot window, counted for the
    ...                25 MHz boot clock, is shorter then: E1-E10 check its timing, E11 the flash.
    [Arguments]    ${bin}
    IF    '${CHIP}' == 'f401'
        ${rom}=    Set Variable    pid: 0x423; flashKiB: 256
        ${mhz}=    Set Variable    84
    ELSE
        ${rom}=    Set Variable    pid: 0x431; flashKiB: 512
        ${mhz}=    Set Variable    96
    END
    Create VdMot Machine    bin=${bin}    systick_hz=${mhz}000000
    Execute Command    cpu PerformanceInMips ${mhz}
    Execute Command    include @${REPO}/software_stm32_rust/renode/VdmEsp.cs
    Execute Command    include @${REPO}/software_stm32_rust/renode/VdmRom.cs
    Execute Command    machine LoadPlatformDescriptionFromString "esp: UART.VdmEsp @ sysbus 0x50070000"
    Execute Command    machine LoadPlatformDescriptionFromString "rom: Miscellaneous.VdmRom @ sysbus 0x50070100 { ${rom} }"
    Execute Command    emulation CreateUARTHub "esphub"
    Execute Command    connector Connect sysbus.usart1 esphub
    Execute Command    connector Connect sysbus.esp esphub

Run The Flasher
    [Documentation]    Starts vdm-e11 with the jobs and runs until it reports the end (at most
    ...                ${limit} s of virtual time); returns the result ("ok" / "fail").
    [Arguments]    ${jobs}    ${limit}=240
    Execute Command    sysbus.esp Start "${HARNESS}" "${TAG} ${jobs}"
    ${steps}=    Evaluate    int(${limit}) // 5
    FOR    ${i}    IN RANGE    ${steps}
        Execute Command    emulation RunFor "5"
        ${r}=    Execute Command    sysbus.esp Result
        ${r}=    Strip String    ${r}
        IF    '${r}' != ''    RETURN    ${r}
    END
    Fail    the flasher did not end within ${limit} s

*** Test Cases ***
E11 The ESP Flasher Replaces The Image Through The Boot Window
    [Tags]    E11
    ${rust}=    Set Variable    ${IMAGES}/${IMAGE}.bin
    IF    '${CPP_IMAGES}' == ''
        ${other}=    Evaluate    "9.9.9" + "${VERSION}"[5:]
        Create E11 Machine    ${rust}
        ${jobs}=    Set Variable    ${rust}@${other} ${rust}
        ${cycle}=    Set Variable    Rust ${VERSION} -> Rust ${other} (ID block patched) -> Rust ${VERSION}
    ELSE
        ${cpp}=    Find One Image    ${CPP_IMAGES}    ${IMAGE}
        Create E11 Machine    ${cpp}
        ${jobs}=    Set Variable    ${rust} ${cpp}
        ${cycle}=    Set Variable    C++ ${CPP_VERSION} -> Rust ${VERSION} -> C++ ${CPP_VERSION}
    END
    ${result}=    Run The Flasher    ${jobs}
    ${results}=    Execute Command    sysbus.esp Results
    ${results}=    Evaluate    " | ".join(l for l in $results.splitlines() if l.strip())
    ${ops}=    Execute Command    sysbus.rom Operations
    ${report}=    Execute Command    sysbus.esp Report
    Log    ${report}
    Log    ${ops}
    Should Be Equal    ${result}    ok    ${results}
    ${sessions}=    Check D9 Order    ${ops}
    Should Be Equal As Integers    ${sessions}    2
    Evidence    E11 ${cycle}: ${results}; ROM log ${ops.strip()}
