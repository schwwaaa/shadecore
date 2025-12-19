// native/syphon_bridge.m
// Syphon bridge compiled into libsyphon_bridge.a via build.rs (cc-rs)
//
// This file exports C symbols that match what src/main.rs declares:
//
//   syphon_server_create(const char*)
//   syphon_server_publish_texture(void*, uint32_t, int32_t, int32_t)
//   syphon_server_destroy(void*)
//
// NOTE: SyphonOpenGLServer expects a CGLContextObj, not an NSOpenGLContext*.
// We obtain the current NSOpenGLContext and convert using -[NSOpenGLContext CGLContextObj].
//
// NOTE: GL constants like GL_TEXTURE_2D require OpenGL headers.

#import <Cocoa/Cocoa.h>
#import <OpenGL/OpenGL.h>
#import <OpenGL/gl3.h>

#import "Syphon.h"

#import "syphon_bridge.h"

@interface GLSLEngineSyphonWrapper : NSObject
@property (nonatomic, strong) SyphonOpenGLServer *server;
@end

@implementation GLSLEngineSyphonWrapper
@end

void* syphon_server_create(const char* name_utf8) {
    @autoreleasepool {
        NSString *name = name_utf8 ? [NSString stringWithUTF8String:name_utf8] : @"glsl_engine";

        // Must be called on the thread where the OpenGL context is current.
        NSOpenGLContext *ns_ctx = [NSOpenGLContext currentContext];
        if (!ns_ctx) {
            return NULL;
        }

        CGLContextObj cgl_ctx = [ns_ctx CGLContextObj];

        GLSLEngineSyphonWrapper *wrap = [GLSLEngineSyphonWrapper new];
        wrap.server = [[SyphonOpenGLServer alloc] initWithName:name context:cgl_ctx options:nil];

        return (__bridge_retained void*)wrap;
    }
}

void syphon_server_destroy(void* server_ptr) {
    @autoreleasepool {
        if (!server_ptr) return;
        GLSLEngineSyphonWrapper *wrap = (__bridge_transfer GLSLEngineSyphonWrapper*)server_ptr;
        wrap.server = nil;
        (void)wrap;
    }
}

void syphon_server_publish_texture(void* server_ptr, uint32_t tex_id, int32_t width, int32_t height) {
    @autoreleasepool {
        if (!server_ptr) return;

        GLSLEngineSyphonWrapper *wrap = (__bridge GLSLEngineSyphonWrapper*)server_ptr;
        if (!wrap.server) return;

        NSSize size = NSMakeSize((CGFloat)width, (CGFloat)height);

        [wrap.server publishFrameTexture:(GLuint)tex_id
                           textureTarget:GL_TEXTURE_2D
                             imageRegion:NSMakeRect(0, 0, size.width, size.height)
                       textureDimensions:size
                                 flipped:false];
    }
}
