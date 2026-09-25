# Fork-per-case doctest runner of the native glue suites (testkit.h) and its self-test.
#
# The native test projects add this directory (add_subdirectory), so their ctest runs
# testkit_selftest; glue executables link the runner with target_link_libraries(<exe> PRIVATE
# vdm_testkit) instead of a doctest main. The parent project provides THIRD_PARTY (doctest.h),
# WARNINGS, SANITIZERS and VDM_FAST_LINK. Standalone:
#   cmake -S tools/native/testkit -B build/testkit -G Ninja && cmake --build build/testkit
#   ctest --test-dir build/testkit --output-on-failure
cmake_minimum_required(VERSION 3.13)

if(CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)
  project(vdm_testkit CXX)
  set(CMAKE_CXX_STANDARD 17)
  set(CMAKE_CXX_STANDARD_REQUIRED ON)
  set(CMAKE_CXX_EXTENSIONS OFF)
  set(THIRD_PARTY ${CMAKE_CURRENT_SOURCE_DIR}/../../../software_stm32/test/native/third_party)
  set(SANITIZERS -fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  set(WARNINGS -Wall -Wextra -Werror)
  enable_testing()
endif()

add_library(vdm_testkit OBJECT ${CMAKE_CURRENT_SOURCE_DIR}/fork_main.cpp)
target_include_directories(vdm_testkit PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_include_directories(vdm_testkit SYSTEM PUBLIC ${THIRD_PARTY})
target_compile_options(vdm_testkit PRIVATE ${WARNINGS} ${SANITIZERS})

add_executable(testkit_selftest ${CMAKE_CURRENT_SOURCE_DIR}/selftest.cpp)
target_compile_options(testkit_selftest PRIVATE ${WARNINGS} ${SANITIZERS})
target_link_libraries(testkit_selftest PRIVATE vdm_testkit)
target_link_options(testkit_selftest PRIVATE ${SANITIZERS} ${VDM_FAST_LINK})
add_test(NAME testkit_selftest
         COMMAND bash ${CMAKE_CURRENT_SOURCE_DIR}/selftest.sh $<TARGET_FILE:testkit_selftest>)
