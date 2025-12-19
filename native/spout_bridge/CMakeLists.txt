cmake_minimum_required(VERSION 3.20)
project(spout_bridge LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# SPOUT2_DIR is passed from build.rs:
# -DSPOUT2_DIR=.../native/spout2
if(NOT DEFINED SPOUT2_DIR)
  message(FATAL_ERROR "SPOUT2_DIR not set. Pass -DSPOUT2_DIR=path/to/native/spout2")
endif()

# Normalize to forward slashes so CMake never sees \U or other escapes
file(TO_CMAKE_PATH "${SPOUT2_DIR}" SPOUT2_DIR_CMAKE)

set(SPOUTSDK_DIR "${SPOUT2_DIR_CMAKE}/SPOUTSDK")
set(SPOUTGL_DIR  "${SPOUTSDK_DIR}/SpoutGL")
set(SPOUTLIB_DIR "${SPOUTSDK_DIR}/SpoutLibrary")

# --- Build Spout sources as a static library ---
set(SPOUT_SOURCES
  "${SPOUTGL_DIR}/Spout.cpp"
  "${SPOUTGL_DIR}/SpoutCopy.cpp"
  "${SPOUTGL_DIR}/SpoutDirectX.cpp"
  "${SPOUTGL_DIR}/SpoutFrameCount.cpp"
  "${SPOUTGL_DIR}/SpoutGL.cpp"
  "${SPOUTGL_DIR}/SpoutGLextensions.cpp"
  "${SPOUTGL_DIR}/SpoutReceiver.cpp"
  "${SPOUTGL_DIR}/SpoutSender.cpp"
  "${SPOUTGL_DIR}/SpoutSenderNames.cpp"
  "${SPOUTGL_DIR}/SpoutSharedMemory.cpp"
  "${SPOUTGL_DIR}/SpoutUtils.cpp"
  "${SPOUTLIB_DIR}/SpoutLibrary.cpp"
)

add_library(spout_static STATIC ${SPOUT_SOURCES})

target_include_directories(spout_static PRIVATE
  "${SPOUTGL_DIR}"
  "${SPOUTLIB_DIR}"
  "${SPOUTSDK_DIR}"
)

if(MSVC)
  target_compile_definitions(spout_static PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
  # Optional, quiets the EH warnings you saw:
  target_compile_options(spout_static PRIVATE /EHsc)
endif()

# Spout uses OpenGL + DirectX + WinMM on Windows
target_link_libraries(spout_static PRIVATE
  opengl32
  user32
  gdi32
  dxgi
  d3d11
  winmm
)

# --- Build your C-ABI bridge DLL ---
add_library(spout_bridge SHARED
  spout_bridge.cpp
  spout_bridge.h
)

# Ensure exports exist even if declspecs get weird across configs
set_target_properties(spout_bridge PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS ON)

target_include_directories(spout_bridge PRIVATE
  "${SPOUTGL_DIR}"
  "${SPOUTLIB_DIR}"
  "${SPOUTSDK_DIR}"
)

if(MSVC)
  target_compile_definitions(spout_bridge PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
  target_compile_options(spout_bridge PRIVATE /EHsc)
endif()

target_link_libraries(spout_bridge PRIVATE
  spout_static
  opengl32
  user32
  gdi32
  dxgi
  d3d11
  winmm
)

# Some build systems want an explicit output name:
set_target_properties(spout_bridge PROPERTIES OUTPUT_NAME "spout_bridge")
