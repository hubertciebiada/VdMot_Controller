// HTTP server (AsyncWebServer on port 80): gzip-embedded dashboard, the
// /api/* JSON API (DESIGN.md "HTTP API", binding), Basic auth, uploads for
// ESP OTA and STM images. Handlers run in the AsyncTCP task: they never
// block on the STM, never touch the UART, NVS writes go through
// storage::applyConfig, STM actions through app::submit.
//
// Request size limits are enforced before the library buffers anything: a
// guard handler answers oversized, unauthenticated or conflicting uploads
// and bodies without parsing them (DESIGN.md "Persistence and memory").
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace web {

// Response buffers: a fixed pool, one slot per in-flight JSON response
// (released when the client disconnects, i.e. after the response). No
// per-request heap growth; when every slot is busy the handler answers 503.
constexpr size_t kResponseSlots = 2;
constexpr size_t kResponseSlotSize = 12 * 1024;
// JSON POST bodies are collected in one static buffer (one body at a time,
// 409 when busy, 413 when larger).
constexpr size_t kMaxBodySize = 8192;
// Uploads (multipart): STM image file size limit and the allowance for the
// multipart framing around the file in Content-Length.
constexpr size_t kMaxStmImageSize = 512 * 1024;
constexpr size_t kMultipartSlack = 8 * 1024;

// Starts the server once the network is up (idempotent).
void begin();
bool started();

}  // namespace web
