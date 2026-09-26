#!/usr/bin/env python3
"""Bound the memory one HTTP request can take in AsyncWebServer_WT32_ETH01,
and size AsyncTCP's task stack.

The pinned library (1.6.2) keeps request data in Strings without limits:
a header line grows until a newline arrives, every header and every
parameter is a heap object, and a non-file multipart field is appended byte
by byte. Our size guard (web_server.cpp) only sees Content-Length after the
headers, so one LAN client could exhaust the heap before authentication
(abort() -> reboot, which also resets the STM through IO15).

This script patches the library source after PlatformIO installed it:
  - request line + headers: at most MAX_HEADER_BYTES in total and
    MAX_HEADERS header lines, else the connection is aborted;
  - query and form parameters: at most MAX_PARAMS, further ones are dropped;
  - multipart part header lines: at most MAX_PART_LINE bytes, non-file
    fields at most MAX_FIELD bytes, else the multipart parse fails (the
    rest of the body is discarded, the handler answers the request);
  - the existing close() on an empty request line becomes abort():
    close() deletes the request synchronously inside its own data callback.
Aborting is deferred by AsyncTCP (error event on the async task), so the
request object stays valid until _onData returns.

AsyncTCP 1.1.1 creates its task ("async_tcp", every web handler runs there)
with a fixed 16 KB stack; the script puts in app::kAsyncTcpStackBytes of
src/app.h (the heap of the WT32-ETH01 needs the difference). A file patched
with another value fails the build: delete .pio/libdeps to patch it again.

Runs as a PlatformIO post script (lib_deps are installed while the build
script runs, after the pre scripts; compilation starts after the post
scripts). Idempotent: an already patched file is left alone. Any anchor
that is not found exactly once fails the build, so a library update cannot
silently drop the limits.

    python3 tools/patch_libs.py LIBDEPS_ENV_DIR [PROJECT_DIR]   (standalone)
"""
from __future__ import annotations

import os
import re
import sys

MARKER = "VDM-PATCH-HTTP-LIMITS v1"
MAX_HEADER_BYTES = 4096
MAX_HEADERS = 32
MAX_PARAMS = 16
MAX_PART_LINE = 512
MAX_FIELD = 256

LIB = "AsyncWebServer_WT32_ETH01"
ASYNC_TCP_LIB = "AsyncTCP"
ASYNC_TCP_MARKER = "VDM-PATCH-ASYNC-TCP-STACK v1"

HEADER_EDITS = [
    (
        "    size_t _parsedLength;\n",
        "    size_t _parsedLength;\n"
        f"    size_t _vdmHeaderBytes = 0;  // {MARKER}\n"
        "    void _vdmFail();\n",
    ),
]

SOURCE_EDITS = [
    # Limits.
    (
        "enum { PARSE_REQ_START, PARSE_REQ_HEADERS, PARSE_REQ_BODY, PARSE_REQ_END, PARSE_REQ_FAIL };\n",
        "enum { PARSE_REQ_START, PARSE_REQ_HEADERS, PARSE_REQ_BODY, PARSE_REQ_END, PARSE_REQ_FAIL };\n"
        f"// {MARKER}: bounded request parsing (tools/patch_libs.py)\n"
        f"#define VDM_HTTP_MAX_HEADER_BYTES {MAX_HEADER_BYTES}\n"
        f"#define VDM_HTTP_MAX_HEADERS {MAX_HEADERS}\n"
        f"#define VDM_HTTP_MAX_PARAMS {MAX_PARAMS}\n"
        f"#define VDM_HTTP_MAX_PART_LINE {MAX_PART_LINE}\n"
        f"#define VDM_HTTP_MAX_FIELD {MAX_FIELD}\n",
    ),
    # Header bytes budget, counted before anything is stored.
    (
        "      for (i = 0; i < len; i++)\n"
        "      {\n"
        "        if (str[i] == '\\n')\n"
        "        {\n"
        "          break;\n"
        "        }\n"
        "      }\n"
        "\n"
        "      if (i == len)\n",
        "      for (i = 0; i < len; i++)\n"
        "      {\n"
        "        if (str[i] == '\\n')\n"
        "        {\n"
        "          break;\n"
        "        }\n"
        "      }\n"
        "\n"
        "      _vdmHeaderBytes += (i == len) ? len : i + 1;\n"
        "\n"
        "      if (_vdmHeaderBytes > VDM_HTTP_MAX_HEADER_BYTES)\n"
        "      {\n"
        "        _vdmFail();\n"
        "        return;\n"
        "      }\n"
        "\n"
        "      if (i == len)\n",
    ),
    (
        "        _temp.trim();\n"
        "        _parseLine();\n"
        "\n",
        "        _temp.trim();\n"
        "        _parseLine();\n"
        "\n"
        "        if (_parseState == PARSE_REQ_FAIL)\n"
        "          return;\n"
        "\n",
    ),
    # Deferred abort instead of a synchronous close (use after free).
    (
        "      _parseState = PARSE_REQ_FAIL;\n"
        "      _client->close();\n",
        "      _vdmFail();\n",
    ),
    (
        "bool AsyncWebServerRequest::_parseReqHeader()\n"
        "{\n",
        "void AsyncWebServerRequest::_vdmFail()\n"
        "{\n"
        "  _temp = String();\n"
        "  _parseState = PARSE_REQ_FAIL;\n"
        "  _client->abort();\n"
        "}\n"
        "\n"
        "bool AsyncWebServerRequest::_parseReqHeader()\n"
        "{\n"
        "  if (_headers.length() >= VDM_HTTP_MAX_HEADERS)\n"
        "  {\n"
        "    _vdmFail();\n"
        "    return false;\n"
        "  }\n"
        "\n",
    ),
    (
        "void AsyncWebServerRequest::_addParam(AsyncWebParameter *p)\n"
        "{\n"
        "  _params.add(p);\n"
        "}\n",
        "void AsyncWebServerRequest::_addParam(AsyncWebParameter *p)\n"
        "{\n"
        "  if (_params.length() >= VDM_HTTP_MAX_PARAMS)\n"
        "  {\n"
        "    delete p;\n"
        "    return;\n"
        "  }\n"
        "\n"
        "  _params.add(p);\n"
        "}\n",
    ),
    (
        "#define itemWriteByte(b)        do { _itemSize++; if(_itemIsFile) _handleUploadByte(b, last); "
        "else _itemValue+=(char)(b); } while(0)\n",
        "#define itemWriteByte(b)        do { _itemSize++; if(_itemIsFile) _handleUploadByte(b, last); "
        "else if(_itemValue.length() < VDM_HTTP_MAX_FIELD) _itemValue+=(char)(b); "
        "else _multiParseState = PARSE_ERROR; } while(0)\n",
    ),
    (
        "    if ((char)data != '\\r' && (char)data != '\\n')\n"
        "      _temp += (char)data;\n",
        "    if (_temp.length() >= VDM_HTTP_MAX_PART_LINE)\n"
        "    {\n"
        "      _temp = String();\n"
        "      _multiParseState = PARSE_ERROR;\n"
        "\n"
        "      return;\n"
        "    }\n"
        "\n"
        "    if ((char)data != '\\r' && (char)data != '\\n')\n"
        "      _temp += (char)data;\n",
    ),
]


def async_tcp_stack(project_dir: str) -> int:
    """app::kAsyncTcpStackBytes from src/app.h."""
    with open(os.path.join(project_dir, "src", "app.h"), "r", encoding="utf-8") as f:
        m = re.search(r"kAsyncTcpStackBytes = (\d+);", f.read())
    if m is None:
        raise RuntimeError("src/app.h: kAsyncTcpStackBytes not found")
    return int(m.group(1))


def async_tcp_edits(stack: int) -> list[tuple[str, str]]:
    return [(
        'xTaskCreateUniversal(_async_service_task, "async_tcp", 8192 * 2, ',
        f'xTaskCreateUniversal(_async_service_task, "async_tcp", {stack} /* {ASYNC_TCP_MARKER} {stack} */, ',
    )]


def patch_file(path: str, edits, marker: str = MARKER) -> bool:
    """Returns True when the file was changed. Raises on a missing anchor."""
    with open(path, "r", encoding="utf-8", newline="") as f:
        text = f.read()
    if marker in text:
        return False
    crlf = "\r\n" in text
    if crlf:
        text = text.replace("\r\n", "\n")
    for old, new in edits:
        n = text.count(old)
        if n != 1:
            raise RuntimeError(f"{path}: anchor found {n} times, expected once:\n{old}")
        text = text.replace(old, new)
    if marker not in text:
        raise RuntimeError(f"{path}: patch has no marker")
    if crlf:
        text = text.replace("\n", "\r\n")
    tmp = path + ".vdmtmp"
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        f.write(text)
    os.replace(tmp, path)
    return True


def patch(libdeps_env_dir: str, project_dir: str) -> list[str]:
    src = os.path.join(libdeps_env_dir, LIB, "src")
    if not os.path.isdir(src):
        raise RuntimeError(f"{src}: library not installed")
    changed = []
    for name, edits in (("AsyncWebServer_WT32_ETH01.h", HEADER_EDITS),
                        ("WebRequest.cpp", SOURCE_EDITS)):
        path = os.path.join(src, name)
        if patch_file(path, edits):
            changed.append(f"{LIB}/src/{name}")
    stack = async_tcp_stack(project_dir)
    path = os.path.join(libdeps_env_dir, ASYNC_TCP_LIB, "src", "AsyncTCP.cpp")
    with open(path, "r", encoding="utf-8", newline="") as f:
        text = f.read()
    marker = f"{ASYNC_TCP_MARKER} {stack} "
    if ASYNC_TCP_MARKER in text and marker not in text:
        raise RuntimeError(f"{path}: patched with another stack size; delete .pio/libdeps")
    if patch_file(path, async_tcp_edits(stack), marker):
        changed.append(f"{ASYNC_TCP_LIB}/src/AsyncTCP.cpp")
    return changed


def _run_pio(env) -> None:
    target = os.path.join(env.subst("$PROJECT_LIBDEPS_DIR"), env.subst("$PIOENV"))
    try:
        changed = patch(target, env.subst("$PROJECT_DIR"))
    except (OSError, RuntimeError) as e:
        sys.stderr.write(f"patch_libs.py: {e}\n")
        env.Exit(1)
        return
    for name in changed:
        print(f"patch_libs.py: patched {name}")


def _in_scons() -> bool:
    try:
        Import  # type: ignore[name-defined]  # noqa: B018,F821 - provided by SCons
    except NameError:
        return False
    return True


if _in_scons():
    Import("env")  # type: ignore[name-defined]  # noqa: F821
    _run_pio(env)  # type: ignore[name-defined]  # noqa: F821
elif __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    project = sys.argv[2] if len(sys.argv) == 3 else os.path.dirname(os.path.dirname(os.path.abspath(sys.argv[0])))
    try:
        for n in patch(sys.argv[1], project):
            print(f"patched {n}")
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
