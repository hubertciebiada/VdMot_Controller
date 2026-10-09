#!/usr/bin/env python3
"""The gzip and ETag rules of the embedded dashboard (web/*).

software_esp32_rust/glue/build.rs (feature `dashboard`) imports this module and embeds every
regular file of web/ (recursively, hidden files skipped) gzip-compressed deterministically
(level 9, mtime 0, no file name), so identical sources give identical bytes and ETags (the
CRC32 of the gzip bytes), within the budget of MAX_TOTAL_GZ.
"""
from __future__ import annotations

import gzip
import io
import os

CONTENT_TYPES = {
    ".html": "text/html; charset=utf-8",
    ".js": "application/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".json": "application/json",
    ".svg": "image/svg+xml",
    ".ico": "image/x-icon",
    ".png": "image/png",
    ".txt": "text/plain; charset=utf-8",
}
# Budget for the embedded UI (compressed). The app image must stay < 1.1 MB.
MAX_TOTAL_GZ = 160 * 1024


def gzip_bytes(data: bytes) -> bytes:
    buf = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=buf, compresslevel=9, mtime=0) as gz:
        gz.write(data)
    return buf.getvalue()


def collect(web_dir: str) -> list[tuple[str, str]]:
    files = []
    for root, dirs, names in os.walk(web_dir):
        dirs[:] = sorted(d for d in dirs if not d.startswith("."))
        for name in sorted(names):
            if name.startswith("."):
                continue
            full = os.path.join(root, name)
            url = "/" + os.path.relpath(full, web_dir).replace(os.sep, "/")
            files.append((url, full))
    return files
