// native/syphon_bridge.h
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void* syphon_server_create(const char* name_utf8);
void  syphon_server_destroy(void* server_ptr);
void  syphon_server_publish_texture(void* server_ptr, uint32_t tex_id, int32_t width, int32_t height);

#ifdef __cplusplus
}
#endif
