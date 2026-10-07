# VdMot Revamped in Rust: changes from C++ 2.1.7

Every externally visible difference of the Rust firmware from VdMot Revamped C++ 2.1.7, newest
release first. Each entry links to its source of truth. Everything not listed behaves as in
2.1.7: the HTTP API ([docs/revamped/API.md](../revamped/API.md)), MQTT and Home Assistant
([docs/revamped/MQTT.md](../revamped/MQTT.md)), the UART protocol
([software_stm32/PROTOCOL_V2.md](../../software_stm32/PROTOCOL_V2.md)), the settings and files in
NVS and LittleFS, the STM EEPROM and the STM warm state, in both directions: the C++ and the Rust
firmware read what the other wrote ([PORTING.md](PORTING.md#contract-parity)).

## 2.2.0-revamped

The first Rust release, ESP32 and STM32. How to install it and how to go back:
[INSTALL.md](INSTALL.md).

### Version

- Both MCUs report `2.2.0-revamped` (decision D8): the dashboard header, `/api/status`,
  `/api/health`, the `boot` event (`fw 2.2.0-revamped`), the Home Assistant device, `gvers`
  (`gvers 2.2.0-revamped_C2 1 `) and the banner of the STM debug terminal
  ([GLUE-DESIGN-STM.md §6.2](GLUE-DESIGN-STM.md#62-markers-and-version)).

### ESP32: firmware updates and the boot guard

- A new ESP image runs on trial with the previous image as its fallback: the boot guard does the
  rollback the controllers' bootloader cannot do (C++ 2.1.7 left it to the bootloader, so on
  these controllers no image ran a trial). It is confirmed after 120 s of health (event
  `app_marked_valid`). The boot guard
  switches back to the previous image on the 4th unconfirmed boot (a start-up that hangs for
  60 s is restarted and counts as a boot) and after 15 min without 120 s of health (restart
  reason 4). `/api/health` shows `ota` during the trial
  ([GLUE-DESIGN-ESP.md §6](GLUE-DESIGN-ESP.md#6-boot-guard)).
- An ESP upload during the trial is refused with `409 upload_failed` "image on trial"
  ([GLUE-DESIGN-ESP.md §4.7](GLUE-DESIGN-ESP.md#47-behaviour-that-changes), row 10).
- New route `POST /api/system/ota/switch-back` with `{"confirm":"switch-back"}`: `202
  {"result":"restarting"}`, then a restart into the image of the other slot, in any state;
  `400 confirm_required`, `409 busy` (upload or STM flash), `409 restarting`, `409 no_fallback`.
  The C++ firmware answers 404 there
  ([GLUE-DESIGN-ESP.md §6.3](GLUE-DESIGN-ESP.md#63-at-run-time), [§7](GLUE-DESIGN-ESP.md#7-open-risks-and-decisions) item 6).
- Events: restart reason 7 "switch back" (`reboot_requested` arg1 7, Info; C++ 2.1.7 names it
  "unknown"); `esp_ota_failed` arg1 -4 when the image before failed its trial (arg2 1 boot
  limit, 2 health). `esp_ota_failed` arg1 -3 (no switch possible, the image runs on as
  confirmed) also comes when the other slot holds the image that failed the last trial
  ([PORT-NOTES.md](PORT-NOTES.md#event_log), event_log;
  [GLUE-DESIGN-ESP.md §6.2](GLUE-DESIGN-ESP.md#62-decision-at-boot)).
- A factory reset keeps the boot guard's records (`otaOk`, `otaTrial`) besides the factory latch
  ([GLUE-DESIGN-ESP.md §6.5](GLUE-DESIGN-ESP.md#65-interactions)).
- An update between C++ and Rust loses the RTC records (desired targets, ESP lease emulation, HA
  status, network watchdog count): the targets come from NVS, which every restart writes first
  ([GLUE-DESIGN-ESP.md §6.1](GLUE-DESIGN-ESP.md#61-records)).

### ESP32: firmware upload errors (C++ bugs fixed on purpose)

- An upload that fails never changes the boot image. In C++ an image left complete by an earlier
  upload of the same boot that had failed its MD5 check became the boot image after a small
  wrong file. Such uploads now end with event `esp_ota_failed` arg1 3 ("Flash Read Failed")
  where C++ selected the slot (no error) or failed to activate it (9)
  ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations), ota/update).
- Erase failures report "Flash Write Failed" (1), never "Flash Erase Failed" (2)
  ([PORT-NOTES.md](PORT-NOTES.md#glue), ota/update).
- Multipart uploads: a body without the final CRLF or with an epilogue is answered; a malformed
  one no longer crashes the controller (the C++ library wrote through a freed buffer) and is
  refused with `400 upload_failed` "incomplete file" or "no file in request"; boundaries of 1 to
  70 bytes, part headers, field names, values and file names have limits
  ([PORT-NOTES.md](PORT-NOTES.md#glue), http_parse).

### ESP32: HTTP server

The server is ESP-IDF's `esp_http_server` instead of AsyncWebServer
([GLUE-DESIGN-ESP.md §4.7](GLUE-DESIGN-ESP.md#47-behaviour-that-changes), rows 1 to 9):

- One request at a time: an upload, a log download or a slow client (up to 5 s per wait) delays
  the other requests, which wait in the backlog instead of being refused. The `503 busy`
  "response buffers in use", `409 busy` for a second JSON body and `503 retry` "state changed"
  answers do not occur.
- HTTP/1.1 keep-alive; a fifth client closes the least recently used connection.
- A header block above 1024 bytes gets `431`, a URI above 512 bytes `414`, an unknown method
  `400`, each with the server's own text/html body.
- Header values are taken without leading blanks.
- A body that stalls for 3 receive timeouts of 5 s in a row is dropped without an answer (an
  upload is aborted).
- A list request (valves, sensors, events, files) without heap for its list is answered `503
  busy` "out of memory"; C++ answered it from a kept buffer
  ([PORT-NOTES.md](PORT-NOTES.md#glue), web_server).

### ESP32: health, memory, timing

- `/api/health` lists the task `httpd` in place of `async_tcp`; `arduino_events` is gone. The
  task stacks and the free heap differ from 2.1.7
  ([GLUE-DESIGN-ESP.md §2.1](GLUE-DESIGN-ESP.md#21-threads-d3), [§2.3](GLUE-DESIGN-ESP.md#23-ram-budget-no-psram-263-kb-of-8-bit-heap)).
- The time of the last SNTP sync and the one WiFi retry after the first unrequested disconnect
  come up to 1 s later ([PORT-NOTES.md](PORT-NOTES.md#net), net).

### ESP32: MQTT

The MQTT client is an own implementation with the PubSubClient 2.8 behaviour
([PORT-NOTES.md](PORT-NOTES.md#glue), mqtt_conn and mqtt_client):

- A PUBLISH whose topic length runs past the packet is not delivered (C++ read and wrote past
  the packet).
- A failed write of the HA discovery list (`/HADiscovery.cfg`, file system full) keeps the old
  list; C++ could rename a truncated list over it and then never prune the entities of the lost
  lines.
- A volt value of magnitude 2,147,483.648 or more is compared on change with a saturated value
  (undefined in C++).

### ESP32: files

- LittleFS names that are not UTF-8 are not listed or opened ([PORT-NOTES.md](PORT-NOTES.md),
  storage).

### STM32: boot, clocks, faults

Details in [GLUE-DESIGN-STM.md §5](GLUE-DESIGN-STM.md#5-boot-and-flashing-safety) and the
decisions of [§8](GLUE-DESIGN-STM.md#decisions-for-the-operator):

- The boot window runs on the 25 MHz crystal when it starts within 5 ms, else on the internal
  16 MHz oscillator (D1). The C++ image hangs before its window when the crystal does not start,
  and cannot be flashed then.
- Without the crystal the application runs on the internal oscillator at the same clock
  frequencies (D5); the C++ image hangs.
- A fault or a panic switches the valve outputs and the valve PSU off, records the fault and
  ends in a watchdog reset (D4). The C++ image loops in its fault handler: until the watchdog
  resets it, or until an NRST when the fault comes before the watchdog runs (the first part of
  the start-up). The reset counts as a watchdog reset, as in C++, so the safe mode is unchanged.
  The debug terminal (USART6) prints `last fault: …` after its banner at the next start
  ([§5.5](GLUE-DESIGN-STM.md#55-faults)).
- A stack overflow ends in a fault (an MPU guard), not in silent corruption of the no-init
  cells.
- The independent watchdog starts at the head of the application stage, before the clock
  set-up, instead of at the head of the C++ `setup_system`
  ([§5.4](GLUE-DESIGN-STM.md#54-watchdog)).

### STM32 flashing by the Rust ESP32 (D9)

- An STM image above 16 KiB is flashed in two passes: sectors 1 and up are erased, written and
  verified first, then sector 0. The phases erasing, writing and verifying appear twice, the
  byte count runs over both passes, and the percentage holds at the value of the first
  verification while sector 0 is erased and written. An erase failure of the first pass reports
  the address 0x08004000 ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations), stm_flasher).
- An interrupted flash leaves the STM without a bootable vector table only during the sector-0
  pass (about 2 s) instead of the whole flash; an STM that runs a Rust image still answers the
  next flash after an interruption in the first pass
  ([GLUE-DESIGN-STM.md §5.10](GLUE-DESIGN-STM.md#510-what-the-image-cannot-cover), [§8](GLUE-DESIGN-STM.md#decisions-for-the-operator) D9).

### C++ 2.1.7 behaviour kept on purpose

Kept for parity although it looks odd ([PORT-NOTES.md](PORT-NOTES.md),
[PORT-NOTES-STM.md](PORT-NOTES-STM.md)); the ones a user can meet:

- A host name of digits only (a station named `1234`, or such a `web.allowedHosts` entry) is
  taken for an IPv4 address and refused (`403 host_not_allowed`): reach such a device by its IP
  ([PORT-NOTES.md](PORT-NOTES.md#kept-quirks), web_guard).
- After an STM reboot, inactive valves are polled every 500 ms instead of every 30 s until they
  are activated or the ESP restarts
  ([PORT-NOTES.md](PORT-NOTES.md#valve_model), valve_model).
- `DELETE /api/stm/images/<name>` right after the upload of that image can answer `500` while
  the image scan reads it; a retry succeeds ([PORT-NOTES.md](PORT-NOTES.md), storage).
- `GET /api/log` during the 10 s back-off after a failed log write streams the files without the
  newest events ([PORT-NOTES.md](PORT-NOTES.md#glue), logger).
- JSON bodies other than the config are read as ArduinoJson 6.21.6 reads them: a repeated key
  reuses the first member, and a later `null` keeps its value
  ([PORT-NOTES.md](PORT-NOTES.md#glue), json_body).
- Event messages keep their wording, e.g. "1 events lost" ([PORT-NOTES.md](PORT-NOTES.md#event_log)).
