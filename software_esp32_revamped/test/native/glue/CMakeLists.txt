# ESP32 glue suite: every src/<stem>.cpp of the firmware compiled unchanged against the fake
# Arduino-ESP32 / ESP-IDF / library layer (hal/include comes first in the include path; the
# quoted includes next to a glue file are the real firmware headers).
#
#   glue_<stem>     src/<stem>.cpp + test_<stem>*.cpp + the sibling fakes of every other glue
#                   module (siblings/fake_<module>.cpp); what a mutant of src/<stem>.cpp is tested
#                   with (esp32-glue.json)
#   glue_selftest   tests of the fakes, the runner hooks and the fake STM (selftest.sh also runs
#                   the cases that must fail: invariants, fail-fast, ASan)
#
# Every executable runs with the fork-per-case runner (vdm_testkit) and the runner hooks of this
# directory. Test files are globbed: a new test_<stem>_<topic>.cpp needs no edit here.
# The glue objects are built like on the target: -fexceptions -fno-rtti (Arduino-ESP32 2.0.7
# platform.txt); UBSan's vptr check is off because RTTI and no-RTTI objects share classes.
# WARNINGS, SANITIZERS, VDM_FAST_LINK, THIRD_PARTY come from ../CMakeLists.txt.

set(ESP ${CMAKE_CURRENT_SOURCE_DIR}/../../..)
set(GLUE ${CMAKE_CURRENT_SOURCE_DIR})
set(GLUE_MODULES app logger main mqtt_client net ota stm_link stm_service storage web_server)
# glue modules with a header and therefore a sibling fake (main.cpp has neither)
set(GLUE_SIBLINGS app logger mqtt_client net ota stm_link stm_service storage web_server)

# The vendored ArduinoJson must be the version platformio.ini builds the firmware with.
file(READ ${ESP}/platformio.ini VDM_PIO_INI)
string(REGEX MATCH "bblanchon/ArduinoJson@([0-9.]+)" _ "${VDM_PIO_INI}")
set(VDM_ARDUINOJSON_PIO "${CMAKE_MATCH_1}")
set(VDM_ARDUINOJSON_HEADER ${THIRD_PARTY}/ArduinoJson-v${VDM_ARDUINOJSON_PIO}.h)
if(NOT EXISTS ${VDM_ARDUINOJSON_HEADER})
  message(FATAL_ERROR "platformio.ini pins ArduinoJson ${VDM_ARDUINOJSON_PIO}, but "
                      "${VDM_ARDUINOJSON_HEADER} is not vendored (test/native/third_party)")
endif()
file(STRINGS ${VDM_ARDUINOJSON_HEADER} VDM_ARDUINOJSON_DEFINE
     REGEX "^#define ARDUINOJSON_VERSION \"")
string(REGEX MATCH "\"([0-9.]+)\"" _ "${VDM_ARDUINOJSON_DEFINE}")
if(NOT CMAKE_MATCH_1 STREQUAL VDM_ARDUINOJSON_PIO)
  message(FATAL_ERROR "vendored ArduinoJson is ${CMAKE_MATCH_1}, platformio.ini pins "
                      "${VDM_ARDUINOJSON_PIO}")
endif()
file(READ ${GLUE}/hal/include/ArduinoJson.h VDM_ARDUINOJSON_WRAPPER)
if(NOT VDM_ARDUINOJSON_WRAPPER MATCHES "ArduinoJson-v${VDM_ARDUINOJSON_PIO}\\.h")
  message(FATAL_ERROR "hal/include/ArduinoJson.h does not include "
                      "ArduinoJson-v${VDM_ARDUINOJSON_PIO}.h")
endif()

# ArduinoJson switches of platformio.ini ([env] build_flags), for every translation unit of the
# suite (a test that parses a response must see the same library configuration).
set(VDM_GLUE_DEFINES
    ARDUINOJSON_USE_LONG_LONG=1 ARDUINOJSON_ENABLE_STD_STRING=0 ARDUINOJSON_ENABLE_STD_STREAM=0)

set(VDM_GLUE_OPTIONS ${WARNINGS} ${SANITIZERS})
if(NOT VDM_BUILD_TYPE STREQUAL "MUTATION")
  list(APPEND VDM_GLUE_OPTIONS -fno-sanitize=vptr)
endif()
set(VDM_GLUE_INCLUDES ${GLUE}/hal/include ${ESP}/src ${GLUE} ${GLUE}/siblings ${GLUE}/support
    ${CMAKE_CURRENT_SOURCE_DIR}/../support)

# The dashboard assets web_server.cpp includes (src/generated/web_assets.h, git-ignored): the
# generator of the pio pre-script, run only when web/ or the script changed, so a mutant build
# never starts Python.
find_program(VDM_PYTHON NAMES python3 python REQUIRED)
file(GLOB_RECURSE VDM_WEB_FILES CONFIGURE_DEPENDS ${ESP}/web/*)
add_custom_command(OUTPUT ${CMAKE_CURRENT_BINARY_DIR}/web_assets.stamp
  COMMAND ${VDM_PYTHON} ${ESP}/tools/gen_web_assets.py ${ESP}
  COMMAND ${CMAKE_COMMAND} -E touch ${CMAKE_CURRENT_BINARY_DIR}/web_assets.stamp
  DEPENDS ${VDM_WEB_FILES} ${ESP}/tools/gen_web_assets.py
  BYPRODUCTS ${ESP}/src/generated/web_assets.h
  VERBATIM)
add_custom_target(vdm_web_assets DEPENDS ${CMAKE_CURRENT_BINARY_DIR}/web_assets.stamp)

# vdm_glue_object(<name> <sources...>): an OBJECT library of the suite (static initializers of
# the hooks must always be linked, hence no static archive).
function(vdm_glue_object name)
  add_library(${name} OBJECT ${ARGN})
  target_include_directories(${name} BEFORE PRIVATE ${VDM_GLUE_INCLUDES})
  target_include_directories(${name} SYSTEM PRIVATE ${THIRD_PARTY} ${VDM_TESTKIT_DIR})
  target_include_directories(${name} PRIVATE ${CORE_DIR}/include)
  target_compile_definitions(${name} PRIVATE ${VDM_GLUE_DEFINES})
  target_compile_options(${name} PRIVATE ${VDM_GLUE_OPTIONS})
endfunction()

# Fake framework and libraries, runner hooks, sibling state, fake STM: compiled once.
file(GLOB VDM_HAL_SOURCES CONFIGURE_DEPENDS ${GLUE}/hal/src/*.cpp)
vdm_glue_object(vdm_glue_support ${VDM_HAL_SOURCES} ${GLUE}/runner_hooks.cpp
                ${GLUE}/siblings/siblings.cpp ${GLUE}/support/fake_stm.cpp)
foreach(m ${GLUE_SIBLINGS})
  vdm_glue_object(vdm_glue_fake_${m} ${GLUE}/siblings/fake_${m}.cpp)
endforeach()

# vdm_glue_executable(<name> <sources...>)
function(vdm_glue_executable name)
  add_executable(${name} ${ARGN} $<TARGET_OBJECTS:vdm_glue_support>)
  target_include_directories(${name} BEFORE PRIVATE ${VDM_GLUE_INCLUDES})
  target_include_directories(${name} SYSTEM PRIVATE ${THIRD_PARTY})
  target_compile_definitions(${name} PRIVATE ${VDM_GLUE_DEFINES})
  target_compile_options(${name} PRIVATE ${VDM_GLUE_OPTIONS})
  target_link_libraries(${name} PRIVATE vdm_testkit vdm_core)
  target_link_options(${name} PRIVATE ${SANITIZERS} ${VDM_FAST_LINK})
endfunction()

foreach(stem ${GLUE_MODULES})
  # the glue file itself, compiled like on the target
  add_library(vdm_glue_${stem}_obj OBJECT ${ESP}/src/${stem}.cpp)
  target_include_directories(vdm_glue_${stem}_obj BEFORE PRIVATE ${GLUE}/hal/include)
  target_include_directories(vdm_glue_${stem}_obj SYSTEM PRIVATE ${THIRD_PARTY})
  target_include_directories(vdm_glue_${stem}_obj PRIVATE ${CORE_DIR}/include)
  target_compile_definitions(vdm_glue_${stem}_obj PRIVATE ${VDM_GLUE_DEFINES})
  target_compile_options(vdm_glue_${stem}_obj PRIVATE ${VDM_GLUE_OPTIONS} -fexceptions -fno-rtti)
  file(GLOB tests CONFIGURE_DEPENDS ${GLUE}/test_${stem}*.cpp)
  set(fakes)
  foreach(m ${GLUE_SIBLINGS})
    if(NOT m STREQUAL stem)
      list(APPEND fakes $<TARGET_OBJECTS:vdm_glue_fake_${m}>)
    endif()
  endforeach()
  vdm_glue_executable(glue_${stem} $<TARGET_OBJECTS:vdm_glue_${stem}_obj> ${tests} ${fakes})
  add_test(NAME glue_${stem} COMMAND glue_${stem})
endforeach()
# web_server.cpp and its tests include the generated assets
add_dependencies(vdm_glue_web_server_obj vdm_web_assets)
add_dependencies(glue_web_server vdm_web_assets)

set(fakes)
foreach(m ${GLUE_SIBLINGS})
  list(APPEND fakes $<TARGET_OBJECTS:vdm_glue_fake_${m}>)
endforeach()
vdm_glue_executable(glue_selftest ${GLUE}/selftest.cpp ${fakes})
add_test(NAME glue_selftest COMMAND bash ${GLUE}/selftest.sh $<TARGET_FILE:glue_selftest>)
