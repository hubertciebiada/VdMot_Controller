# Goldens of the C++ glue_system scenarios for the Rust system tests (docs/rust/GLUE-DESIGN-STM.md
# §7.4). The executable is glue_system of software_stm32/test/native/glue (the same firmware
# sources, fakes, stubs, valve sim and test_system_*.cpp, unchanged) plus recorder.cpp, which
# writes per test case the UART transcripts (USART1 both ways, USART6 out), the 24LC64 image and
# the no-init cells of every boot. The C++ tree is not modified: the recorder hooks in through
# the linker (--wrap) and a doctest listener.
#
#   tools/native/docker.sh run "bash tools/rust/stm/golden/run.sh"
#
# No sanitizers (the no-init cells are read at their symbols, without ASan redzones around
# them), -O1; the defines are those of VDM_GLUE_DEFINES in software_stm32/test/native/glue.
cmake_minimum_required(VERSION 3.13)
project(vdm_stm_golden CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

get_filename_component(REPO ${CMAKE_CURRENT_SOURCE_DIR}/../../../.. ABSOLUTE)
set(STM ${REPO}/software_stm32)
set(NATIVE ${STM}/test/native)
set(GLUE ${NATIVE}/glue)
set(FAKES ${NATIVE}/fakes)
set(TESTKIT ${REPO}/tools/native/testkit)

file(GLOB CORE_SOURCES CONFIGURE_DEPENDS ${STM}/lib/core/src/*.cpp)
add_library(golden_core STATIC ${CORE_SOURCES})
target_include_directories(golden_core PUBLIC ${STM}/lib/core/include)
target_compile_options(golden_core PRIVATE -O1 -fno-exceptions -fno-rtti)

file(GLOB SYSTEM_TESTS CONFIGURE_DEPENDS ${GLUE}/test_system_*.cpp)
add_executable(glue_system_golden
  ${STM}/src/app.cpp ${STM}/src/communication.cpp ${STM}/src/eeprom.cpp ${STM}/src/i2c_bus.cpp
  ${STM}/src/main.cpp ${STM}/src/motor.cpp ${STM}/src/otasupport.cpp ${STM}/src/owDevices.cpp
  ${STM}/src/sysstat.cpp ${STM}/src/terminal.cpp
  ${GLUE}/stubs/stub_boot_jump.cpp ${GLUE}/stubs/stub_ds2438.cpp ${GLUE}/stubs/stub_log.cpp
  ${GLUE}/sim/valve_sim.cpp ${GLUE}/runner_hooks.cpp ${SYSTEM_TESTS}
  ${FAKES}/src/fake_board.cpp ${FAKES}/src/Print.cpp ${FAKES}/src/WString.cpp
  ${TESTKIT}/fork_main.cpp
  ${CMAKE_CURRENT_SOURCE_DIR}/recorder.cpp)
# the fakes first: the quoted includes next to a glue file are the real firmware headers
target_include_directories(glue_system_golden PRIVATE
  ${FAKES}/include ${STM}/include ${GLUE} ${GLUE}/stubs ${GLUE}/sim ${TESTKIT})
target_include_directories(glue_system_golden SYSTEM PRIVATE ${NATIVE}/third_party)
target_compile_definitions(glue_system_golden PRIVATE
  SERIAL_RX_BUFFER_SIZE=1024 SERIAL_TX_BUFFER_SIZE=1024
  FIRMWARE_VERSION="2.1.7-revamped" FIRMWARE_BUILD="1" HARDWARE_VERSION="C2" HARDWARE_REVISION_C2
  commDebug appDebug)
target_compile_options(glue_system_golden PRIVATE -O1 -Wall -Wextra)
target_link_libraries(glue_system_golden PRIVATE golden_core)
# the recorder's seams: every loop_system() and setup_system() of the tests (UART bytes out),
# fake::inject() (bytes in) and testkit::reboot() (end of a boot)
target_link_options(glue_system_golden PRIVATE
  -Wl,--wrap=_Z11loop_systemv
  -Wl,--wrap=_Z12setup_systemv
  -Wl,--wrap=_ZN4fake6injectER14HardwareSerialRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE
  -Wl,--wrap=_ZN7testkit6rebootENS_5ResetE)
