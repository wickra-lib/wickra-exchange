# Build the C binding-cost benchmark against the wickra-exchange C ABI.
#
#   cargo build --release -p wickra-exchange-c
#   cmake -S benchmarks/c -B benchmarks/c/build
#   cmake --build benchmarks/c/build --config Release
#   ./benchmarks/c/build/binding_cost        # Release/binding_cost.exe on Windows
#
# WICKRA_EXCHANGE_LIB_DIR defaults to the workspace target/release directory and
# can be overridden for out-of-tree builds.

cmake_minimum_required(VERSION 3.15)
project(wickra_exchange_benchmarks C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

set(WICKRA_EXCHANGE_LIB_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../target/release"
    CACHE PATH "Directory containing the built wickra-exchange C ABI library")

set(WICKRA_EXCHANGE_INCLUDE "${CMAKE_CURRENT_SOURCE_DIR}/../../bindings/c/include")

find_library(WICKRA_EXCHANGE_LIB
    NAMES wickra_exchange libwickra_exchange
    HINTS "${WICKRA_EXCHANGE_LIB_DIR}"
    REQUIRED)

add_executable(binding_cost binding_cost.c)
target_include_directories(binding_cost PRIVATE "${WICKRA_EXCHANGE_INCLUDE}")
target_link_libraries(binding_cost PRIVATE "${WICKRA_EXCHANGE_LIB}")
if(NOT WIN32)
    target_link_libraries(binding_cost PRIVATE m)
endif()

# On Windows the runtime DLL must sit next to the executable: there is no rpath,
# and without it the loader pops a modal dialog rather than failing.
if(WIN32)
    add_custom_command(TARGET binding_cost POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
            "${WICKRA_EXCHANGE_LIB_DIR}/wickra_exchange.dll"
            "$<TARGET_FILE_DIR:binding_cost>"
        VERBATIM)
endif()
