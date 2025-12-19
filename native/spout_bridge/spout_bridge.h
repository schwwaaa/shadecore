#pragma once

#include <stdint.h>

#ifdef _WIN32
  #define SPOUT_BRIDGE_API __declspec(dllexport)
#else
  #define SPOUT_BRIDGE_API
#endif

extern "C" {

// Initialize a Spout sender (creates sender if needed).
// Returns 1 on success, 0 on failure.
SPOUT_BRIDGE_API int spout_init_sender(const char* sender_name_utf8, int width, int height);

// Send an OpenGL texture (GL_TEXTURE_2D) via Spout.
// invert: 1 to invert vertically, 0 for normal.
// Returns 1 on success, 0 on failure.
SPOUT_BRIDGE_API int spout_send_gl_texture(uint32_t gl_tex_id, int width, int height, int invert);

// Shutdown / release sender resources.
SPOUT_BRIDGE_API void spout_shutdown();

} // extern "C"
