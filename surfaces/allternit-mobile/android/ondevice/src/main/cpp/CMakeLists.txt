# Allternit on-device engine: llama.cpp + a small JNI layer (engine_jni.cpp).
#
# Pins (change all three together, and re-run the Moto check in ondevice/README.md):
#   llama.cpp      b12414  commit 278a89cb787c1d2b4cff9748b1df9cfd2aa69976 (same pin as the
#                  Desktop engine pack, spec/on-device/engine-pack-spike.md)
#   OpenCL-Headers + OpenCL-ICD-Loader v2025.07.22 (link-time only, see below)
#
# Backends are separate shared libraries (GGML_BACKEND_DL) loaded at run time from the
# app's native lib dir: libggml-cpu-<variant>.so for every arm64 CPU level, and
# libggml-opencl.so for Adreno GPUs. A phone without a public libOpenCL.so simply fails
# to load the OpenCL backend and runs on the CPU.
cmake_minimum_required(VERSION 3.22)
project(allternit_ondevice C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include(FetchContent)
set(FETCHCONTENT_QUIET ON)

# --- OpenCL: headers + the Khronos ICD loader, used only so libggml-opencl.so links. The
# loader is NOT packaged (ondevice/build.gradle.kts excludes libOpenCL.so): Qualcomm phones
# list /vendor/lib64/libOpenCL.so in public.libraries.txt, and the app manifest declares it
# with <uses-native-library android:required="false">, so the dynamic linker binds to the
# vendor driver directly.
FetchContent_Declare(opencl_headers
    URL https://github.com/KhronosGroup/OpenCL-Headers/archive/refs/tags/v2025.07.22.tar.gz
    URL_HASH SHA256=98f0a3ea26b4aec051e533cb1750db2998ab8e82eda97269ed6efe66ec94a240
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
FetchContent_Declare(opencl_icd_loader
    URL https://github.com/KhronosGroup/OpenCL-ICD-Loader/archive/refs/tags/v2025.07.22.tar.gz
    URL_HASH SHA256=dff7a0b11ad5b63a669358e3476e3dc889a4a361674e5b69b267b944d0794142
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
FetchContent_Declare(llamacpp
    URL https://github.com/ggml-org/llama.cpp/archive/refs/tags/b12414.tar.gz
    URL_HASH SHA256=90ac5d4699bfd4758d110bab700f57a44f2387439ffe7f43460f69592fa08b20
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE)

FetchContent_GetProperties(opencl_headers)
if(NOT opencl_headers_POPULATED)
    FetchContent_Populate(opencl_headers)
endif()
set(OPENCL_ICD_LOADER_HEADERS_DIR "${opencl_headers_SOURCE_DIR}" CACHE PATH "" FORCE)
set(OPENCL_ICD_LOADER_BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(OPENCL_ICD_LOADER_BUILD_SHARED_LIBS ON CACHE BOOL "" FORCE)
set(ENABLE_OPENCL_LAYERINFO OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
FetchContent_GetProperties(opencl_icd_loader)
if(NOT opencl_icd_loader_POPULATED)
    FetchContent_Populate(opencl_icd_loader)
    add_subdirectory(${opencl_icd_loader_SOURCE_DIR} ${opencl_icd_loader_BINARY_DIR} EXCLUDE_FROM_ALL)
endif()
# FindOpenCL (called by ggml-opencl) takes these as found; OpenCL_LIBRARY names the loader
# target so the link step depends on it.
set(OpenCL_INCLUDE_DIR "${opencl_headers_SOURCE_DIR}" CACHE PATH "" FORCE)
set(OpenCL_LIBRARY OpenCL CACHE STRING "" FORCE)

# --- llama.cpp
set(BUILD_SHARED_LIBS ON CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_BACKEND_DL ON CACHE BOOL "" FORCE)
set(GGML_CPU_ALL_VARIANTS ON CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(GGML_LLAMAFILE OFF CACHE BOOL "" FORCE)
set(GGML_OPENCL ON CACHE BOOL "" FORCE)
set(GGML_OPENCL_EMBED_KERNELS ON CACHE BOOL "" FORCE)
set(GGML_OPENCL_USE_ADRENO_KERNELS ON CACHE BOOL "" FORCE)
set(LLAMA_BUILD_COMMON ON CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TOOLS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_APP OFF CACHE BOOL "" FORCE)
set(LLAMA_CURL OFF CACHE BOOL "" FORCE)
set(LLAMA_OPENSSL OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_IS_DEV OFF CACHE BOOL "" FORCE)
FetchContent_MakeAvailable(llamacpp)

add_library(allternit_llama SHARED engine_jni.cpp)
target_include_directories(allternit_llama PRIVATE
    ${llamacpp_SOURCE_DIR}/include
    ${llamacpp_SOURCE_DIR}/common
    ${llamacpp_SOURCE_DIR}/ggml/include)
target_link_libraries(allternit_llama PRIVATE llama llama-common ggml android log)
# The backend .so files (libggml-cpu-android_armv*.so, libggml-opencl.so) are loaded with
# dlopen, not linked. Gradle builds the default "all" target, so they are built and packaged.
# 16 KB page alignment (Play requires it for apps targeting Android 15+ devices).
target_link_options(allternit_llama PRIVATE "-Wl,-z,max-page-size=16384")
