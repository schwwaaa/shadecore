#include "spout_bridge.h"

#include <memory>
#include <mutex>
#include <string>

#include "SpoutSender.h"

// IMPORTANT:
// Avoid global/static Spout objects with non-trivial constructors.
// If their initialization touches Win32/COM/GL or loads other modules during DLL load,
// Windows can fail module initialization with STATUS_DLL_INIT_FAILED (0xc0000142).
// We'll create/destroy SpoutSender lazily from the exported C API functions instead.

static std::mutex g_mutex;
static std::unique_ptr<SpoutSender> g_sender;
static std::string g_sender_name;

static void ensure_sender() {
    if (!g_sender) g_sender = std::make_unique<SpoutSender>();
}

extern "C" {

int spout_init_sender(const char* sender_name_utf8, int width, int height) {
    try {
        std::lock_guard<std::mutex> lock(g_mutex);

        const char* name = (sender_name_utf8 && *sender_name_utf8) ? sender_name_utf8 : "shadecore";

        // (Re)create the sender if name changed or if we don't have one yet.
        if (!g_sender || g_sender_name != name) {
            if (g_sender) {
                // Best-effort cleanup.
                g_sender->ReleaseSender();
            }
            g_sender.reset();
            ensure_sender();
            g_sender_name = name;
            g_sender->SetSenderName(g_sender_name.c_str());
        }

        // Create or update the sender.
        // Spout expects unsigned sizes.
        const unsigned int w = (width > 0) ? (unsigned int)width : 1u;
        const unsigned int h = (height > 0) ? (unsigned int)height : 1u;

        if (!g_sender->CreateSender(g_sender_name.c_str(), w, h)) {
            // If already exists, try update.
            if (!g_sender->UpdateSender(g_sender_name.c_str(), w, h)) {
                return 0;
            }
        }

        return 1;
    } catch (...) {
        return 0;
    }
}

int spout_send_gl_texture(unsigned int gl_tex_id, int width, int height, int invert) {
    try {
        std::lock_guard<std::mutex> lock(g_mutex);
        if (!g_sender) return 0;

        const unsigned int w = (width > 0) ? (unsigned int)width : 1u;
        const unsigned int h = (height > 0) ? (unsigned int)height : 1u;
        const bool inv = invert != 0;

        // Ensure sender exists with the latest dimensions.
        if (!g_sender_name.empty()) {
            g_sender->UpdateSender(g_sender_name.c_str(), w, h);
        }

        return g_sender->SendTexture(gl_tex_id, GL_TEXTURE_2D, w, h, inv) ? 1 : 0;
    } catch (...) {
        return 0;
    }
}

void spout_shutdown() {
    try {
        std::lock_guard<std::mutex> lock(g_mutex);
        if (g_sender) {
            g_sender->ReleaseSender();
            g_sender.reset();
        }
        g_sender_name.clear();
    } catch (...) {
        // swallow
    }
}

} // extern "C"
