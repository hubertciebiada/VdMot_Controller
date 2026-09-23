# Native (host) unit tests for software_esp32_revamped/lib/core.
#   cmake -S test/native -B build/native && cmake --build build/native -j 2
#   ctest --test-dir build/native --output-on-failure
# Sources are globbed (lib/core/src/*.cpp, test/native/test_*.cpp), so adding
# a module or a test file never requires editing this file.
cmake_minimum_required(VERSION 3.13)
project(vdm_esp32_core_tests CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(CORE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../../lib/core)
file(GLOB CORE_SOURCES CONFIGURE_DEPENDS ${CORE_DIR}/src/*.cpp)
file(GLOB TEST_SOURCES CONFIGURE_DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/test_*.cpp)

set(SANITIZERS -fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
set(WARNINGS -Wall -Wextra -Werror)

# The core is built like on the target: no exceptions, no RTTI.
add_library(vdm_core STATIC ${CORE_SOURCES})
target_include_directories(vdm_core PUBLIC ${CORE_DIR}/include)
target_compile_options(vdm_core PRIVATE ${WARNINGS} -fno-exceptions -fno-rtti ${SANITIZERS})

add_executable(core_tests main.cpp ${TEST_SOURCES})
target_include_directories(core_tests SYSTEM PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/third_party)
target_compile_options(core_tests PRIVATE ${WARNINGS} ${SANITIZERS})
target_link_libraries(core_tests PRIVATE vdm_core)
target_link_options(core_tests PRIVATE ${SANITIZERS})

enable_testing()
add_test(NAME core_tests COMMAND core_tests)
