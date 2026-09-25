# STM32 glue suite: every src/<stem>.cpp of the firmware compiled unchanged against the fake board (test/native/fakes/include comes first in the include path, the
# quoted includes next to a glue file are the real firmware headers).
#
#   glue_<stem>    src/<stem>.cpp + test_<stem>*.cpp + link-seam stubs of the glue modules it calls
#                  (stubs/stub_<module>.cpp); what a mutant of src/<stem>.cpp is tested with first
#   glue_system    every glue file + the valve sim + test_system_*.cpp; the second mutation stage
#   glue_fakes     tests of the fakes, the runner hooks and the valve sim
#   glue_motor_c1  glue_motor with the C1 wiring (ctest only)
#
# Every executable runs with the fork-per-case runner (vdm_testkit) and the runner hooks of this
# directory. Test files are globbed: a new test_<stem>_<topic>.cpp or test_system_<topic>.cpp needs
# no edit here. WARNINGS, SANITIZERS, VDM_FAST_LINK, THIRD_PARTY come from ../CMakeLists.txt.

set(STM ${CMAKE_CURRENT_SOURCE_DIR}/../../..)
set(FAKES ${CMAKE_CURRENT_SOURCE_DIR}/../fakes)
set(GLUE ${CMAKE_CURRENT_SOURCE_DIR})

# The -D flags of [env:STM32_release_C2] / [env:STM32_release_C1] in platformio.ini (the framework's
# own defines are not needed by the fakes). tools/native/check_release_flags.py fails when they
# differ; docker.sh test stm32 runs it before cmake.
set(VDM_GLUE_DEFINES
    SERIAL_RX_BUFFER_SIZE=1024 SERIAL_TX_BUFFER_SIZE=1024
    FIRMWARE_VERSION="2.0.0-revamped" FIRMWARE_BUILD="1" HARDWARE_VERSION="C2" HARDWARE_REVISION_C2
    commDebug appDebug)
set(VDM_GLUE_DEFINES_C1
    SERIAL_RX_BUFFER_SIZE=1024 SERIAL_TX_BUFFER_SIZE=1024
    FIRMWARE_VERSION="2.0.0-revamped" FIRMWARE_BUILD="1" HARDWARE_VERSION="C1" HARDWARE_REVISION_C1
    commDebug appDebug)

# Fake board, stub registry and runner hooks: compiled once, linked into every glue executable. The
# serial buffer sizes change the HardwareSerial layout, so they are the same in every translation unit.
add_library(vdm_glue_support OBJECT
  ${FAKES}/src/fake_board.cpp ${FAKES}/src/Print.cpp ${FAKES}/src/WString.cpp
  ${GLUE}/stubs/stub_log.cpp ${GLUE}/runner_hooks.cpp)
target_include_directories(vdm_glue_support PUBLIC ${FAKES}/include ${STM}/include ${GLUE} ${GLUE}/stubs ${GLUE}/sim)
target_compile_definitions(vdm_glue_support PRIVATE SERIAL_RX_BUFFER_SIZE=1024 SERIAL_TX_BUFFER_SIZE=1024)
target_compile_options(vdm_glue_support PRIVATE ${WARNINGS} ${SANITIZERS})
target_link_libraries(vdm_glue_support PUBLIC vdm_core vdm_testkit)

# vdm_glue_executable(<name> <defines list variable> <sources...>)
function(vdm_glue_executable name defines)
  add_executable(${name} ${ARGN})
  target_include_directories(${name} SYSTEM PRIVATE ${THIRD_PARTY})
  target_compile_definitions(${name} PRIVATE ${${defines}})
  target_compile_options(${name} PRIVATE ${WARNINGS} ${SANITIZERS})
  target_link_libraries(${name} PRIVATE vdm_glue_support vdm_testkit vdm_core)
  target_link_options(${name} PRIVATE ${SANITIZERS} ${VDM_FAST_LINK})
endfunction()

# vdm_glue_test(<stem> STUBS <module>... [SIM] [C1]): glue_<stem> (glue_<stem>_c1 with C1)
function(vdm_glue_test stem)
  cmake_parse_arguments(G "SIM;C1" "" "STUBS" ${ARGN})
  file(GLOB tests CONFIGURE_DEPENDS ${GLUE}/test_${stem}*.cpp)
  set(srcs ${STM}/src/${stem}.cpp ${tests})
  foreach(s ${G_STUBS})
    list(APPEND srcs ${GLUE}/stubs/stub_${s}.cpp)
  endforeach()
  if(G_SIM)
    list(APPEND srcs ${GLUE}/sim/valve_sim.cpp)
  endif()
  if(G_C1)
    vdm_glue_executable(glue_${stem}_c1 VDM_GLUE_DEFINES_C1 ${srcs})
    add_test(NAME glue_${stem}_c1 COMMAND glue_${stem}_c1)
  else()
    vdm_glue_executable(glue_${stem} VDM_GLUE_DEFINES ${srcs})
    add_test(NAME glue_${stem} COMMAND glue_${stem})
  endif()
endfunction()

vdm_glue_test(app           STUBS motor eeprom owDevices terminal)
vdm_glue_test(communication STUBS app motor eeprom owDevices sysstat terminal)
vdm_glue_test(eeprom        STUBS app i2c_bus terminal)
vdm_glue_test(i2c_bus       STUBS terminal)
vdm_glue_test(main          STUBS app communication eeprom i2c_bus motor otasupport owDevices sysstat terminal)
vdm_glue_test(motor         STUBS app eeprom owDevices terminal SIM)
vdm_glue_test(otasupport    STUBS boot_jump terminal)
vdm_glue_test(owDevices     STUBS app ds2438 terminal)
vdm_glue_test(sysstat       STUBS terminal)
vdm_glue_test(terminal      STUBS app communication eeprom motor owDevices)
# the mutation build takes the C2 wiring only (the glue sources are the same, hardware.h differs)
if(NOT VDM_BUILD_TYPE STREQUAL "MUTATION")
  vdm_glue_test(motor       STUBS app eeprom owDevices terminal SIM C1)
endif()

vdm_glue_executable(glue_fakes VDM_GLUE_DEFINES
  ${GLUE}/test_fakes.cpp ${GLUE}/sim/valve_sim.cpp ${GLUE}/stubs/stub_motor.cpp ${GLUE}/stubs/stub_terminal.cpp)
add_test(NAME glue_fakes COMMAND glue_fakes)

file(GLOB system_tests CONFIGURE_DEPENDS ${GLUE}/test_system_*.cpp)
vdm_glue_executable(glue_system VDM_GLUE_DEFINES
  ${STM}/src/app.cpp ${STM}/src/communication.cpp ${STM}/src/eeprom.cpp ${STM}/src/i2c_bus.cpp
  ${STM}/src/main.cpp ${STM}/src/motor.cpp ${STM}/src/otasupport.cpp ${STM}/src/owDevices.cpp
  ${STM}/src/sysstat.cpp ${STM}/src/terminal.cpp
  ${GLUE}/stubs/stub_boot_jump.cpp ${GLUE}/stubs/stub_ds2438.cpp ${GLUE}/sim/valve_sim.cpp ${system_tests})
add_test(NAME glue_system COMMAND glue_system)
