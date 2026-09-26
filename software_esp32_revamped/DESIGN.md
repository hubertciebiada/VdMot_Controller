# VdMot Revamped ESP32 firmware: design (binding)

Status: implemented, version 2.1.0-revamped. Every public header in
`lib/core/include/vdm/` is the API contract; this file holds the numbers,
tables and rules that the modules follow. The user-facing tables live in
`docs/revamped/`: MQTT topics and HA entities in MQTT.md, HTTP routes and
documents in API.md, installation, failsafe and recovery in INSTALL.md; the
STM protocol in `software_stm32/PROTOCOL_V2.md`. Where this file and a header
disagree, fix both in the same commit.

Version: `2.1.0-revamped` (release env) and `2.1.0-revamped-dev` (dev env),
from the `VDM_VERSION` build flag via `vdm::firmwareVersion()`. The string
appears in `/api/status`, `/api/health`, the dashboard footer, the HA
`device.sw_version` and the `Boot` event.

Contents
1. Build, layout, rules for implementers
2. Module map
3. Task model
4. Data flow
5. STM link policy
6. Target delivery, failsafe and valve health
7. Config schema
8. Legacy import
9. Persistence and memory
10. MQTT
11. HA discovery
12. HTTP API
13. Event codes
14. Calibration schedule
15. STM flasher
16. OTA, restarts and network
17. Tests and mutation

---

## 1. Build, layout, rules for implementers

```
software_esp32_revamped/
  platformio.ini            envs wt32-eth01_revamped (release), wt32-eth01_revamped_dev
  lib/core/                 hardware-free C++17 logic (vdm_core), host-tested
    include/vdm/*.h         public APIs (this contract)
    src/*.cpp               implementations
  src/                      Arduino glue (thin): app, stm_link, stm_service, mqtt_client,
                            web_server, net, storage, logger, ota, board.h, main.cpp
  src/generated/            web_assets.h, generated on every pio build (git-ignored)
  web/                      dashboard sources (vanilla HTML/CSS/JS)
  tools/gen_web_assets.py   gzip web/* -> src/generated/web_assets.h (PlatformIO pre-script)
  tools/mock_api.py         dashboard preview with a simulated API (API.md)
  test/native/              CMake + doctest: test_*.cpp (core), glue/ (glue suites)
```

Toolchain (pinned, never `^` ranges): PlatformIO core 6.1.19 in CI,
`espressif32@6.1.0` (Arduino-ESP32 2.0.7, ESP-IDF 4.4, GCC 8.4),
ArduinoJson 6.21.6, PubSubClient 2.8, AsyncWebServer_WT32_ETH01 1.6.2,
AsyncTCP 1.1.1. C++17 (`-std=gnu++17`).

Partition table: board default, identical to the legacy build (nvs 0x9000
20 KB, otadata 0xE000, app0 0x10000 1280 KB, app1 0x150000 1280 KB, spiffs
0x290000 1472 KB used as LittleFS). Never set `board_build.partitions`: the
upgrade from the legacy firmware is an OTA, and OTA cannot change the table.
The app image must stay below 1.2 MB (`tools/check_image_size.py`, CI), so
every module has to watch its flash cost (no iostream, no `std::string`, no
`printf` of floats outside formatters).

Commands (repo root):
```
bash tools/native/docker.sh test esp32                     # core + glue suites
bash tools/native/docker.sh mutate esp32 --files lib/core/src/<file>.cpp
cd software_esp32_revamped && pio run -e wt32-eth01_revamped
```

Rules for module implementers:
- Keep the public API and the documented semantics. Private members may be
  changed freely. If a contract turns out wrong, change header + this file +
  tests together and say why in the commit.
- Core code: no Arduino/ESP-IDF headers, no heap, no exceptions, no RTTI
  (virtual interfaces are fine), fixed-size storage, every input bounded. Use
  `elapsedMs()`/`timeReached()` for time. Keep outputs NUL-terminated.
- Tests: one `test/native/test_<module>.cpp` per module (more files as
  `test_<module>__<part>.cpp`). Cover every public function, boundaries,
  malformed input, and for parsers a fixed-seed random-bytes fuzz loop.
- Native tests are built with `-Wall -Wextra -Werror` plus ASan/UBSan.
- Glue: no decision logic. If a glue function needs an `if` that is more than
  plumbing, the decision goes into a core module first. The glue is tested in
  the glue suites against fakes of the hardware and libraries
  (`tools/native/testkit`, section 17).

## 2. Module map

| Core header | Owns | Used by (glue) |
|---|---|---|
| `common.h` | constants (12 valves, 34 temp slots, 8 volt slots), time helpers, strict parsers, IPv4, 1-Wire id, name policy, `roundTargetPercent`, `buildHaId` | all |
| `version.h` | ESP version, `gvers` parsing, compare, revamped detection, `stmSupport` (below 1.4.0 = TooOld) | stm_link, web |
| `line_assembler.h` | bounded CR/LF line framing of UART bytes | stm_link |
| `stm_codec.h` | request builders, reply parser, reply/request matching, flag/fault/cfg names, chip names | stm_link |
| `stm_types.h` | `StmCommand`, `StmSnapshot`, `StmSaveState` (shared by the tasks) | all |
| `failsafe.h` | failsafe constants, lease/regulator enums and names, `regulatorCause`, `LeaseStatus` | stm_link, mqtt, web |
| `link_policy.h` | request queue, one outstanding request, timeouts/retries, R6 reset decision, reboot detection | stm_link |
| `poll_planner.h` | re-sync sequence and periodic poll cadence | stm_link |
| `valve_model.h` | valve state, target delivery, failsafe drive, health flags, sensor readings | stm_link (writer), others via snapshot |
| `lease_client.h` | K1: lease heartbeat, lease config push, ESP emulation, `LearnTimeSync` is in `calib_schedule.h` | stm_link |
| `target_store.h` | desired-target record (RTC/NVS) and the NVS save policy | stm_link, stm_service |
| `reset_gate.h` | wait for the STM EEPROM before NRST, a flash, an ESP restart | stm_link |
| `stm_session.h` | the whole STM link session behind `StmSessionPort` (owns the modules above) | stm_link |
| `health_monitor.h` | events from model and STM status changes | stm_link |
| `event_log.h` | event registry (code, name, severity, MQTT class, message), ring buffer, text/JSON formatting | logger, web, mqtt |
| `event_limiter.h` | MQTT event rate limit and aggregation | mqtt_client |
| `mqtt_topics.h` | topic tree, subscriptions, inbound topic/payload parsing, payload formatters, publish scheduler | mqtt_client |
| `mqtt_policy.h` | inbound decisions, retained clearing, button gate, reject log, reconnect pacer, client id, HA status watch | mqtt_client |
| `mqtt_values.h` | derived MQTT values (`systemState`, published target, problem flag, sensor segments, calibration end) | mqtt_client |
| `ha_discovery.h` | entity table, discovery payloads, list-file classification, discovery run | mqtt_client |
| `config.h` | config schema, defaults, validation, repair, key-path setter, JSON export, NVS base and ext encoding, naming helpers, restart rules | storage, web, all readers |
| `legacy_import.h` | legacy NVS -> Config mapping, import report | storage |
| `calib_schedule.h` | scheduled calibration decision, confirmation and retries; STM learn-time sync | stm_service, stm_link |
| `stm_flasher.h` | AN3155 flasher state machine, image validation, board check | stm_link |
| `image_store.h` | STM image names and index | storage, web |
| `json_writer.h` | bounded JSON writer | json_api, event_log, config, ha_discovery, mqtt |
| `json_api.h` | `/api/*` documents, API router with auth class, health document | web_server |
| `web_guard.h` | request guard (Host, Origin, X-VdMot, Content-Type) | web_server |
| `legacy_http.h` | legacy aliases and the 410 table | web_server |
| `auth.h` | Basic-auth check, constant-time compare, per-address lockout | web_server |
| `file_manager.h` | LittleFS file list and delete rules | storage, web |
| `ota_policy.h` | OTA image validation (`OtaValidator`), MD5 argument | ota |
| `restart_gate.h` | restart after the STM EEPROM wait (guard) | ota |
| `net_policy.h` | network reachability, network watchdog, time sync step | net |
| `net_trial.h` | network trial record and state machine | net |
| `log_sink.h` | log flush policy, file rotation step, gap lines, syslog format | logger |
| `sys_health.h` | heap and stack alarms, abnormal reset reasons | app |
| `factory_reset.h` | GPIO2 decision and latch | app |

| Glue module | Owns |
|---|---|
| `main.cpp` | `setup()` -> `app::setup()`, `loop()` deletes itself, `verifyRollbackLater()` |
| `app` | boot sequence, task creation, TWDT, command queue, STM snapshot, health data, app task (config apply, net/web start, OTA validation, stm_service, log flush, deferred restart, resource alarms, factory latch) |
| `stm_link` | Serial2, NRST; runs `StmSession` and implements its port |
| `stm_service` | app-task side of the link: scheduled calibration, desired-target NVS saver, `flushForRestart()` |
| `mqtt_client` | PubSubClient, LWT, publishing, discovery, inbound commands, regulator state |
| `web_server` | AsyncWebServer, guard, assets, API, legacy aliases, uploads |
| `net` | ETH/WiFi, SNTP/TZ, mDNS, reachability, network watchdog, network trial |
| `storage` | NVS `vdmrev`, legacy NVS reader, LittleFS, config load with backup and repair, files |
| `logger` | event ring, serial mirror, file flush and rotation, syslog |
| `ota` | ESP OTA upload, rollback validation, restart sequence |

## 3. Task model

| Task | Created by | Core | Prio | Stack | TWDT | Loop | Runs |
|---|---|---|---|---|---|---|---|
| `stm` | app::setup | 1 | 5 | 6144 B | yes | 2 ms (`vTaskDelay(2)`) | commands in, UART RX, parse, link policy, target delivery, planner, lease client, reset gate, UART TX, flasher, health events, snapshot publish, RTC copies of targets and lease |
| `app` | app::setup | 1 | 3 | 8192 B | yes | 100 ms | config apply, every 1 s: net service + web start, OTA validation, factory latch, `stm_service::service` (calibration schedule, target saver); every 10 s: heap and stack alarms; log flush, storage service, restart sequence |
| `mqtt` | app::setup | 1 | 2 | 8192 B | yes | 20 ms connected, 100 ms connecting, 500 ms off | connect/back-off, `loop()`, publishing, discovery (one message per pass), events, inbound commands |
| `async_tcp` | AsyncTCP lib | 0 | lib default (3) | lib default | lib WDT (`CONFIG_ASYNC_TCP_USE_WDT=1`) | event driven | all HTTP handlers |
| sys event | ESP-IDF | 0 | IDF | IDF | no | event driven | `net` WiFi/ETH event callback (only sets flags) |
| `loopTask` | Arduino | 1 | 1 | - | - | - | runs `setup()`, then deletes itself |

- Task watchdog: `esp_task_wdt_init(30 s, panic=true)`; `stm`, `app` and
  `mqtt` subscribe and reset it every loop, `mqtt` also after every publish.
  A missed feed ends in a panic reboot with reason TASK_WDT, reported by the
  next `Boot` event (Warning).
- Blocking limits: `stm` never blocks except the 100 ms NRST pulse; `mqtt`
  may block in `connect()` for at most 3 s TCP + 5 s CONNACK (< 30 s TWDT);
  `app` blocks only for LittleFS writes (bounded per pass) and the factory
  pin check at boot (5 s only while the pin is LOW); HTTP handlers never
  block.
- Ownership: only `stm` touches Serial2 and NRST. Only `mqtt` touches the
  PubSubClient. NVS is accessed only through `storage` (mutex).
- Cross-task data:
  - `app::submit(Command)`: FreeRTOS queue, depth 16, non-blocking; full ->
    HTTP 503 / MQTT reject (the MQTT task keeps the latest target per valve
    and submits it again).
  - `app::StmSnapshot`: written by `stm` at most every 100 ms and only when
    something changed; readers copy it under a mutex into their own static
    buffer (`web`, `mqtt`).
  - Config: `storage::getConfig()` copy under mutex; `configRevision()`
    tells readers when to reload. `storage::applyConfig()` validates,
    persists and bumps the revision.
  - Events: `logger::log()` from any task (mutex); readers use cursors
    (`readSince`).
  - `mqtt::regulatorState()` (read by `stm` once per second),
    `app::requestStmSave()`/`stmSaveState()` (restart sequence, section 16),
    desired targets and scheduled-calibration results between `stm` and
    `stm_service` (portMUX variables inside the link glue).
- Boot order (`app::setup`): Serial -> `stm_link::begin()` (NRST released,
  UART 8N1, never a reset: R6) -> queue/mutex -> logger -> factory pin ->
  LittleFS -> config load (backup, repair, import) -> `Boot` event -> logger
  sinks -> `stm_service::begin()` (boot targets: RTC, else NVS) -> net -> OTA
  state -> MQTT -> TWDT -> tasks. The HTTP server starts in the app task once
  the network has an IP.

## 4. Data flow

```
 MQTT target ─┐                     ┌────────────── stm task ───────────────┐
 HTTP actions ├─ app::submit ──────►│ Command -> ValveModel / LinkPolicy     │
 schedule ────┘                     │ LinkPolicy.nextToSend -> Serial2 TX    │
 regulator state ──────────────────►│ LeaseClient (slhbt, emulation)         │
                                    │ Serial2 RX -> LineAssembler -> parse   │
                                    │   -> LinkPolicy.onReply -> models      │
                                    │ PollPlanner / ValveModel pushes        │
                                    │ HealthMonitor -> logger::log(Event)    │
                                    └──── publishStmSnapshot ───────────────┘
                                                  │
                    ┌─────────────────────────────┼──────────────────────┐
              mqtt task (copy)             web handlers (copy)       app task
              compat + new topics          /api/* JSON               OTA validator (link state)
              events (limited)             dashboard                 stm_service (schedule, NVS)
```

Rules (R7): targets flow only from MQTT and HTTP. The ESP never invents a
target, with two exceptions: the failsafe emulation pushes the failsafe
position instead of the desired target (the desired target is kept), and
restored targets (RTC/NVS) are pushed after an ESP restart. Boot rule:
restore, else adopt: a valve with a restored target gets it back (source
`restored`, pushed only after the first read-back differs); a valve without
one adopts the STM's current target (`gtgtp`/`gvlvx`/`gvlvy`, source `stm`).
After an STM reboot the ESP pushes every desired target once, also when the
read-back equals it (`forcePush`).

## 5. STM link policy

Physical: Serial2, RX IO5, TX IO17, 115200 8N1 (8E1 only while flashing), RX
buffer 2048, TX buffer 512. NRST on IO15 through jumper X20, HIGH = reset
asserted. The ESP never resets the STM at its own boot (R6).

Request format: `cmd arg ... ` with a space after every token, then CR LF.
Line limit 63 chars. Replies up to 1023 chars (`kStmMaxLineLen`).

`LinkParams` defaults (binding):

| Parameter | Value | Notes |
|---|---|---|
| Normal timeout | 400 ms | from `onSent()` |
| Long timeout | 1500 ms | `gonec 255`, `gowvc 255`, `gvlon 255`, `gprof` |
| Slow timeout | 3000 ms | `stons`, `masns`, `stdet`, `reset`, `smotc`, `stvls` |
| Retries | 2 | idempotent commands only (`cmdIsIdempotent`) |
| Inter-request gap | 5 ms | after reply or timeout |
| Boot hold-off | 5000 ms | after an ESP-initiated NRST pulse, flashing, or the ESP's own boot |
| Down after | 5 consecutive timed-out attempts | probes never count |
| Queue | 24 entries | User > Config > Poll, FIFO within a priority |

Coalescing: a new `stgtp` for a valve replaces the queued one (latest
target wins). Any other byte-identical line is merged. Full queue: the newest
Poll entry is evicted for User/Config; Poll never evicts.

Priorities: User = HTTP/MQTT actions (calibrate, assembly, detect, scan,
sensor mapping, motor parameters, breakaway, service move, stop, safe mode).
Config = target pushes, the re-sync sequence, lease and learn-time requests.
Poll = periodic reads and read-backs.

Reply matching (`replyMatches`): same command, and for `gvlvd`, `gvlvx`,
`gvlvy`, `gtgtp`, `gprof`, `svmov`, `sfspo`, `sstop` and `stvls` the same
valve; `goned`/`gowvd` replies must carry the requested sensor id (a late
reply for another sensor cannot complete the request). `gvlon` also accepts
`goned error`. `goned 0`/`gowvd 0` complete as Rejected. `svmov -1 err 1`
parses. A valid reply that matches nothing is stray: it is counted, and
self-identifying data (`gvlvd`, `gvlvx`, `gvlvy`, `gtgtp`, `gprof`, and sensor
data by id) is still applied. Parse errors are counted and never count as
proof of life. Protocol 3 replies are parsed leniently (at least the
documented count, numeric extras ignored); `gvlvx` and `gstat` stay exact.

STM reset (R6), `shouldResetStm()` true only when all of these hold:
- at least 5 consecutive timed-out attempts,
- the first of them at least 60 s ago,
- no policy reset in the last 10 min,
- the link is not Booting or Suspended.
After the pulse: `StmResetByPolicy` (Error), the outstanding request and all
Poll entries are dropped, User/Config entries stay queued, 5 s hold-off, full
re-sync, and all desired targets are re-pushed. Policy resets are not gated;
a user reset and a flash start wait for the STM EEPROM (`ResetGate`: `eepst`
every 500 ms until `eepst 1`, at most 10 s, then event 323; link not up ->
at once).

Reboot detection (`RebootDetector`): on protocol 2/3 when the `gstat`/`gstax`
uptime decreased, the reset counter changed, or the uptime is below what the
STM must have reached by now (expected-uptime rule: a power-on resets the
counter to 0); on protocol 1 when a valve that had `oc` or `cc` > 0 reports
`oc = cc = moves = 0` with status 5, 6 or 8. A link Down -> Up counts as a
reboot only on protocol 1 or when the status check after the recovery failed.
The result is `StmRebootDetected` (Warning), re-sync, and every desired
target pushed once.

Feature detection: every re-sync starts with `gproto` and `gvers`, each sent
alone (nothing else goes out before the version is known). `gproto` is a
probe: normal retries, timeouts never count towards Down. `gproto 2`/`3`
selects that protocol (higher counts as 3), a timeout selects 1. Because the
IO15 strap pull-up can hold NRST while the ESP boots, the link starts in
Booting with the normal 5 s hold-off (`holdAfterEspBoot`, no reset counted).
When `gvers` (re-read every 5 min) reports a revamped version while the link
runs in protocol 1, one more `gproto` probe runs; an answer restarts the
re-sync. Commands are never sent to an STM that does not support them
(`cmdMinProtocol`).

Minimum STM version 1.4.0 (`stmSupport`): a TooOld STM ends the re-sync;
the planner then hands out only `gvers` every 30 s, `gtgtp` read-backs and
`stgtp` pushes; the protocol-1 failsafe emulation stays active. Every other
command is dropped and HTTP answers `409 stm_unsupported`. A later `gvers` of
a supported version starts a re-sync. `StmIncompatible` and a dashboard
banner report it.

### Poll cadence (`PollCadence`, binding)

| Item | Period | Request |
|---|---|---|
| Busy valve (moving, calibrating, sync not settled) | 500 ms | `gvlvd` (v1) / `gvlvx` (v2) / `gvlvy` (v3) |
| Active valve, idle | 2 s | same |
| Inactive valve | 30 s | same |
| Each DS18 bus index | 10 s | `goned i` (one index per 10 s / count) |
| Each DS2438 bus index | 10 s | `gowvd i` |
| Sensor counts | 30 s | `gonec`, `gowvc` (list re-read on change) |
| STM health (v2/v3) | 10 s | `gstat` / `gstax` |
| Version | 5 min (TooOld: 30 s, the only poll) | `gvers` |
| Lease heartbeat (v3) | 60 s and at once on a regulator change | `slhbt` |
| Lease config check (v3) | 10 min while synced, 60 s after a failure | `glcfg`, then `slcfg`/`sfspo` |

`gvlvx`/`gvlvy` carry no temperatures, so on v2/v3 a valve's temp1/temp2 come
from its `gvlon` assignment and the `goned` readings
(`ValveModel::applySensorTemps`, once per second): the reading of the
assigned sensor when it is on the bus and was read within 60 s; one failed
read is bridged by the previous value (debounce 2); an STM temperature older
than 200 s (`gstax` tempAgeS) counts as stale.

Re-sync sequence (`ResyncStep`): `gproto`, `gvers` (both alone), `ghwin`,
`gmotc`, `gtlnm`, `gcalx` (v2+), `gonec 255`, `gowvc 255`, `gvlon 255`,
`gvlst` (v1), then `gtgtp 0..11` (v1), `gvlvx 0..11` (v2) or `gvlvy 0..11`
(v3). After the first two steps it is interleaved with valve polling: at most
every second request is a re-sync step. Failed steps are retried. Triggers:
ESP boot, STM reboot detected, STM reset (policy or user), link Down -> Up,
after flashing, a new version text.

Known v1 quirks the codec and model handle: the trailing comma in `gvlst`;
`gonec 0` means both "count 0" and "empty list"; `gvlon` error has the
`goned` prefix; `gvlon` may report garbage ids for unassigned sensors (resolved
against configured slots, never written back); temperature sentinels -500,
-1270 and 850; `stgtp` is ACKed but dropped while calibrating (delivery is
verified, section 6); after `stons` a v1 STM needs `masns` (sent 5 s after
the `stons` ack, below protocol 2 only).

## 6. Target delivery, failsafe and valve health

Target delivery (`ValveModel`), per valve:
```
Unknown --gtgtp/gvlvx/gvlvy--> Synced (desired := STM target, source stm)
Synced --setDesiredTarget(new)--> Pending
Pending --nextTargetPush (active, known, !calibrating on v1, >= 2 s since last push)--> AwaitAck
AwaitAck --stgtp could not be queued (queue full)--> Pending (attempt not counted)
AwaitAck --stgtp ack--> AwaitVerify --read-back equal--> Synced
                                     --read-back differs--> Pending
AwaitAck --timeout--> Pending; after 5 pushes -> Failed (TargetNotConfirmed, kHealthTargetUnconfirmed)
Failed --5 min--> Pending
any --STM reboot/reset/flash--> Pending + forcePush (desired kept) or Unknown (no desired)
restoreDesired (ESP boot) --> Pending, no push before the first read-back (equal -> Synced)
```
Read-back: `gtgtp` on v1, `gvlvx`/`gvlvy` on v2/v3 (they carry the target).
Because STM 1.x drops `stgtp` during calibration, on v1 (and while the
protocol is not known yet) a Pending push waits while `calibrating` is true,
and read-back mismatches are retried after the calibration ends. STM 2.x
takes a target during a calibration, so on v2/v3 targets are pushed at once.

Target sources: `none`, `stm` (adopted), `web`, `mqtt`, `restored` (RTC/NVS
copy), `assembly`. Assembly (`staop`): desired 100, source Assembly; on
protocol >= 2 it is delivered and restored (after an STM reboot or an ESP
restart) with `staop`, never with `stgtp 100`, so the STM keeps its assembly
hold; the next target ends it. Inactive valves never get pushes, and
`setDesiredTarget` rejects them (HTTP 409, MQTT reject).

`calibrating`: v1 is `gvlvd` status bit 7. On v2/v3 the bit is set only for
`staln` and the movement trigger, so it is `calState` phase 2 (running) or
bit 7; phase 1 alone (time trigger queued, automatic retry, or a valve that
calibrates on its first target change) is shown as "calibration queued" and
may last for days. `calState` is split into the phase (bits 0..1) and the
flags (bit 2 early end stop since the last good calibration, bit 3 last
calibration failed); a calibration that ends with bit 3 set is CalibFailed.

Failsafe (K1, `LeaseClient`, `vdm/failsafe.h`):
- Regulator: `regulatorCause(mqtt::regulatorState())`: MQTT off -> alive;
  mode 1 -> alive while the broker is connected; mode 2 -> broker connected
  and HA status not Offline (Unknown counts as alive; an accepted MQTT
  command sets Online). The HA status survives software restarts (RTC).
- Protocol 3 (`LeaseMode::Stm`): `slhbt 1` every 60 s and at once when the
  regulator comes back, `slhbt 0` when it is lost; during a failsafe a
  regulator that comes back renews only after staying alive for 120 s, or at
  once with an MQTT command. The config (`failsafe.timeoutMin`,
  `valves.N.failsafePct`, inactive valves 255) is compared with `glcfg` and
  pushed with `slcfg`/`sfspo`; after 3 failed attempts `lease_config_failed`.
  Nothing is pushed while the ESP runs on default config (`configTrusted`
  false). Status: `gstax` lease fields.
- Protocol 1/2 (`LeaseMode::Emulated`, timeout > 0): after the timeout
  without a live regulator, `ValveModel::setFailsafeDrive` makes the push
  target of every active valve with a desired target (not in assembly, pct
  <= 100) its failsafe position (`fsOverride`); desired and source are kept.
  The emulation state survives ESP restarts (RTC `gRtcLease`).
- Events 314-318 (section 13).

Health flags (`HealthFlag`), recomputed on every update:

| Flag | Condition |
|---|---|
| Blocked | status 9 |
| Failed | status 4 |
| NoValve | status 6 and valve active |
| CalibRetries | `calibRetries > 0` |
| EarlyStop | v2 `earlyStops` > value at ESP boot / last STM reboot |
| CmdRejected | v2 `cmdRejected` > baseline |
| Stale | active and no `gvlvd`/`gvlvx`/`gvlvy` for 60 s |
| TargetUnconfirmed | sync Failed |
| TempFailed | an assigned sensor (`temp1`/`temp2` != -500) reports -1270, 850 or out of range |
| Failsafe | the valve is at its failsafe position (`failsafeKind` Lease) |
| StrokeShort | minCounts, oc and cc known and min(oc, cc) < 1.2 × minCounts (event 415) |

`failsafeKind(v)`: Blocked when protocol 3 flags FS_BLOCKED, Lease when
`fsOverride` or FS_LEASE, else None. `HealthMonitor` turns transitions into
events (section 13). Each condition fires once when it starts and once
(`ValveRecovered`, or the calibration outcome) when it ends, so a stuck
condition never floods the log.

`systemState` (MQTT `common/state`): 2 = link Down, STM safe mode, or an
active valve Blocked/Failed; 1 = link Unknown/Degraded/Booting/Suspended, the
failsafe active, or any other health flag on an active valve; 0 = otherwise.

## 7. Config schema

One struct, `vdm::Config` (`config.h`). The key path is what
`setConfigValue` takes and what `writeConfigJson` emits. Arrays are 1-based in
paths (`valves.3.name`) and 0-based JSON arrays in the export. IPv4 values are
dotted strings in JSON. Secrets are write-only: the export has `"<key>Set":
true|false`, and an empty string leaves the secret unchanged unless the
request says to clear secrets (`GET /api/config/export?secrets=1` exports them
when web login is enabled). "Legacy source" is the NVS namespace/key that
`importLegacyConfig` reads. `-` means the key is new.

Storage: the base blob `cfg` has `kConfigBaseSchema` 1 and exactly the 2.0.0
fields, so ESP 2.0.0 still reads it after a downgrade; keys added in 2.1 live
in the ext blob `cfgx` (TLV records, tags never reused, unknown records kept
and written back). JSON `schema` = `kConfigJsonSchema` 2; import accepts 1..2.

| Key | Type | Range / rule | Default | Legacy source |
|---|---|---|---|---|
| `schema` | int | read-only | 2 | - |
| `station` | string | 1..20 bytes, `isSafeName` (UTF-8 and spaces allowed like legacy; DHCP/mDNS/syslog use `buildHostname`) | `VdMot` | `sysCfg/stName` ("" -> `VdMot` + `mqtt.rootTopic` `VdMotFBH`) |
| `net.iface` | int | 0 auto, 1 ethernet, 2 wifi | 0 | `netCfg/ethwifi` |
| `net.dhcp` | bool | | true | `netCfg/dhcp` |
| `net.ip` | IPv4 | non-zero when !dhcp | 0.0.0.0 | `netCfg/staticIp` |
| `net.mask` | IPv4 | contiguous, non-zero when !dhcp | 0.0.0.0 | `netCfg/mask` |
| `net.gateway` | IPv4 | non-zero when !dhcp | 0.0.0.0 | `netCfg/gw` |
| `net.dns` | IPv4 | any; 0 with a static IP = the gateway (`effectiveDns`) | 0.0.0.0 | `netCfg/dnsIp` |
| `net.ssid` | string | 0..32 bytes printable text (ASCII or UTF-8); required when iface = wifi | "" | `netCfg/ssid` |
| `net.wifiPassword` | secret | "" (open network) or 8..63 when ssid set | "" | `netCfg/pwd` |
| `net.reconnectTimeoutMin` | int | 0..240 (0 = watchdog off) | 5 | `netCfg/netConnTO` |
| `time.ntpServer` | host | 0..64, `isHostName` or IPv4; "" disables SNTP | `pool.ntp.org` | `netCfg/timeServer` |
| `time.tzName` | string | 0..49 printable | `Europe/Berlin` | `tZCfg/tZ` |
| `time.tzPosix` | string | 1..49 printable, no spaces | `CET-1CEST,M3.5.0,M10.5.0/3` | `tZCfg/tZCode` |
| `syslog.level` | int | 0 off, 1 warning+, 2 info+, 3 debug | 0 | `netCfg/syslogEnable` (1..3 -> 3) |
| `syslog.server` | IPv4 | non-zero when level > 0 | 0.0.0.0 | `netCfg/sysLogIp` |
| `syslog.port` | int | 1..65535 | 514 | `netCfg/sysLogPort` (0 -> 514) |
| `web.user` | string | 0..64 printable, no ':' | "" | `netCfg/userName` |
| `web.password` | secret | 0..64; user and password both set or both empty | "" | `netCfg/userPwd` |
| `web.protectRead` | bool | | false | - |
| `web.allowedHosts` | string | "" or 1..4 comma-separated host names / IPv4, 0..80 bytes | "" | - |
| `mqtt.mode` | int | 0 off, 1 MQTT, 2 MQTT + HA (needs `separate`, V1: not `germanDecimal`) | 0 | `protCfg/dataProt` |
| `mqtt.host` | host | 0..64, host name or IPv4; required when mode > 0 | "" | `protCfg/brokerIp` (u32 -> dotted, 0 -> "") |
| `mqtt.port` | int | 1..65535 | 1883 | `protCfg/brokerPort` (0 -> 1883) |
| `mqtt.user` | string | 0..64 | "" | `protCfg/brokerUser` |
| `mqtt.password` | secret | 0..64 | "" | `protCfg/brokerPwd` |
| `mqtt.keepAliveS` | int | 5..300 | 60 | `protCfg/brokerKAT` |
| `mqtt.publishIntervalS` | int | 2..3600 | 10 | `protCfg/publishInterval` (clamped) |
| `mqtt.minDelayS` | int | 0..publishIntervalS | 5 | `protCfg/brokerMD` |
| `mqtt.separate` | bool | | true | `protCfg/brokerPF` bit0 |
| `mqtt.allTemps` | bool | | true | bit1 |
| `mqtt.pathAsRoot` | bool | | false | bit2 |
| `mqtt.upTime` | bool | | true | bit3 |
| `mqtt.onChange` | bool | | true | bit4 |
| `mqtt.retained` | bool | | true | bit5 |
| `mqtt.plainText` | bool | | true | bit6 |
| `mqtt.diag` | bool | | true | bit7 (`brokerPF` missing -> 7, like the legacy read fallback) |
| `mqtt.germanDecimal` | bool | | false | `protCfg/brokerMQF` bit2 |
| `mqtt.newDiag` | bool | | true | - |
| `mqtt.events` | bool | | true | - |
| `mqtt.haDiscoveryOnConnect` | bool | | true | - |
| `mqtt.rootTopic` | string | 0..20 `isSafeName`; "" = station | "" | "" station -> `VdMotFBH` |
| `mqtt.clientId` | string | 0..64 `[A-Za-z0-9._-]`; "" = `<host16>-<mac6>` | "" | - |
| `mqtt.discoveryPrefix` | string | 1..32, levels `[A-Za-z0-9_-]+` separated by single `/` | `homeassistant` | - |
| `valves.N.name` (N 1..12) | string | 0..10 `isSafeName`; V2: effective segments unique; V3: HA ids unique | "" | `valvesCfg/valves` blob |
| `valves.N.active` | bool | | false | same blob |
| `valves.N.failsafePct` | int | 0..100, 255 = hold | 50 | - (`brokerMQToPos` only in the import report) |
| `valves.N.topic` | string | 0..10 topic segment override (printable, no space, `+`, `#`; `/` only between parts) | "" | legacy names with `/`, `"` or `\` |
| `temps.N.name` (N 1..34) | string | 0..10 `isSafeName` | "" | `tempsCfg/temps` blob |
| `temps.N.active` | bool | active needs an id | false | same |
| `temps.N.offset` | number, °C | -10.0..10.0, stored as tenths, rounded half away from zero | 0.0 | same (int tenths) |
| `temps.N.id` | 1-Wire id | "" or `hh-hh-hh-hh-hh-hh-hh-hh`; unique across slots | "" | same (`ID[25]`) |
| `temps.N.topic` | string | as `valves.N.topic` | "" | legacy name |
| `volts.N.name` (N 1..8) | string | 0..10 | "" | `voltsCfg/volts` blob |
| `volts.N.active` | bool | active needs an id | false | same |
| `volts.N.offset` | number | finite, -1000..1000 | 0 | same (float) |
| `volts.N.factor` | number | finite, -1000..1000, != 0 | 1 | same (float) |
| `volts.N.unit` | string | 0..8 `isSafeName` | "" | same (`unit[9]`) |
| `volts.N.id` | 1-Wire id | as temps | "" | same |
| `volts.N.topic` | string | as `valves.N.topic` | "" | legacy name |
| `calib.dayMask` | int | 0..127, bit i = weekday i (bit0 Sunday), 0 = off | 9 (Sun + Wed) | `valvesCfg/dayOfCalib` |
| `calib.hour` | int | 0..23 | 0 | `valvesCfg/hourOfCalib` |
| `calib.minute` | int | 0..59 | 0 | - |
| `failsafe.timeoutMin` | int | 0 (off) or 5..1440 | 60 | - (`brokerMQTO` only in the import report) |
| `persistLog` | bool | | true | - |

Validation rules V1-V3 (mode 2 without `germanDecimal`; unique effective
valve segments; unique HA ids of valves, of active temp slots with an id and
of active volt slots with an id) run on save and import. Loading never
rejects: `sanitizeConfig` repairs single fields (repair bits, event
`config_repaired`) so a stored config never falls back to defaults because of
one field.

Not in the ESP config, but held by the STM and edited through `/api/stm/motor`:
motor characteristics (`gmotc`/`smotc`: low/high factor 10..40, startOnPower
0..100, minCounts 0..60000, maxCalibRetries 0..2), learnAfterMovements
(`gtlnm`/`stlnm`: 0 or 50..65534), breakaway (v2 `gcalx`/`scalx`), and the
valve to sensor mapping (`gvlon`/`stvls`; the ESP resolves ids to slots for
display and only writes a mapping the user changed). The STM learn time
(`stlnt`) follows the calibration schedule (section 14).

Dropped legacy keys (never imported): `sysCfg/CF` (°C only),
`protCfg/brokerInterval`, `brokerMQTO`, `brokerMQToPos`, `brokerMQF` bit0 and
bit1, all of `valvesCtrlCfg`, `msgCfg`, `motorCfg`, `valvesCfg/movCalib`.

Apply semantics (`POST /api/config`): the handler reserves its response slot
first (503 `busy`, nothing applied), copies the active config, applies every
key with `setConfigValue` (the first failure aborts with 400 and the key
path), runs `validateConfig` (a failure gives 400 and the path), then
`storage::applyConfig`. The response carries `restartRequired`
(`configRestartReasons != 0`) and `netTrial` (`netTrialRequired`); `?dryRun=1`
answers only these. Live effects: MQTT reconnects (clean session) on
`mqttTopicConfigChanged` (station, MQTT settings, names, active flags,
overrides, ids); logger sinks reconfigure; the stm task reloads the active
mask, slot ids and the failsafe settings (pushed to the STM at once, no MQTT
reconnect). An ESP restart follows after the response only when network
fields in use change (then on trial, section 16) or the station changes the
host name; static fields under DHCP and WiFi fields on Ethernet do not
restart. With jumper X20 fitted the ESP restart also resets the STM (IO15
strap); STM 2.1 keeps positions and targets over that warm reset, older STMs
recalibrate.

## 8. Legacy import

`storage::loadConfig` runs `importLegacyConfig` exactly once: when
`vdmrev/cfg` is missing and `vdmrev/imported` is not 1. The result is saved,
`imported` is set, and the legacy namespaces are never written or erased (a
downgrade still finds them). Per-key rules are in `legacy_import.h`. Blob
sizes must match exactly (valves 144, temps 1496, volts 480 or the 1.4.0
variant 448), otherwise the whole blob is rejected. `Misc/MiscLC` becomes
`vdmrev/lastCal`.

Mapping specials: an empty legacy station gives station `VdMot` and
`mqtt.rootTopic` `VdMotFBH`; a legacy name that is not a valid name (`/`, `"`,
`\`) is imported as a cleaned name plus the original as topic override, so
the MQTT topics and HA unique_ids stay; legacy syslog levels 1..3 become 3;
the failsafe keeps its defaults (60 min / 50 %).

Report: the `ConfigImported` event (imported/rejected counts, first rejected
key); `import_dropped` (117) when legacy features in use were dropped (PI
valves, window logic, messenger, DS18 timeout, legacy MQTT failsafe); the
report file `/sys/import.json` (`GET /api/import-report`, dashboard notice
until dismissed) with counts, dropped features, `legacyFailsafe`, the root
topic, renamed items, `syslogDebug` and `voltsBlob448`.

## 9. Persistence and memory

NVS namespace `vdmrev`:

| Key | Type | Content |
|---|---|---|
| `cfg` | blob <= 4096 | `encodeConfig`: "VDMC", u16 schema 1, u16 length, fields LE, CRC32 (frozen 2.0.0 layout) |
| `cfgx` | blob <= 1536 | `encodeConfigExt`: "VDMX", version 1, u16 length, records `{u8 tag, u8 element, u8 len, bytes}`, CRC32 |
| `imported` | u8 | 1 = legacy import done (or factory reset) |
| `boots` | u32 | boot counter |
| `calSlot` | u32 | last scheduled calibration slot (yyyymmdd), booked at the STM's confirmation |
| `lastCal` | i64 | epoch of the last calibration command |
| `haDrop` | u8 | 1 = legacy DROP discovery entities deleted |
| `haLayout` | u8 | 2 = the 2.1 discovery layout was published (2.0.0 migration done) |
| `targets` | blob 46 | desired targets: "VDTG", version 1, count 12, 12 x {flags bit0 valid, pos, source}, CRC32 |
| `netTrial` | blob <= 136 | network trial record "VDNT" (state, previous interface/DHCP/addresses/WiFi, CRCs) |
| `frLatch` | u8 | 1 = GPIO2 factory reset done, pin not released yet |
| `otaStm` | u8 | 1 = STM link was up at the ESP OTA upload (read and erased at the next boot) |

Config load order (`loadConfigBlobs`): `cfg` + `cfgx` from NVS; a missing or
undecodable `cfg` falls back to the backup files (`config_restored`); a
decodable one is repaired field by field (`config_repaired`); a newer schema
is loaded as far as known and not overwritten automatically
(`config_newer_schema`); only when nothing is usable: defaults
(`ConfigDefaults`, Error), and the next explicit save replaces the blob. The
backups are written after every save, but not while a network trial runs.

RTC slow memory (`RTC_NOINIT_ATTR`, survives software restarts, garbage after
power-on, every record CRC-checked): network watchdog restart counter,
desired targets (same format as NVS `targets`), ESP failsafe emulation
(`gRtcLease`), last HA status (`gRtcHaStatus`).

LittleFS (partition `spiffs`, mounted at `/littlefs`, formatted only when
mounting fails, which raises `FsFormatted`):

| Path | Content | Limit |
|---|---|---|
| `/stm/<name>.bin` | uploaded STM images, name `[A-Za-z0-9._-]{1,31}` | 512 KiB each, at most 3 plus `last_good.bin` |
| `/stm/<name>.bin.part` | upload in progress, renamed when complete | |
| `/log/events.log`, `/log/events.1.log` | event log lines (`formatEventLine`) | 64 KB each + 8 KB slack while a reader blocks the rotation |
| `/sys/cfg.bak`, `/sys/cfgx.bak` (+ `.tmp`) | byte copies of the last saved config blobs, one pair: written as `.tmp` and renamed base first; `cfgx.bak.tmp` without `cfg.bak.tmp` is the ext blob of the renamed base | internal, not deletable |
| `/sys/import.json` | legacy import report | until dismissed |
| `/HADiscovery.cfg` (+ `.tmp`) | list of the discovery topics this device published (legacy format) | kept, never renamed |

Log file: the RAM ring (512 events) is the buffer; the file is written every
5 min, within 10 s after a Warning+ event, and at once when 256 events are
pending or before a restart; Debug events are not written. Lost lines are
recorded as a gap line; failures raise `log_write_failed` (hourly at most).

RAM budget:
- Allocated once at boot and never freed: event ring 512 x 48 B = 24 KB
  (`logger::begin`) and web response slots 2 x 12 KB (`web::begin`).
  Everything else is static or on a task stack.
- Big objects stay off the stacks: `vdm::Reply` (1 KB), `StmSnapshot`,
  `Config`, `DiscoveryContext`.
- No per-operation heap: PubSubClient's buffer is set once (2304 B,
  discovery payloads up to 2047 B). HTTP responses use the slot pool through
  `beginResponse_P`, and the slot is released in `onDisconnect`;
  `/api/health` uses its own 1 KB buffer. POST bodies go into one static
  8 KB buffer. The only per-request allocations are AsyncWebServer's own
  request and response objects, which it frees after each request.
- Alarms (`ResourceMonitor`, every 10 s): `LowHeap` below 30 KB free,
  `HeapFragmented` when the largest free block is below 8 KB (each at most
  once per hour), `StackLow` once per task and boot when a task's minimum
  free stack falls below max(512 B, stack / 8).

## 10. MQTT

Topic tables, payloads, subscriptions, retained handling and broker settings:
`docs/revamped/MQTT.md`. Implementation rules:

- Connection: PubSubClient over WiFiClient (ETH or WiFi), MQTT 3.1.1. Client
  id `mqtt.clientId` or `buildMqttClientId(station, efuse MAC)`
  (`<buildHostname cut to 16>-<mac[3..5] hex>`). `cleanSession=false`, except
  the first connect after `mqttTopicConfigChanged`. Socket timeout 5 s, TCP
  connect 3 s, both independent of keep-alive. `ReconnectPacer`: back-off
  2 s doubling to 60 s, reset only after 60 s connected. LWT `<main>status`
  = `offline` retained; after connecting `online`, subscriptions, then a full
  publish. Every `publish()` result is checked; failures are counted in
  `/api/status`.
- Naming: `<main>` = (`pathAsRoot` ? `/` : "") + `mqttRootTopic(cfg)` + `/`;
  segments `itemSegment()` (override, name with ' ' -> '_', 1-based index);
  unnamed temp/volt slots without override use 1 + STM bus index
  (`sensorTopicSegment`).
- Subscriptions (`buildSubscriptions`, at most 29): the target wildcard
  filters (QoS 1), the same filters spelled out for every valve whose segment
  contains `/`, `<main>cmd/#` (QoS 0), and in mode 2 `homeassistant/status`
  plus `<discoveryPrefix>/status` when different (QoS 1).
- Inbound (`decideInbound`): configured segments first (multi-level), then
  the number 1..12; payloads per `parseTargetPayload` (digits with one `.` or
  `,`, <= 16 bytes, `roundTargetPercent`), `OPEN`/`CLOSE`/`STOP`, buttons
  `PRESS`. Non-empty command messages are cleared with an empty retained
  publish; buttons wait for the broker's echo of that clear
  (`ButtonGate`, 5 s). Rejects go to `RejectLog` (log once per valve and
  reason per 10 s, count all) and event 204. `EchoFilter` drops the ESP's
  own state publications on the non-separate command form. An accepted
  command increments `commandSeq` and, in mode 2, sets HaStatus Online.
- Regulator: `RegulatorWatch` tracks `online`/`offline` of the HA status
  topics (exact payloads); `mqtt::regulatorState()` feeds the lease client.
- Cadence (`PublishScheduler`): periodic mode publishes everything every
  `publishIntervalS`. On-change mode publishes changed items at most every
  `minDelayS` per item and at least every `publishIntervalS`, plus a full
  publish every `publishIntervalS`. Slots: 0 common, 1..12 valves, 13..46
  temps, 47..54 volts, 55 STM diagnostics. A full publish is spread over
  several loop passes, with `loop()` called between valves, so incoming
  commands are never starved. `esp_task_wdt_reset()` after every publish.
- Values: `systemState` for `common/state`, `publishedTarget` for
  `valves/<V>/target` (separate: the STM read-back, nothing while a restored
  target is not synced, the desired target during the emulation),
  `valveProblem`, `CalibEndTracker` for `calibration/date`.
- Events: `eventReachesMqtt(e)` (registry MQTT class), `EventAggregator`
  (valve events of one code within 2 s, 4 slots) and `EventRateLimiter`
  (per key 10 min, normal and error buckets 30/h, critical per key 1 min).

## 11. HA discovery

Entity tables and the user view: `docs/revamped/MQTT.md`. The table in
`ha_discovery.cpp` is the source (`kEntityCount` 309).

- Runs in mode 2; manual runs (`POST /api/mqtt/discovery`) also in mode 1
  (publish/republish need `separate`; delete does not). Triggers: every
  connect when `haDiscoveryOnConnect`, HA status Offline -> Online (not every
  `online`: a retained birth would run it on every connect), manual.
- `DiscoveryRun` phases, one message per MQTT loop pass, 20 ms apart,
  `loop()` in between: (1) first run on a device (`haDrop` != 1): the legacy
  DROP list for every valve in both segment forms; migration from 2.0.0
  (`haLayout` < 2); (2) prune: every line of `/HADiscovery.cfg` that
  `classifyDiscoveryTopic` does not find among the current entities gets an
  empty retained config; (3) publish all current entities; (4) rewrite the
  list (`.tmp`, then rename). Delete: every listed and every current topic.
  The list is never renamed or removed, so the legacy firmware's "delete
  discovery" cleans up after a rollback.
- Topic `<discoveryPrefix>/<component>/<node>/<objectId>/config`, retained,
  `<node>` = `buildHaId(station)`, object ids `buildHaId(segment)`;
  unique_id = `<station ' '->'_'>.<uid>` from the raw segment (legacy ids
  kept, also for overrides with `/`).
- Availability classes: none (kept legacy entities: rollback-proof), esp
  (`<main>status`), esp+stm (`<main>status` and `<main>stm/status`,
  `availability_mode: all`).
- Payload parts: `name`, `unique_id`, `state_topic`, `command_topic` (when
  commandable), availability, device {identifiers station, name station,
  model "VdMot Revamped", manufacturer "Lenti84/Surfgargano", hw_version STM
  board tag (absent while unknown), sw_version, configuration_url
  `http://<ip>/`}. Everything is JSON-escaped. Temperature/volt sensors get
  `value_template` `{{ value | replace(',', '.') | float(None) }}` and
  `expire_after` max(3 x publishIntervalS, 60). The valve entity gets
  `qos: 1` and, with protocol 3, `payload_stop: STOP`. The event entity lists
  `eventMqttNames()`. Payloads up to 2047 B.
- The first-run cleanup does not wait for settled STM data: temp entities of
  valves whose sensors are still unknown are kept (KeptUnknown).

## 12. HTTP API

Routes, documents, error codes and the legacy aliases: `docs/revamped/API.md`.
Implementation rules:

- Port 80. Order per request: guard (`web_guard`, K5: Host, Origin, X-VdMot,
  Content-Type; refusals logged as 213 once per verdict per 60 s), then
  routing (`matchApiRoute`, `matchLegacyRoute`), then auth, then the body.
  Static assets and 410 answers skip the guard. Refusals are answered before
  the body is buffered.
- Auth: HTTP Basic (`checkBasicAuth`), enabled when `web.user` and
  `web.password` are both set. Classes: `/api/health` public; read routes
  (status, valves, profile, sensors, events, stm/motor, stm/flash,
  import-report) public unless `web.protectRead`; everything else needs
  auth. `AuthLimiter`: 8 client addresses, 10 failures within 60 s lock the
  address 1, 5, then 15 min (429 + `Retry-After`, event 214).
- Bodies: one static 8 KB buffer, one body at a time; uploads stream straight
  to LittleFS (`.part` file, the write result checked on every chunk, free
  space checked up front) or to the OTA partition, never through the JSON
  buffer. Only one upload or flash runs at a time.
- Responses: 2 x 12 KB slots; `POST /api/config` reserves its slot before
  applying. `/api/health` is answered outside the pool (1 KB).
- STM actions: `409 stm_unsupported` while `app::stmSupport()` is TooOld
  (except target, reset and flash) and for stop/safe mode below protocol 3;
  STM reset and flash start go through the reset gate (section 5).
- Every served request calls `net::noteInboundHttp(remoteIp)` (network
  evidence); `GET /api/log` calls `logger::requestFlush()` first.
- Static assets carry an ETag (CRC32 of their gzip bytes, generated at build
  time) and answer 304 on `If-None-Match`, so a browser reloads a changed
  `app.js` after an OTA.

## 13. Event codes

Numbers are external API: never renumber, only append. Severity is the
default from `eventDefaultSeverity`. MQTT (`eventMqtt`): **A** = always
reaches `<main>events`, **W** = when the logged severity is Warning or worse,
**-** = never. The registry (`event_log.cpp`) holds the messages.

| Code | Name | Severity | MQTT | Valve | arg1 / arg2 / text |
|---|---|---|---|---|---|
| 100 | boot | Info (Warning for reset reasons panic, int_wdt, task_wdt, wdt, brownout) | W | - | reset reason / boot count / fw version |
| 101 | config_imported | Info | W | - | imported / rejected / first rejected key |
| 102 | config_saved | Info | W | - | revision / - / source |
| 103 | config_defaults | Error | W | - | reason / - / - |
| 104 | fs_formatted | Warning | W | - | -1 = format failed |
| 105 | esp_ota_started | Info | W | - | size |
| 106 | esp_ota_done | Info | W | - | size / - / version |
| 107 | esp_ota_failed | Error | W | - | Update error |
| 108 | app_marked_valid | Info | W | - | seconds after boot |
| 109 | reboot_requested | Info (Warning for 2, 4 and 5) | W | - | reason 0 user, 1 ota, 2 net watchdog, 3 factory reset, 4 rollback, 5 network revert / detail: reason 2 outage minutes, reason 4 missing checks (bit0 net, bit1 http, bit2 stm) |
| 110 | low_heap | Warning | W | - | free / min free |
| 111 | time_synced | Info (first), Debug after | W | - | step s |
| 112 | calib_time_missing | Warning | W | - | slot key |
| 113 | factory_reset_skipped | Warning | W | - | factory pin still set: settings kept |
| 114 | stack_low | Warning | W | - | min free B / stack B / task |
| 115 | heap_fragmented | Warning | W | - | largest block / free |
| 116 | log_write_failed | Warning | W | - | step 1 open, 2 write, 3 rotate, 4 size limit / events lost |
| 117 | import_dropped | Warning | W | - | PI valves / dropped mask (1 pi, 2 window, 4 messenger, 8 ds18Timeout, 16 legacyFailsafe) / `ignored <n> keys` |
| 118 | config_restored | Warning | W | - | primary DecodeResult (0 NVS empty) |
| 119 | config_repaired | Warning | W | - | repair mask / count / first key |
| 120 | config_newer_schema | Warning | W | - | base schema / unknown ext records |
| 121 | files_removed | Info | - | - | files / KiB / name or `legacy images` |
| 200 | net_up | Info | W | - | 1 eth, 2 wifi / - / IP |
| 201 | net_down | Warning | W | - | interface |
| 202 | mqtt_connected | Info | W | - | |
| 203 | mqtt_disconnected | Warning | W | - | PubSubClient state |
| 204 | mqtt_command_rejected | Warning | W | - | valve 1-based or 0 / TargetPayload for payload rejects / reject reason |
| 205 | ha_discovery_sent | Info | W | - | configs / deletes |
| 206 | auth_failed | Warning | W | - | failures in window / - / client IP |
| 207 | net_trial_started | Info | - | - | window s / - / new address |
| 208 | net_trial_confirmed | Info | - | - | s since the network came up / 1 = by a newer change |
| 209 | net_trial_reverted | Warning | W | - | reason 1 not confirmed, 2 no network, 3 interrupted, 4 user, 5 trial not stored / 0 ok, -1 revert failed / previous address |
| 210 | net_unreachable | Warning | W | - | s since evidence / last NetEvidence |
| 211 | net_reachable | Info | - | - | outage s |
| 212 | net_interface_restart | Warning | W | - | outage s / 1 eth, 2 wifi, 3 both |
| 213 | request_refused | Warning | W | - | 1 host, 2 origin, 3 header, 4 content type / - / client IP |
| 214 | auth_locked | Warning | W | - | lock s / lockout level / client IP |
| 300 | link_up | Info | W | - | |
| 301 | link_degraded | Info | W | - | consecutive timeouts |
| 302 | link_down | Error | W | - | consecutive timeouts |
| 303 | stm_reset_by_policy | Error | W | - | consecutive timeouts / span s |
| 304 | stm_reset_by_user | Info | W | - | |
| 305 | stm_reboot_detected | Warning | W | - | cause 1 uptime, 2 resets, 3 v1 heuristic, 4 link recovered (protocol 1 or failed status check) |
| 306 | stm_version | Info | W | - | proto / hw id / version |
| 307 | stm_incompatible | Error | W | - | - / - / version |
| 308 | stm_rx_overflow | Warning | W | - | total / side 0 esp, 1 stm |
| 309 | stm_parse_errors | Warning | W | - | total / side |
| 310 | stm_queue_full | Warning | W | - | command |
| 311 | stm_flash_started | Info | W | - | size / - / image |
| 312 | stm_flash_done | Info | W | - | ms / - / new STM version |
| 313 | stm_flash_failed | Critical | W | - | FlashError / address / phase |
| 314 | failsafe_active | Warning | A | - | valve mask / 1 STM lease, 2 ESP emulation |
| 315 | failsafe_ended | Info | A | - | seconds / source |
| 316 | regulator_lost | Warning | W | - | 1 broker down, 2 HA offline |
| 317 | regulator_back | Info | A | - | seconds lost |
| 318 | lease_config_failed | Warning | W | - | 1 no reply, 2 rejected, 3 read-back differs / attempts |
| 319 | stm_safe_mode | Critical | A | - | watchdog resets |
| 320 | stm_safe_mode_ended | Info | A | - | |
| 321 | stm_config_repaired | Warning | W | - | cfgFlags / cfgEvents (first status only when the STM uptime is below 600 s, then every increase) |
| 322 | stm_uart_errors | Warning | W | - | ore + fe + ne / rxDropped |
| 323 | stm_eeprom_wait_timeout | Warning | W | - | waited ms / 1 STM reset, 2 flash, 3 ESP restart / "" or `stm task silent` |
| 324 | targets_restored | Info | - | - | valves / 1 RTC, 2 NVS |
| 325 | stm_protection_suspended | Error | W | - | (`gstax` sysFlags bit0 rising, once per STM boot) |
| 400 | target_set | Info | W | yes | target / TargetSource |
| 401 | valve_state_changed | Debug | W | yes | old / new status |
| 402 | valve_blocked | Error | W | yes | calibRetries / failsafe position or -1 |
| 403 | valve_failed | Error | W | yes | - / fault (protocol 3) or -1 |
| 404 | valve_no_valve | Warning | W | yes | |
| 405 | valve_recovered | Info | W | yes | previous bad status |
| 406 | calib_started | Info | W | yes (or all) | 1 scheduled, 2 automatic retry |
| 407 | calib_ok | Info | A | yes | oc / cc |
| 408 | calib_retry | Warning | A | yes | calibRetries |
| 409 | calib_failed | Error | A | yes | calibRetries / failsafe position or -1 |
| 410 | early_stop | Warning | W | yes | total / stop reason |
| 411 | cmd_rejected | Warning | W | yes | total |
| 412 | target_not_confirmed | Warning | W | yes | desired / attempts |
| 413 | valve_stale | Warning | W | yes | seconds |
| 414 | service_move_done | Info | W | yes | counted / stop reason |
| 415 | calib_stroke_short | Warning | W | yes | min(oc, cc) / minCounts |
| 500 | temp_sensor_failed | Warning | W | - | slot / raw / id |
| 501 | temp_sensor_recovered | Info | W | - | slot |
| 502 | sensor_count_changed | Info | W | - | count / 0 temp, 1 volt |
| 503 | volt_sensor_failed | Warning | W | - | slot / raw |
| 600 | scheduled_calibration | Info | W | - | slot key / minutes late (logged at the STM's confirmation) |
| 601 | scheduled_calibration_failed | Warning | W | - | slot key / 1 no reply, 2 not sent, 3 no result, 4 STM unsupported |
| 602 | scheduled_calibration_missed | Error | W | - | slot key / attempts |

Event text line (`formatEventLine`, file, serial and `/api/log`):
`#7 2026-09-23T14:03:05Z WARNING early_stop v3 valve 3: early stop (total 2, endstop)`,
or `+123s` in place of the time before SNTP sync. Gap line:
`#<from>-<to> gap: <n> events not written`. Syslog: RFC 5424 over UDP,
facility local0, app `vdmot`, host = DHCP host name, msgid = code name.
Syslog level 1 sends Warning+, level 2 Info+, level 3 everything.

## 14. Calibration schedule

`CalibScheduler` (`stm_service`, app task, every 10 s) with `calib.dayMask`,
`calib.hour` and `calib.minute`. Grace window 120 min. A slot fires at most
once per local date (key yyyymmdd) and the key only moves forward. DST:
spring forward fires at the first minute after the gap; fall back fires once.
Without valid time nothing fires; after 1 h of uptime without time,
`CalibTimeMissing` is raised once per boot.

Firing sends `Calibrate(kAllValves)` (`staln 255`; the STM runs one motor at
a time). The slot is **booked** (NVS `calSlot`, `lastCal`, event
`ScheduledCalibration`) only when the STM acknowledged the `staln` (result
within 60 s). Without a confirmation the attempt is repeated every 10 min
inside the grace window (`scheduled_calibration_failed` per attempt, reason
1..4), then `scheduled_calibration_missed`; the next scheduled date fires
normally. A lost ack can cause one extra calibration 10 min later.
`app::CalibInfo` carries the last booked slot and `nextEpoch` (the next slot
at `calib.hour:calib.minute` local time, 0 = none) for `/api/status` and
`diag/calibration/next`. Manual calibrations are not tied to the schedule.

STM time trigger (`LearnTimeSync`, stm task): while the ESP schedule is on,
the STM's own time trigger is switched off (`stlnt 0`), else it is set to
604800 s. Protocol 3: read with `gtlnt`, written only when different, then
verified. Protocols 1/2 (no read-back, not stored by those STMs): one
`stlnt 0` per session, and the default only after such a `stlnt 0`. Failures
retry after 60 s.

## 15. STM flasher

`StmFlasher` runs inside the stm task (`step()` every 2 ms). While it runs,
`LinkPolicy` is suspended and nothing else uses the UART. A flash start waits
for the STM EEPROM (reset gate, section 5; `pending` in the flash status).
Sequence and timing (`FlashOptions` defaults, binding):

1. Validating: `validateImage` (size 1..512 KiB, and within the detected chip's
   flash once the PID is known; SP in RAM; odd reset vector inside the image;
   `DEADBEEF` and `BEEFIT` strings present unless `force`; padded to a
   multiple of 4 with 0xFF; CRC32; the `VDM-HW:C<n>` marker, two different
   markers = conflict). Then the board check `checkBoard(image tag,
   boardHw)`: boardHw is the running STM's `gvers` tag, else the user's
   choice. Mismatch or conflict -> BoardMismatch; a tagged image with an
   unknown board -> BoardRequired; both unless `force`; an untagged image
   flashes with status `untagged`. Nothing is touched before.
2. Resetting: UART 115200 8E1, NRST asserted 100 ms, released.
3. Handshake (normal mode): first `DEADBEEF\n` 20 ms after release, then every
   100 ms, for up to 2.5 s. The STM compares fixed 8-byte chunks without
   resynchronising; the 9-byte period re-aligns within 8 sends. `BEEFIT` is
   found with a sliding window. Blank mode (BOOT0 held by hand) skips this
   step and opens the UART at the session baud directly.
4. Sync: wait 250 ms, then `0x7F`. ACK or NACK both count as synced (NACK
   means already synced). 3 attempts, 1 s each.
5. GetId `0x02`: the PID must be 0x423, 0x431 or 0x433 and match the image's
   SP/size, else UnknownChip / ImageChipMismatch. SyncFailed or a GetId
   without any answer start one more session at 57600 baud (new NRST pulse,
   `fallbackBaud`); `baud` in the status.
6. Erasing: `0x44` with the sector list from `sectorsForImage` (F401/F411:
   16, 16, 16, 16, 64, 128, ... KiB). Timeout 60 s. No mass erase.
7. Writing: `0x31` 256-byte blocks from block 1 upwards, block 0 (the vector
   table) last. Every ACK is awaited with a 1 s timeout plus the frame's wire
   time at the session baud. A failed block is retried 3 times with command,
   address and data.
8. Verifying: `0x11` read-back of every block, byte compare. The first
   mismatch address is reported.
9. On a write or verify failure the whole erase, write and verify is
   repeated up to 2 times in the same ROM session, without a reset.
10. Blank mode ends here: Done with `manualReset` (BOOT0 still set: no reset,
    no `gvers`; the user removes BOOT0 and resets the STM). UART back to
    115200 8N1.
11. Starting (normal mode): NRST pulse, UART 8N1.
12. WaitingApp: after 4 s, `gvers` every 1 s, for up to 60 s from the NRST
    release. The reply must match the image's version string when one was
    found and the image's board tag (else AppVersionMismatch, unless `force`).
    No reply gives AppNotResponding.

Percent (monotonic): validating to getid 0..5, erasing 5..15, writing 15..75
by bytes, verifying 75..95, starting and waiting 95..99, done 100. The legacy
status code (`legacyFlashStatus`): Idle 0; Validating to GetId 1; Erasing 3;
Writing 4; Verifying, Starting and WaitingApp 5; Done 6; Failed 8. The ESP
never restarts after flashing, whether it succeeds or fails. Afterwards the
link resumes with a 5 s hold-off and a full re-sync, and desired targets are
re-pushed. On success the image is copied to `/stm/last_good.bin`.

## 16. OTA, restarts and network

- OTA validation (`OtaValidator`): `verifyRollbackLater()` returns true, so a
  new image boots in PENDING_VERIFY. It is marked valid after 120 s of
  uninterrupted health: **net** (IP up and proven by traffic, see below),
  **http** (the loopback self-check `GET /api/health` every 10 s answered
  200 within the last 30 s), **stm** (link Up; required only when the link
  was Up at the upload, NVS `otaStm`). If that never happens by 15 min of
  uptime, the image is rolled back (restart reason 4 with the missing
  checks). The pending image gets one boot: a user restart (reboot, network
  settings, factory reset) confirms it first when net and, if required, stm
  are fine; any other reset of a pending image lets the bootloader roll back.
  A second ESP upload is refused while the image is pending. `/api/health`
  `ota` shows the checks and the time left.
- ESP OTA upload: `ota::uploadBegin`, `uploadWrite` and `uploadEnd`. One
  upload at a time, none while an STM flash is running. Content-Length must
  match the bytes written; optional MD5. On success the ESP restarts 1 s
  after the response.
- Restart sequence: every restart goes through `ota::requestRestart(reason,
  delay, detail)` (event `RebootRequested`), then `ota::serviceRestart` once
  no STM flash runs: (1) `app::requestStmSave()` and, except for a factory
  reset, `stm_service::flushForRestart()` (desired targets to NVS); (2)
  `RestartGate`: proceed when the stm task reports Saved, Unavailable or
  TimedOut (its `ResetGate`, at most 10 s), or after the 12 s guard (event
  323 `stm task silent`); (3) confirm a pending image if allowed, persist
  `otaStm` for an OTA, `logger::flush()`, MQTT `offline`, then
  `esp_restart()` or the rollback call. No STM flash starts while a restart
  is pending (HTTP 409). With jumper X20 fitted the ESP restart also resets
  the STM (IO15 strap).
- Network reachability (`NetReachability`): evidence = a gateway ping reply
  (probe every 60 s), the MQTT session, an SNTP sync, an HTTP request from a
  LAN peer, a DHCP lease. The staleness check (150 s without evidence) is
  armed only by a ping reply, so a gateway without ICMP keeps the IP-only
  behaviour. Events 210/211.
- Network watchdog (`NetWatchdog`, legacy `netConnTO`): `reconnectTimeoutMin`
  minutes (default 5) without reachability -> restart the network interface
  (event 212, once per outage); after another `reconnectTimeoutMin` ×
  4^restarts minutes (capped at 24 h) -> restart the ESP (reason 2). 0
  disables both. The restart count survives software restarts in RTC memory
  and is cleared as soon as the network is reachable.
- Network trial (`NetTrial`): a saved change with `netTrialRequired`
  (interface, DHCP, static fields in use, WiFi fields on WiFi) stores the
  previous fields in NVS `netTrial` (Armed) and restarts. The next boot runs
  the new settings (Running) with a 120 s window from the first IP (120 s
  after boot without network); `POST /api/system/network/confirm` keeps
  them, `revert` or the timeout restores the previous fields (event 209,
  restart reason 5). A boot that finds Running reverts at once without an
  extra restart. A network change saved during a trial confirms it (208
  arg2 1) and starts a new one. A record that cannot be stored (Armed at
  the save, Running at boot) reverts at once to the settings in use (209
  reason 5): without it an interrupted trial would not revert. Config
  backups are not written while a trial runs.
- Factory reset: GPIO2 LOW for 5 s at boot, once per fitting of the jumper
  (NVS latch `frLatch`, set after a reset that succeeded, cleared when the
  pin reads HIGH at boot or at run time; a boot with the latch set logs 113
  and keeps the settings), or
  `POST /api/system/factory-reset`. It erases `vdmrev` except the latch (the
  legacy namespaces stay, and `imported` is set so they are not imported
  again).

## 17. Tests and mutation

- `test/native`: core tests `test_<module>.cpp` (doctest, ASan/UBSan,
  `-Werror`); `test_smoke.cpp` covers linkage, version, number helpers, the
  JSON writer and config defaults.
- Glue suites (`test/native/glue`): the files of `src/` built on the host
  against fakes of Arduino, ESP-IDF, FreeRTOS, AsyncWebServer, PubSubClient,
  LittleFS, NVS and the STM (`FakeStm` with golden protocol-3 replies), run
  by `tools/native/testkit` (multi-boot: RTC_NOINIT data carried over
  simulated software restarts, 0xA5 after power-on).
- Run everything in the container: `bash tools/native/docker.sh test esp32`.
- Mutation: `tools/mutation/esp32.json` (every `lib/core/src/*.cpp`) and
  `esp32-glue.json` (the glue). Target 95 % overall and for every file;
  stillborn mutants (not compilable) are not counted. Surviving mutants are
  either killed with new tests or documented as equivalent with a reason
  (`tools/mutation/equivalents`); unreachable code carries `// NOMUTATE`
  with its reason.
- On-device behaviour is verified with the checklist in
  `docs/revamped/INSTALL.md`; items that need a measurement are marked [HW]
  there.
