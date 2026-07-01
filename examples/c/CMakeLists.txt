# Build and run the C and C++ examples against the wickra-exchange C ABI.
#
#   cargo build --release -p wickra-exchange-c
#   cmake -S examples/c -B examples/c/build
#   cmake --build examples/c/build --config Release
#   ctest --test-dir examples/c/build -C Release --output-on-failure
#
# WICKRA_EXCHANGE_LIB_DIR defaults to the workspace target/release directory and
# can be overridden for out-of-tree builds.

cmake_minimum_required(VERSION 3.15)
project(wickra_exchange_examples C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_CXX_STANDARD 14)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(WICKRA_EXCHANGE_LIB_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../target/release"
    CACHE PATH "Directory containing the built wickra-exchange C ABI library")

set(WICKRA_EXCHANGE_INCLUDE "${CMAKE_CURRENT_SOURCE_DIR}/../../bindings/c/include")

find_library(WICKRA_EXCHANGE_LIB
    NAMES wickra_exchange libwickra_exchange
    HINTS "${WICKRA_EXCHANGE_LIB_DIR}"
    REQUIRED)

add_executable(replay replay.c)
target_include_directories(replay PRIVATE "${WICKRA_EXCHANGE_INCLUDE}")
target_link_libraries(replay PRIVATE "${WICKRA_EXCHANGE_LIB}")
if(NOT WIN32)
    target_link_libraries(replay PRIVATE m)
endif()

add_executable(paper paper.cpp)
target_include_directories(paper PRIVATE "${WICKRA_EXCHANGE_INCLUDE}")
target_link_libraries(paper PRIVATE "${WICKRA_EXCHANGE_LIB}")

enable_testing()
add_test(NAME replay COMMAND replay)
add_test(NAME paper COMMAND paper)
