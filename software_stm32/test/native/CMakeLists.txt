# Native (host) unit tests for software_stm32/lib/core.
#   cmake -S test/native -B build/native -G Ninja && cmake --build build/native
#   ctest --test-dir build/native --output-on-failure
#
# Build types: every build type except Mutation (Debug in ctest and CI) uses ASan + UBSan and
# -Werror. Mutation (tools/mutation/*.json) is -O0 -g0, ASan only, warnings without -Werror (a
# mutant that only warns is built and counted), linked with mold when it is installed.
#
# Targets: core_tests runs every core test file (ctest). Per core module lib/core/src/<stem>.cpp,
# core_<stem> (not built by default) = test_<stem>.cpp + test_<stem>__*.cpp +
# ${CORE_EXTRA_TESTS_<stem>}; the mutation tool builds and runs it for the mutants of <stem>.cpp.
# Test files and modules are globbed, so adding one never requires editing this file.
# test/native/support/*.cpp (shared test helpers) are linked into every test executable.
# glue/ is added when it has a CMakeLists.txt; it uses WARNINGS, SANITIZERS, VDM_FAST_LINK,
# THIRD_PARTY and the vdm_testkit runner (tools/native/testkit) defined here.
cmake_minimum_required(VERSION 3.13)
project(vdm_stm32_core_tests CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
# mutate.py finds the lines a build does not compile (inactive #if) through the compile commands
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

set(CORE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../../lib/core)
set(THIRD_PARTY ${CMAKE_CURRENT_SOURCE_DIR}/third_party)
set(VDM_TESTKIT_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../../../tools/native/testkit)
file(GLOB CORE_SOURCES CONFIGURE_DEPENDS ${CORE_DIR}/src/*.cpp)
file(GLOB TEST_SOURCES CONFIGURE_DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/test_*.cpp)
file(GLOB SUPPORT_SOURCES CONFIGURE_DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/support/*.cpp)

string(TOUPPER "${CMAKE_BUILD_TYPE}" VDM_BUILD_TYPE)
if(VDM_BUILD_TYPE STREQUAL "MUTATION")
  set(CMAKE_CXX_FLAGS_MUTATION "-O0 -g0")
  set(SANITIZERS -fsanitize=address -fno-omit-frame-pointer)
  set(WARNINGS -Wall -Wextra)
  find_program(VDM_MOLD mold)
  if(VDM_MOLD)
    # per target, not global: mold cannot link with a SECTIONS linker script
    set(VDM_FAST_LINK -fuse-ld=mold)
  endif()
else()
  set(SANITIZERS -fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  set(WARNINGS -Wall -Wextra -Werror)
endif()

# The core is built like on the target: no exceptions, no RTTI.
add_library(vdm_core STATIC ${CORE_SOURCES})
target_include_directories(vdm_core PUBLIC ${CORE_DIR}/include)
target_compile_options(vdm_core PRIVATE ${WARNINGS} -fno-exceptions -fno-rtti ${SANITIZERS})

# doctest main, compiled once for all core test executables
add_library(vdm_test_main OBJECT main.cpp)
target_include_directories(vdm_test_main SYSTEM PRIVATE ${THIRD_PARTY})
target_compile_options(vdm_test_main PRIVATE ${WARNINGS} ${SANITIZERS})

set(TEST_SUPPORT)
if(SUPPORT_SOURCES)
  add_library(vdm_test_support STATIC ${SUPPORT_SOURCES})
  target_include_directories(vdm_test_support SYSTEM PRIVATE ${THIRD_PARTY})
  target_compile_options(vdm_test_support PRIVATE ${WARNINGS} ${SANITIZERS})
  target_link_libraries(vdm_test_support PUBLIC vdm_core)
  set(TEST_SUPPORT vdm_test_support)
endif()

function(vdm_core_test_executable name)
  add_executable(${name} ${ARGN} $<TARGET_OBJECTS:vdm_test_main>)
  target_include_directories(${name} SYSTEM PRIVATE ${THIRD_PARTY})
  target_compile_options(${name} PRIVATE ${WARNINGS} ${SANITIZERS})
  target_link_libraries(${name} PRIVATE ${TEST_SUPPORT} vdm_core)
  target_link_options(${name} PRIVATE ${SANITIZERS} ${VDM_FAST_LINK})
endfunction()

vdm_core_test_executable(core_tests ${TEST_SOURCES})

# Cross-module test files whose cases also count for a module's mutants (tagged fuzz or slow,
# so they run in the second mutation stage only).
set(CORE_EXTRA_TESTS_arg_parser test_fuzz.cpp)
set(CORE_EXTRA_TESTS_buf_writer test_fuzz.cpp)
set(CORE_EXTRA_TESTS_line_assembler test_fuzz.cpp)
set(CORE_EXTRA_TESTS_replies test_fuzz.cpp)
set(CORE_EXTRA_TESTS_tokenizer test_fuzz.cpp)

foreach(src ${CORE_SOURCES})
  get_filename_component(stem ${src} NAME_WE)
  file(GLOB module_tests CONFIGURE_DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/test_${stem}.cpp
       ${CMAKE_CURRENT_SOURCE_DIR}/test_${stem}__*.cpp)
  set(extra_tests)
  foreach(t ${CORE_EXTRA_TESTS_${stem}})
    list(APPEND extra_tests ${CMAKE_CURRENT_SOURCE_DIR}/${t})
  endforeach()
  vdm_core_test_executable(core_${stem} ${module_tests} ${extra_tests})
  set_target_properties(core_${stem} PROPERTIES EXCLUDE_FROM_ALL ON)
endforeach()

enable_testing()
add_test(NAME core_tests COMMAND core_tests)

# fork-per-case runner of the glue suites (vdm_testkit) and its self-test
if(EXISTS ${VDM_TESTKIT_DIR}/CMakeLists.txt)
  add_subdirectory(${VDM_TESTKIT_DIR} ${CMAKE_CURRENT_BINARY_DIR}/testkit)
endif()
if(EXISTS ${CMAKE_CURRENT_SOURCE_DIR}/glue/CMakeLists.txt)
  add_subdirectory(glue)
endif()
