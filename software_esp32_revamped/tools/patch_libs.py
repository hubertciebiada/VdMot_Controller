#!/usr/bin/env python3
"""Bound the memory one HTTP request can take in AsyncWebServer_WT32_ETH01,
size AsyncTCP's task stack and guard AsyncTCP's event allocations.

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

AsyncTCP 1.1.1 also allocates every lwIP event packet with malloc() and
writes to it without a NULL check, so a failed allocation under memory
pressure (concurrent connections) panics the lwIP thread. The script guards
every allocation: an event without heap is dropped (the callback returns
ERR_OK as if it had queued it), received data without heap is refused with
ERR_MEM and stays with lwIP, which delivers it again, a FIN without heap
still closes the pcb in the lwIP thread (the client object is not told and
stays allocated), and a purge request without heap purges the queue inline.
The accept path creates the client with new (std::nothrow), so the
library's own NULL branch (close the pcb) runs instead of abort().

Under a storm of connections the heap of the WT32-ETH01 is the limit, so
the script caps the connections of a server at ASYNC_TCP_MAX_CONN: every
accepted connection counts against the listen backlog (tcp_backlog_delayed
in the accept callback) until lwIP purges its pcb, and the backlog is that
cap; a SYN beyond it is dropped by lwIP before any allocation and the
peer's TCP retries it (1 s, 2 s, ...) instead of getting a reset. A file
patched with another cap fails the build, like the stack size.

Two more defects of the library on that path are patched: the lwIP
callbacks used the client behind arg without a NULL check although
_close() clears tcp_arg() from another task (data without a client is
consumed like lwIP's tcp_recv_null does, everything else is dropped), and
_error() wrote into the pcb that lwIP had freed before the error callback
(tcp.h: tcp_err_fn); it now only forgets the pcb and purges the client's
queued events before the client is deleted, as _fin() does.

The event queue (32 packets) is where the library deadlocks: the lwIP
thread blocked in xQueueSend(portMAX_DELAY) while the async task waited
for the lwIP thread in tcpip_api_call, and the purge of a client's events
(_remove_events_with_arg) blocked the same way on the async task; the
task watchdog then restarts the controller. Every send into the queue now
waits at most ASYNC_TCP_QUEUE_WAIT_MS and drops the event: data the queue
refuses stays with lwIP (ERR_MEM, delivered again), a refused purge runs
inline, and the purge itself visits the queued packets once without
waiting (and frees the pbuf of a dropped receive, which leaked).

The closed-slot bookkeeping (_closed_slots, one per pcb) gave a slot back
only in _lwip_fin: every close(), abort() or error kept its slot, and once
all 16 were taken the constructor and _lwip_fin wrote before the array
(_closed_slot == -1). Every end of a pcb releases its slot now and -1 is
never used as an index.

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
# Connections a server holds at a time (listen backlog); the peers' TCP queues the rest.
ASYNC_TCP_MAX_CONN = 4
# Wait for a slot in the full event queue (the lwIP thread, and the purge on the async
# task), then the event is dropped.
ASYNC_TCP_QUEUE_WAIT_MS = 10

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


ASYNC_TCP_ALLOC_MARKER = "VDM-PATCH-ASYNC-TCP-ALLOC v1"
ASYNC_TCP_ALLOC = "    lwip_event_packet_t * e = (lwip_event_packet_t *)malloc(sizeof(lwip_event_packet_t));\n"


def async_tcp_guard(anchor: str, on_null: str, why: str) -> tuple[str, str]:
    """One lwIP callback: the NULL check follows its malloc, anchor is the line after the malloc."""
    return (
        ASYNC_TCP_ALLOC + anchor,
        ASYNC_TCP_ALLOC + f"    if (!e) {{ {on_null} }}  // {ASYNC_TCP_ALLOC_MARKER}: {why}\n" + anchor,
    )


ASYNC_TCP_ALLOC_EDITS = [
    (
        '#include "Arduino.h"\n',
        '#include "Arduino.h"\n'
        f"#include <new>  // {ASYNC_TCP_ALLOC_MARKER}: std::nothrow\n",
    ),
    # A purge request (_tcp_clear_events) precedes the delete of a client; without
    # the purge its queued events would reach the freed object, so it runs inline.
    async_tcp_guard("    e->event = LWIP_TCP_CLEAR;\n",
                    "_remove_events_with_arg(arg); return ERR_OK;", "no heap, purged inline"),
    async_tcp_guard("    e->event = LWIP_TCP_CONNECTED;\n", "return ERR_OK;", "no heap, event dropped"),
    async_tcp_guard("    e->event = LWIP_TCP_POLL;\n", "return ERR_OK;", "no heap, event dropped"),
    # Data is refused (lwIP keeps it as refused_data and delivers it again, the
    # window stays closed until then); a FIN closes the pcb in the lwIP thread as
    # the queued path does below, only the client object is not told.
    (
        ASYNC_TCP_ALLOC +
        "    e->arg = arg;\n"
        "    if(pb){\n",
        ASYNC_TCP_ALLOC +
        f"    if (!e) {{  // {ASYNC_TCP_ALLOC_MARKER}: no heap\n"
        "        if (!pb) {\n"
        "            AsyncClient::_s_lwip_fin(arg, pcb, err);  // FIN: close the pcb, the client is not told\n"
        "            return ERR_OK;\n"
        "        }\n"
        "        return ERR_MEM;  // data: lwIP keeps pb and delivers it again\n"
        "    }\n"
        "    e->arg = arg;\n"
        "    if(pb){\n",
    ),
    async_tcp_guard("    e->event = LWIP_TCP_SENT;\n", "return ERR_OK;", "no heap, event dropped"),
    async_tcp_guard("    e->event = LWIP_TCP_ERROR;\n", "return;", "no heap, event dropped"),
    async_tcp_guard('    //ets_printf("+DNS: name=%s ipaddr=0x%08x arg=%x\\n", name, ipaddr, arg);\n',
                    "return;", "no heap, event dropped"),
    async_tcp_guard("    e->event = LWIP_TCP_ACCEPT;\n",
                    "return ERR_OK;", "no heap, event dropped, the client stays allocated"),
    (
        "        AsyncClient *c = new AsyncClient(pcb);\n",
        f"        AsyncClient *c = new (std::nothrow) AsyncClient(pcb);  // {ASYNC_TCP_ALLOC_MARKER}: NULL, not abort()\n",
    ),
]


def verify_async_tcp_alloc(path: str) -> None:
    """Every event allocation is followed by its NULL check and the accept path cannot abort()."""
    with open(path, "r", encoding="utf-8", newline="") as f:
        lines = f.read().replace("\r\n", "\n").split("\n")
    unguarded = [i + 1 for i, line in enumerate(lines)
                 if "malloc(sizeof(lwip_event_packet_t))" in line
                 and not (i + 1 < len(lines) and lines[i + 1].lstrip().startswith("if (!e) {"))]
    if unguarded:
        raise RuntimeError(f"{path}: event allocation without NULL check at line(s) {unguarded}")
    if re.search(r"\bnew AsyncClient\(", "\n".join(lines)):
        raise RuntimeError(f"{path}: new AsyncClient() without std::nothrow")


ASYNC_TCP_ARG_MARKER = "VDM-PATCH-ASYNC-TCP-ARG v1"

ASYNC_TCP_ARG_SIGNATURES = {
    "_tcp_connected": "static int8_t _tcp_connected(void * arg, tcp_pcb * pcb, int8_t err) {\n",
    "_tcp_poll": "static int8_t _tcp_poll(void * arg, struct tcp_pcb * pcb) {\n",
    "_tcp_recv": "static int8_t _tcp_recv(void * arg, struct tcp_pcb * pcb, struct pbuf *pb, int8_t err) {\n",
    "_tcp_sent": "static int8_t _tcp_sent(void * arg, struct tcp_pcb * pcb, uint16_t len) {\n",
    "_tcp_error": "static void _tcp_error(void * arg, int8_t err) {\n",
    "_tcp_dns_found": "static void _tcp_dns_found(const char * name, struct ip_addr * ipaddr, void * arg) {\n",
}


def async_tcp_arg_guard(name: str, on_null: str) -> tuple[str, str]:
    """One lwIP callback: the NULL check of arg follows the signature line."""
    signature = ASYNC_TCP_ARG_SIGNATURES[name]
    return (
        signature,
        signature + f"    if (!arg) {{ {on_null} }}  // {ASYNC_TCP_ARG_MARKER}: no client behind the callback\n",
    )


# _close() clears tcp_arg() on the async task while lwIP keeps calling back: without a
# client the callback is dropped; data is consumed the way lwIP's tcp_recv_null does it
# and a FIN is left to the close under way. _error() runs on the async task after lwIP
# freed the pcb (tcp.h: tcp_err_fn), so it must not touch it.
ASYNC_TCP_ARG_EDITS = [
    async_tcp_arg_guard("_tcp_connected", "return ERR_OK;"),
    async_tcp_arg_guard("_tcp_poll", "return ERR_OK;"),
    async_tcp_arg_guard("_tcp_recv", "if (pb) { tcp_recved(pcb, pb->tot_len); pbuf_free(pb); } return ERR_OK;"),
    async_tcp_arg_guard("_tcp_sent", "return ERR_OK;"),
    async_tcp_arg_guard("_tcp_error", "return;"),
    async_tcp_arg_guard("_tcp_dns_found", "return;"),
    (
        "void AsyncClient::_error(int8_t err) {\n"
        "    if(_pcb){\n"
        "        tcp_arg(_pcb, NULL);\n"
        "        tcp_sent(_pcb, NULL);\n"
        "        tcp_recv(_pcb, NULL);\n"
        "        tcp_err(_pcb, NULL);\n"
        "        tcp_poll(_pcb, NULL, 0);\n"
        "        _pcb = NULL;\n"
        "    }\n",
        "void AsyncClient::_error(int8_t err) {\n"
        f"    _pcb = NULL;  // {ASYNC_TCP_ARG_MARKER}: lwIP freed the pcb before the error callback, it must not be touched\n"
        f"    _tcp_clear_events(this);  // {ASYNC_TCP_ARG_MARKER}: queued events of this client, which _discard_cb deletes\n",
    ),
]


def verify_async_tcp_arg(path: str) -> None:
    """Every callback with a client checks arg, and _error() does not touch the freed pcb."""
    with open(path, "r", encoding="utf-8", newline="") as f:
        text = f.read().replace("\r\n", "\n")
    for name, signature in ASYNC_TCP_ARG_SIGNATURES.items():
        i = text.find(signature)
        if i < 0 or not text[i + len(signature):].lstrip(" ").startswith("if (!arg) {"):
            raise RuntimeError(f"{path}: {name}() without NULL check of arg")
    i = text.find("void AsyncClient::_error(int8_t err) {\n")
    body = text[i:text.find("\n}\n", i)] if i >= 0 else ""
    if i < 0 or "(_pcb" in body:
        raise RuntimeError(f"{path}: AsyncClient::_error() touches the freed pcb")
    if body.find("_tcp_clear_events(this);") < 0 or body.find("_tcp_clear_events(this);") > body.find("_discard_cb"):
        raise RuntimeError(f"{path}: AsyncClient::_error() deletes the client with its events still queued")


ASYNC_TCP_CONN_MARKER = "VDM-PATCH-ASYNC-TCP-CONN v1"


def async_tcp_conn_edits(max_conn: int) -> list[tuple[str, str]]:
    """The listen backlog is the cap: an accepted connection counts against it until lwIP
    purges its pcb (tcp_pcb_purge, tcp_abandon), a SYN beyond it is dropped by lwIP."""
    marker = f"/* {ASYNC_TCP_CONN_MARKER} {max_conn} */"
    return [
        (
            '#include "esp_task_wdt.h"\n',
            '#include "esp_task_wdt.h"\n'
            f"#define VDM_MAX_CONN {max_conn} {marker}\n"
            "#if !TCP_LISTEN_BACKLOG\n"
            '#error "VDM_MAX_CONN needs TCP_LISTEN_BACKLOG"\n'
            "#endif\n",
        ),
        (
            "    static uint8_t backlog = 5;\n",
            f"    static uint8_t backlog = VDM_MAX_CONN; {marker}\n",
        ),
        (
            "int8_t AsyncServer::_accept(tcp_pcb* pcb, int8_t err){\n"
            '    //ets_printf("+A: 0x%08x\\n", pcb);\n',
            "int8_t AsyncServer::_accept(tcp_pcb* pcb, int8_t err){\n"
            '    //ets_printf("+A: 0x%08x\\n", pcb);\n'
            f"    tcp_backlog_delayed(pcb); {marker}\n",
        ),
    ]


def verify_async_tcp_conn(path: str, max_conn: int) -> None:
    """The cap is the listen backlog and every accepted connection counts against it."""
    with open(path, "r", encoding="utf-8", newline="") as f:
        text = f.read().replace("\r\n", "\n")
    if f"#define VDM_MAX_CONN {max_conn} " not in text or "static uint8_t backlog = VDM_MAX_CONN;" not in text:
        raise RuntimeError(f"{path}: connection cap {max_conn} is not the listen backlog")
    i = text.find("int8_t AsyncServer::_accept(tcp_pcb* pcb, int8_t err){\n")
    body = text[i:text.find("\n}\n", i)] if i >= 0 else ""
    delayed, created = body.find("tcp_backlog_delayed(pcb);"), body.find("new (std::nothrow) AsyncClient(pcb)")
    if delayed < 0 or created < 0 or delayed > created:
        raise RuntimeError(f"{path}: AsyncServer::_accept() does not count the connection against the backlog")


ASYNC_TCP_QUEUE_MARKER = "VDM-PATCH-ASYNC-TCP-QUEUE v1"

# The purge of a client's queued events as shipped: it re-queues every other packet with
# portMAX_DELAY (deadlock on a full queue) and frees a dropped receive without its pbuf.
ASYNC_TCP_PURGE_OLD = (
    "static bool _remove_events_with_arg(void * arg){\n"
    "    lwip_event_packet_t * first_packet = NULL;\n"
    "    lwip_event_packet_t * packet = NULL;\n"
    "\n"
    "    if(!_async_queue){\n"
    "        return false;\n"
    "    }\n"
    "    //figure out which is the first packet so we can keep the order\n"
    "    while(!first_packet){\n"
    "        if(xQueueReceive(_async_queue, &first_packet, 0) != pdPASS){\n"
    "            return false;\n"
    "        }\n"
    "        //discard packet if matching\n"
    "        if((int)first_packet->arg == (int)arg){\n"
    "            free(first_packet);\n"
    "            first_packet = NULL;\n"
    "        //return first packet to the back of the queue\n"
    "        } else if(xQueueSend(_async_queue, &first_packet, portMAX_DELAY) != pdPASS){\n"
    "            return false;\n"
    "        }\n"
    "    }\n"
    "\n"
    "    while(xQueuePeek(_async_queue, &packet, 0) == pdPASS && packet != first_packet){\n"
    "        if(xQueueReceive(_async_queue, &packet, 0) != pdPASS){\n"
    "            return false;\n"
    "        }\n"
    "        if((int)packet->arg == (int)arg){\n"
    "            free(packet);\n"
    "            packet = NULL;\n"
    "        } else if(xQueueSend(_async_queue, &packet, portMAX_DELAY) != pdPASS){\n"
    "            return false;\n"
    "        }\n"
    "    }\n"
    "    return true;\n"
    "}\n"
)


def async_tcp_queue_edits(wait_ms: int) -> list[tuple[str, str]]:
    """No send into the event queue waits without bound; what the full queue refuses is dropped."""
    marker = f"/* {ASYNC_TCP_QUEUE_MARKER} {wait_ms} */"
    return [
        (
            "static inline bool _send_async_event(lwip_event_packet_t ** e){\n"
            "    return _async_queue && xQueueSend(_async_queue, e, portMAX_DELAY) == pdPASS;\n",
            f"#define VDM_ASYNC_QUEUE_WAIT pdMS_TO_TICKS({wait_ms}) {marker}\n"
            "// A full queue never blocks the lwIP thread: the async task may be waiting for it in\n"
            "// tcpip_api_call, and that deadlock trips the task watchdog. The event is dropped.\n"
            "static inline bool _send_async_event(lwip_event_packet_t ** e){\n"
            "    return _async_queue && xQueueSend(_async_queue, e, VDM_ASYNC_QUEUE_WAIT) == pdPASS;\n",
        ),
        (
            "    return _async_queue && xQueueSendToFront(_async_queue, e, portMAX_DELAY) == pdPASS;\n",
            "    return _async_queue && xQueueSendToFront(_async_queue, e, VDM_ASYNC_QUEUE_WAIT) == pdPASS;\n",
        ),
        (
            ASYNC_TCP_PURGE_OLD,
            f"// Frees a packet with what it carries. {marker}\n"
            "static void _vdm_free_event(lwip_event_packet_t * e){\n"
            "    if (e->event == LWIP_TCP_RECV && e->recv.pb) {\n"
            "        pbuf_free(e->recv.pb);\n"
            "    }\n"
            "    free((void*)(e));\n"
            "}\n"
            "\n"
            "// The packets queued now, each once, without waiting for the queue: a packet of arg\n"
            "// is dropped, the others go to the back, one the full queue refuses is dropped too.\n"
            "static bool _remove_events_with_arg(void * arg){\n"
            "    lwip_event_packet_t * packet = NULL;\n"
            "    if(!_async_queue){\n"
            "        return false;\n"
            "    }\n"
            "    for (UBaseType_t n = uxQueueMessagesWaiting(_async_queue); n > 0; --n) {\n"
            "        if (xQueueReceive(_async_queue, &packet, 0) != pdPASS) {\n"
            "            break;\n"
            "        }\n"
            "        if ((int)packet->arg == (int)arg || xQueueSend(_async_queue, &packet, VDM_ASYNC_QUEUE_WAIT) != pdPASS) {\n"
            "            _vdm_free_event(packet);\n"
            "        }\n"
            "    }\n"
            "    return true;\n"
            "}\n",
        ),
        # _tcp_recv: data the full queue refuses stays with lwIP, a FIN (pcb closed above) is not told.
        (
            "        AsyncClient::_s_lwip_fin(e->arg, e->fin.pcb, e->fin.err);\n"
            "    }\n"
            "    if (!_send_async_event(&e)) {\n"
            "        free((void*)(e));\n"
            "    }\n"
            "    return ERR_OK;\n",
            "        AsyncClient::_s_lwip_fin(e->arg, e->fin.pcb, e->fin.err);\n"
            "    }\n"
            "    if (!_send_async_event(&e)) {\n"
            "        free((void*)(e));\n"
            f"        return pb ? ERR_MEM : ERR_OK; {marker}\n"
            "    }\n"
            "    return ERR_OK;\n",
        ),
        # _tcp_clear_events: a purge the full queue refuses runs inline.
        (
            "    e->event = LWIP_TCP_CLEAR;\n"
            "    e->arg = arg;\n"
            "    if (!_prepend_async_event(&e)) {\n"
            "        free((void*)(e));\n"
            "    }\n",
            "    e->event = LWIP_TCP_CLEAR;\n"
            "    e->arg = arg;\n"
            "    if (!_prepend_async_event(&e)) {\n"
            "        free((void*)(e));\n"
            f"        _remove_events_with_arg(arg); {marker}\n"
            "    }\n",
        ),
    ]


def verify_async_tcp_queue(path: str, wait_ms: int) -> None:
    """No send into the event queue waits without bound, and refused data stays with lwIP."""
    with open(path, "r", encoding="utf-8", newline="") as f:
        lines = f.read().replace("\r\n", "\n").split("\n")
    blocking = [i + 1 for i, line in enumerate(lines) if "xQueueSend" in line and "portMAX_DELAY" in line]
    if blocking:
        raise RuntimeError(f"{path}: unbounded send into the event queue at line(s) {blocking}")
    text = "\n".join(lines)
    if f"#define VDM_ASYNC_QUEUE_WAIT pdMS_TO_TICKS({wait_ms}) " not in text:
        raise RuntimeError(f"{path}: event queue wait of {wait_ms} ms is not defined")
    if "return pb ? ERR_MEM : ERR_OK;" not in text or "uxQueueMessagesWaiting(_async_queue)" not in text:
        raise RuntimeError(f"{path}: _tcp_recv() or the purge still drops data")


ASYNC_TCP_SLOTS_MARKER = "VDM-PATCH-ASYNC-TCP-SLOTS v1"
# The release _lwip_fin does, guarded, for every way a pcb ends.
ASYNC_TCP_SLOT_RELEASE = (
    "if (_closed_slot != -1) { _closed_slots[_closed_slot] = _closed_index; ++_closed_index; _closed_slot = -1; }"
)

ASYNC_TCP_SLOTS_EDITS = [
    # constructor: with every slot taken _closed_slot stays -1, which indexed before the array
    (
        "        _closed_slots[_closed_slot] = 0;\n"
        "        xSemaphoreGive(_slots_lock);\n",
        f"        if (_closed_slot != -1) {{ _closed_slots[_closed_slot] = 0; }}  // {ASYNC_TCP_SLOTS_MARKER}: no free slot\n"
        "        xSemaphoreGive(_slots_lock);\n",
    ),
    # _lwip_fin: the same write, guarded
    (
        "    _closed_slots[_closed_slot] = _closed_index;\n"
        "    ++ _closed_index;\n"
        "    _pcb = NULL;\n"
        "    return ERR_OK;\n",
        f"    {ASYNC_TCP_SLOT_RELEASE}  // {ASYNC_TCP_SLOTS_MARKER}\n"
        "    _pcb = NULL;\n"
        "    return ERR_OK;\n",
    ),
    # abort(), _close(), _error(): the slot was kept for ever
    (
        "        _tcp_abort(_pcb, _closed_slot );\n"
        "        _pcb = NULL;\n",
        "        _tcp_abort(_pcb, _closed_slot );\n"
        "        _pcb = NULL;\n"
        f"        {ASYNC_TCP_SLOT_RELEASE}  // {ASYNC_TCP_SLOTS_MARKER}\n",
    ),
    (
        "        err = _tcp_close(_pcb, _closed_slot);\n"
        "        if(err != ERR_OK) {\n"
        "            err = abort();\n"
        "        }\n"
        "        _pcb = NULL;\n",
        "        err = _tcp_close(_pcb, _closed_slot);\n"
        "        if(err != ERR_OK) {\n"
        "            err = abort();\n"
        "        }\n"
        "        _pcb = NULL;\n"
        f"        {ASYNC_TCP_SLOT_RELEASE}  // {ASYNC_TCP_SLOTS_MARKER}\n",
    ),
    (
        "void AsyncClient::_error(int8_t err) {\n",
        "void AsyncClient::_error(int8_t err) {\n"
        f"    {ASYNC_TCP_SLOT_RELEASE}  // {ASYNC_TCP_SLOTS_MARKER}\n",
    ),
]


def verify_async_tcp_slots(path: str) -> None:
    """A pcb that ends by FIN, abort, close or error gives its slot back; -1 is never an index."""
    with open(path, "r", encoding="utf-8", newline="") as f:
        text = f.read().replace("\r\n", "\n")
    if text.count(ASYNC_TCP_SLOT_RELEASE) != 4:
        raise RuntimeError(f"{path}: closed slot released {text.count(ASYNC_TCP_SLOT_RELEASE)} times, expected 4")
    if re.search(r"^\s*_closed_slots\[_closed_slot\] = ", text, re.M):
        raise RuntimeError(f"{path}: _closed_slots[_closed_slot] written without the -1 check")


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
    conn_marker = f"{ASYNC_TCP_CONN_MARKER} {ASYNC_TCP_MAX_CONN} "
    if ASYNC_TCP_CONN_MARKER in text and conn_marker not in text:
        raise RuntimeError(f"{path}: patched with another connection cap; delete .pio/libdeps")
    queue_marker = f"{ASYNC_TCP_QUEUE_MARKER} {ASYNC_TCP_QUEUE_WAIT_MS} "
    if ASYNC_TCP_QUEUE_MARKER in text and queue_marker not in text:
        raise RuntimeError(f"{path}: patched with another queue wait; delete .pio/libdeps")
    patched = [
        patch_file(path, async_tcp_edits(stack), marker),
        patch_file(path, ASYNC_TCP_ALLOC_EDITS, ASYNC_TCP_ALLOC_MARKER),
        patch_file(path, ASYNC_TCP_ARG_EDITS, ASYNC_TCP_ARG_MARKER),
        patch_file(path, async_tcp_conn_edits(ASYNC_TCP_MAX_CONN), conn_marker),
        patch_file(path, async_tcp_queue_edits(ASYNC_TCP_QUEUE_WAIT_MS), queue_marker),
        patch_file(path, ASYNC_TCP_SLOTS_EDITS, ASYNC_TCP_SLOTS_MARKER),
    ]
    verify_async_tcp_alloc(path)
    verify_async_tcp_arg(path)
    verify_async_tcp_conn(path, ASYNC_TCP_MAX_CONN)
    verify_async_tcp_queue(path, ASYNC_TCP_QUEUE_WAIT_MS)
    verify_async_tcp_slots(path)
    if any(patched):
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
