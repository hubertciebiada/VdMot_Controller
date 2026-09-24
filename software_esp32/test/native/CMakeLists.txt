# Host unit tests for the Arduino-free helpers of the legacy ESP firmware.
#   cmake -S test/native -B build/native && cmake --build build/native && ctest --test-dir build/native
cmake_minimum_required(VERSION 3.13)
project(vdmot_esp_legacy_native CXX)

set(CMAKE_CXX_STANDARD 11)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(ESP_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/../..)

add_executable(esp_legacy_tests
  test_main.cpp
  test_valueCheck.cpp
  ${ESP_ROOT}/src/valueCheck.cpp
)
target_include_directories(esp_legacy_tests PRIVATE ${ESP_ROOT}/include third_party)
target_compile_options(esp_legacy_tests PRIVATE -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=all)
target_link_options(esp_legacy_tests PRIVATE -fsanitize=address,undefined)

enable_testing()
add_test(NAME esp_legacy_tests COMMAND esp_legacy_tests)
