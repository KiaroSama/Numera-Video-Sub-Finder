cmake_minimum_required(VERSION 3.18)
project(nvsf_native_kernel_witness LANGUAGES C CXX)

if(NOT IS_ABSOLUTE "${NVSF_LLAMA_SOURCE}" OR NOT EXISTS "${NVSF_LLAMA_SOURCE}/ggml/src/ggml-cpu/vec.cpp")
    message(FATAL_ERROR "NVSF_LLAMA_SOURCE must identify the exact verified upstream checkout")
endif()
if(BUILD_SHARED_LIBS OR GGML_BACKEND_DL OR GGML_CPU_ALL_VARIANTS)
    message(FATAL_ERROR "Kernel witness requires the same single static ggml-cpu backend as the packaged helper")
endif()

# Upstream owns its complete target definitions and compiler options; do not copy vec.cpp into this target.
add_subdirectory("${NVSF_LLAMA_SOURCE}" llama)
if(NOT TARGET ggml-cpu OR NOT TARGET ggml-base OR NOT TARGET llama-mtmd-cli)
    message(FATAL_ERROR "Expected actual ggml-cpu, ggml-base and packaged mtmd targets")
endif()
if(NVSF_REQUIRE_ARM_NEON)
    set(neon_guard "${CMAKE_CURRENT_BINARY_DIR}/require_arm_neon.h")
    file(WRITE "${neon_guard}" "#if !defined(__aarch64__) || !defined(__ARM_NEON)\n#error Required ARM64 NEON kernel branch is unavailable\n#endif\n")
    if(MSVC)
        target_compile_options(ggml-cpu PRIVATE "$<$<COMPILE_LANGUAGE:CXX>:/FI${neon_guard}>")
    else()
        target_compile_options(ggml-cpu PRIVATE "$<$<COMPILE_LANGUAGE:CXX>:-include>" "$<$<COMPILE_LANGUAGE:CXX>:${neon_guard}>")
    endif()
endif()

find_program(NVSF_RUSTC rustc REQUIRED)
set(NVSF_WITNESS_DIR "${CMAKE_BINARY_DIR}/witness-bin")
set(NVSF_WITNESS_OUT "${NVSF_WITNESS_DIR}/nvsf-llama-bf16${CMAKE_EXECUTABLE_SUFFIX}")
set(NVSF_WITNESS_SRC "${CMAKE_CURRENT_SOURCE_DIR}/llama_bf16.rs")

if(WIN32)
    set(NVSF_CXX_RUNTIME_LIBS -l dylib=advapi32)
else()
    set(NVSF_CXX_RUNTIME_LIBS -l dylib=stdc++ -l dylib=m -l dylib=pthread -l dylib=dl)
endif()

add_custom_command(
    OUTPUT "${NVSF_WITNESS_OUT}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${NVSF_WITNESS_DIR}"
    COMMAND "${NVSF_RUSTC}"
        --edition 2024
        -O
        "${NVSF_WITNESS_SRC}"
        -o "${NVSF_WITNESS_OUT}"
        -L "native=$<TARGET_FILE_DIR:ggml-cpu>"
        -L "native=$<TARGET_FILE_DIR:ggml-base>"
        -l static=ggml-cpu
        -l static=ggml-base
        ${NVSF_CXX_RUNTIME_LIBS}
    DEPENDS ggml-cpu ggml-base "${NVSF_WITNESS_SRC}"
    VERBATIM
)
add_custom_target(nvsf-llama-bf16 DEPENDS "${NVSF_WITNESS_OUT}")
