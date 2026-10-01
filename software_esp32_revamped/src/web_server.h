// HTTP server (AsyncWebServer on port 80): gzip-embedded dashboard, the
// /api/* JSON API (DESIGN.md "HTTP API", binding), uploads for ESP OTA and
// STM images. Handlers run in the AsyncTCP task: they never block on the
// STM, never touch the UART, NVS writes go through storage::applyConfig,
// STM actions through app::submit.
//
// Request size limits are enforced before the library buffers anything: a
// guard handler answers oversized or conflicting uploads and bodies
// without parsing them (DESIGN.md "Persistence and memory").
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace web {

// Response buffers: a fixed pool, one slot per in-flight JSON response
// (released when the client disconnects, i.e. after the response). A slot's
// buffer is allocated at its first use and kept, so there is no per-request
// heap growth. (2.1.0 gave them back to the heap after 30 s without a web
// client; 2.1.1 withdrew that after a controller crashed with it, cause not
// found.) When every slot is busy or a buffer cannot be allocated the handler
// answers 503.
constexpr size_t kResponseSlots = 2;
constexpr size_t kResponseSlotSize = 12 * 1024;
// A JSON body is received into a response slot, which then carries the
// answer: one body at a time (409 when another one arrives), 503 when no slot
// is free, 413 when larger.
constexpr size_t kMaxBodySize = 8192;
static_assert(kMaxBodySize < kResponseSlotSize, "a body and its terminator fit a slot");
// Connections the server holds at a time: tools/patch_libs.py reads this value
// and makes it the listen backlog of AsyncTCP (lwIP drops a SYN beyond it).
constexpr size_t kMaxConnections = 4;
// Uploads (multipart): STM image file size limit and the allowance for the
// multipart framing around the file in Content-Length.
constexpr size_t kMaxStmImageSize = 512 * 1024;
constexpr size_t kMultipartSlack = 8 * 1024;

// Starts the server once the network is up (idempotent).
void begin();
bool started();

}  // namespace web
