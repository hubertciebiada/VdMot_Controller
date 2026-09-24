# VdMot Revamped ESP32 firmware: design (binding)

Status: skeleton. Every public header in `lib/core/include/vdm/` is the API
contract; this file holds the numbers, tables and rules that the module
implementers must follow. Architecture decisions live in `specs/00-architecture.md`;
facts about the legacy firmware live in specs 01 to 06. Where this file and a
header disagree, fix both in the same commit.

Version: `2.0.0-revamped` (release env) and `2.0.0-revamped-dev` (dev env),
from the `VDM_VERSION` build flag via `vdm::firmwareVersion()`. The string
appears in `/api/status`, the dashboard footer, the HA `device.sw_version` and
the `Boot` event.

Contents
1. Build, layout, rules for implementers
2. Module map
3. Task model
4. Data flow
5. STM link policy
6. Target delivery and valve health
7. Config schema
8. Legacy import
9. Persistence and memory
10. MQTT topics
11. HA discovery
12. HTTP API
13. Event codes
14. Calibration schedule
15. STM flasher
16. OTA and restart
17. Tests and mutation

---

## 1. Build, layout, rules for implementers

```
software_esp32_revamped/
  platformio.ini            envs wt32-eth01_revamped (release), wt32-eth01_revamped_dev
  lib/core/                 hardware-free C++17 logic (vdm_core), host-tested
    include/vdm/*.h         public APIs (this contract)
    src/*.cpp               implementations
  src/                      Arduino glue (thin): app, stm_link, mqtt_client, web_server,
                            net, storage, logger, ota, board.h, main.cpp
  src/generated/            web_assets.h, generated on every pio build (git-ignored)
  web/                      dashboard sources (vanilla HTML/CSS/JS)
  tools/gen_web_assets.py   gzip web/* -> src/generated/web_assets.h (PlatformIO pre-script)
  test/native/              CMake + doctest; globs lib/core/src/*.cpp and test_*.cpp
```

Toolchain (pinned, never `^` ranges): PlatformIO core 6.1.19 in CI,
`espressif32@6.1.0` (Arduino-ESP32 2.0.7, ESP-IDF 4.4, GCC 8.4),
ArduinoJson 6.21.6, PubSubClient 2.8, AsyncWebServer_WT32_ETH01 1.6.2,
AsyncTCP 1.1.1. C++17 (`-std=gnu++17`).

Partition table: board default, identical to the legacy build (nvs 0x9000
20 KB, otadata 0xE000, app0 0x10000 1280 KB, app1 0x150000 1280 KB, spiffs
0x290000 1472 KB used as LittleFS). Never set `board_build.partitions`: the
upgrade from the legacy firmware is an OTA, and OTA cannot change the table.
The app image must stay below 1.1 MB (CI fails above 1.2 MB). Skeleton size:
964 KB release, so every module has to watch its flash cost (no iostream,
no `std::string`, no `printf` of floats outside formatters).

Commands:
```
pio run -j 2 -e wt32-eth01_revamped
cmake -S test/native -B build/native && cmake --build build/native -j 2
ctest --test-dir build/native --output-on-failure
```

Rules for module implementers:
- Keep the public API and the documented semantics. Private members may be
  changed freely. If a contract turns out wrong, change header + this file +
  tests together and say why in the commit.
- Core code: no Arduino/ESP-IDF headers, no heap, no exceptions, no RTTI
  (virtual interfaces are fine), fixed-size storage, every input bounded. Use
  `elapsedMs()`/`timeReached()` for time. Keep outputs NUL-terminated.
- Tests: one `test/native/test_<module>.cpp` per module. Cover every public
  function, boundaries, malformed input, and for parsers a fixed-seed
  random-bytes fuzz loop. Never edit CMakeLists.txt to add files.
- Native tests are built with the host g++ and `-Wall -Wextra -Werror`
  plus ASan/UBSan. (The stubs trip clang's `-Wunused-private-field`; finished
  modules must build with clang too.)
- Glue: no decision logic. If a glue function needs an `if` that is more than
  plumbing, the decision goes into a core module first.

## 2. Module map

| Core header | Owns | Used by (glue) |
|---|---|---|
| `common.h` | constants (12 valves, 34 temp slots, 8 volt slots), time helpers, strict parsers, IPv4, 1-Wire id, name policy | all |
| `version.h` | ESP version, `gvers` parsing, compare, revamped detection | stm_link, web |
| `line_assembler.h` | bounded CR/LF line framing of UART bytes | stm_link |
| `stm_codec.h` | request builders, reply parser, reply/request matching, chip names | stm_link |
| `link_policy.h` | request queue, one outstanding request, timeouts/retries, R6 reset decision, reboot detection | stm_link |
| `poll_planner.h` | re-sync sequence and periodic poll cadence | stm_link |
| `valve_model.h` | valve state, target delivery, health flags, sensor readings | stm_link (writer), others via snapshot |
| `health_monitor.h` | events from model changes, MQTT event rate limit, OTA validator, network watchdog, `systemState` | stm_link, app, mqtt, ota, net |
| `event_log.h` | event codes, severities, ring buffer, text/JSON formatting | logger, web, mqtt |
| `mqtt_topics.h` | topic tree, command topic/payload parsing, payload formatters, publish scheduler | mqtt_client |
| `ha_discovery.h` | discovery entity list, DROP delete list, stale-topic check | mqtt_client |
| `config.h` | config schema, defaults, validation, key-path setter, JSON export, NVS encoding, CRC32 | storage, web, all readers |
| `legacy_import.h` | legacy NVS -> Config mapping | storage |
| `calib_schedule.h` | scheduled calibration decision | app |
| `stm_flasher.h` | AN3155 flasher state machine, image validation | stm_link |
| `json_writer.h` | bounded JSON writer | json_api, event_log, config, ha_discovery, mqtt |
| `json_api.h` | `/api/*` documents, API router with auth requirement | web_server |
| `auth.h` | Basic-auth check, constant-time compare, brute-force limiter | web_server |

| Glue module | Owns |
|---|---|
| `main.cpp` | `setup()` -> `app::setup()`, `loop()` deletes itself, `verifyRollbackLater()` |
| `app` | boot sequence, task creation, TWDT, command queue, STM snapshot, app task (net/web start, calibration schedule, log flush, OTA validation, restarts) |
| `stm_link` | Serial2, NRST; runs codec/link/planner/model/flasher |
| `mqtt_client` | PubSubClient, LWT, publishing, discovery, target commands |
| `web_server` | AsyncWebServer, assets, API, uploads |
| `net` | ETH/WiFi, SNTP/TZ, mDNS, net watchdog |
| `storage` | NVS `vdmrev`, legacy NVS reader, LittleFS mount |
| `logger` | event ring, serial mirror, file rotation, syslog |
| `ota` | ESP OTA upload, rollback validation, deferred restart |

## 3. Task model

| Task | Created by | Core | Prio | Stack | TWDT | Loop | Runs |
|---|---|---|---|---|---|---|---|
| `stm` | app::setup | 1 | 5 | 6144 B | yes | 2 ms (`vTaskDelay(2)`) | commands in, UART RX, parse, link policy, target delivery, planner, UART TX, flasher, health events, snapshot publish |
| `app` | app::setup | 1 | 3 | 6144 B | yes | 100 ms | config revision, net service + web start (1 s), OTA validation (1 s), calibration schedule (10 s), log sinks, deferred restart |
| `mqtt` | app::setup | 1 | 2 | 8192 B | yes | 20 ms connected, 100 ms connecting, 500 ms off | connect/back-off, `loop()`, publishing, discovery (one message per pass), events |
| `async_tcp` | AsyncTCP lib | 0 | lib default (3) | lib default | lib WDT (`CONFIG_ASYNC_TCP_USE_WDT=1`) | event driven | all HTTP handlers |
| sys event | ESP-IDF | 0 | IDF | IDF | no | event driven | `net` WiFi/ETH event callback (only sets flags) |
| `loopTask` | Arduino | 1 | 1 | - | - | - | runs `setup()`, then deletes itself |

- Task watchdog: `esp_task_wdt_init(30 s, panic=true)`; `stm`, `app` and
  `mqtt` subscribe and reset it every loop. A missed feed ends in a panic
  reboot with reason TASK_WDT, reported by the next `Boot` event (Warning).
- Blocking limits: `stm` never blocks except the 100 ms NRST pulse; `mqtt`
  may block in `connect()` for at most 3 s TCP + 5 s CONNACK (< 30 s TWDT);
  `app` blocks only for LittleFS writes (bounded per pass) and the 1 s
  factory-reset check at boot; HTTP handlers never block.
- Ownership: only `stm` touches Serial2 and NRST. Only `mqtt` touches the
  PubSubClient. NVS is accessed only through `storage` (mutex).
- Cross-task data:
  - `app::submit(Command)`: FreeRTOS queue, depth 16, non-blocking; full ->
    HTTP 503 / MQTT reject event.
  - `app::StmSnapshot`: written by `stm` at most every 100 ms and only when
    something changed; readers copy it under a mutex into their own static
    buffer (`web`, `mqtt`). About 5.5 KB.
  - Config: `storage::getConfig()` copy under mutex; `configRevision()`
    tells readers when to reload. `storage::applyConfig()` validates,
    persists and bumps the revision.
  - Events: `logger::log()` from any task (mutex); readers use cursors
    (`readSince`).
- Boot order (`app::setup`): Serial -> `stm_link::begin()` (NRST released,
  UART 8N1, never a reset: R6) -> queue/mutex -> logger -> factory-reset pin
  -> LittleFS -> config load/import -> `Boot` event -> net -> OTA state ->
  MQTT config -> TWDT -> tasks. The HTTP server starts in the app task once the
  network has an IP.

## 4. Data flow

```
 MQTT target ─┐                     ┌────────────── stm task ───────────────┐
 HTTP actions ├─ app::submit ──────►│ Command -> ValveModel / LinkPolicy     │
 schedule ────┘                     │ LinkPolicy.nextToSend -> Serial2 TX    │
                                    │ Serial2 RX -> LineAssembler -> parse   │
                                    │   -> LinkPolicy.onReply -> models      │
                                    │ PollPlanner / ValveModel pushes        │
                                    │ HealthMonitor -> logger::log(Event)    │
                                    └──── publishStmSnapshot ───────────────┘
                                                  │
                    ┌─────────────────────────────┼──────────────────────┐
              mqtt task (copy)             web handlers (copy)       app task
              compat + diag topics         /api/* JSON               OTA validator (link state)
              events (rate-limited)        dashboard                 calibration schedule
```

Rules: targets flow only from MQTT and HTTP (R7). The ESP never invents a
target. At ESP boot it adopts the STM's current targets (`gtgtp`/`gvlvx`).
After an STM reboot it re-pushes its own desired targets.

## 5. STM link policy

Physical: Serial2, RX IO5, TX IO17, 115200 8N1 (8E1 only while flashing), RX
buffer 2048, TX buffer 512. NRST on IO15, HIGH = reset asserted. The ESP
never resets the STM at its own boot (R6).

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
| Down after | 5 consecutive timed-out attempts | |
| Queue | 24 entries | User > Config > Poll, FIFO within a priority |

Coalescing: a new `stgtp` for a valve replaces the queued one (latest
target wins). Any other byte-identical line is merged. Full queue: the newest
Poll entry is evicted for User/Config; Poll never evicts.

Priorities: User = HTTP/MQTT actions (calibrate, assembly, detect, scan,
sensor mapping, motor parameters, breakaway, service move). Config = target
pushes and the re-sync sequence. Poll = periodic reads and read-backs.

Reply matching (`replyMatches`): same command, and for `gvlvd`, `gvlvx`,
`gtgtp`, `gprof`, `svmov` and `stvls` the same valve. `gvlon` also accepts
`goned error`. `goned 0`/`gowvd 0` complete as Rejected. A valid reply that
matches nothing is stray: it is counted, and self-identifying data (`gvlvd`,
`gvlvx`, `gtgtp`, `gprof`) is still applied. Parse errors are counted and
never count as proof of life.

STM reset (R6), `shouldResetStm()` true only when all of these hold:
- at least 5 consecutive timed-out attempts,
- the first of them at least 60 s ago,
- no policy reset in the last 10 min,
- the link is not Booting or Suspended.
A `gproto` probe timeout never counts (a v1 STM does not answer it). After the
pulse: `StmResetByPolicy` (Error), the outstanding request and all Poll
entries are dropped, User/Config entries stay queued, 5 s hold-off, full
re-sync, and all desired targets are re-pushed.

Reboot detection (`RebootDetector`): v2 when `gstat` uptime decreases or the
reset counter changes; v1 when a valve that had `oc` or `cc` > 0 reports
`oc = cc = moves = 0` with status 5, 6 or 8; both on Down -> Up. The result is
`StmRebootDetected` (Warning), re-sync, and targets re-pushed.

Feature detection: the re-sync starts with `gproto`. The reply `gproto 2`
selects v2 (`gvlvx`, `gstat`, `gcalx`, `gprof`, `svmov`, `scalx`). A timeout
selects v1. v2 commands are never sent to a v1 STM. Because the IO15 strap
pull-up holds NRST while the ESP boots (specs/06 §5.2), the link starts in
Booting with the normal 5 s hold-off (`holdAfterEspBoot`, no reset counted),
so the first probe does not hit the STM's start-up window. If a probe still
timed out and `gvers` later reports a revamped version, the re-sync is
restarted once to probe again (`PollPlanner::onVersion`). `gvers` is parsed with
`parseVersion`. A version below `VDM_MIN_STM_VERSION` (1.4.0) raises
`StmIncompatible` and a dashboard banner. The ESP keeps running in v1 mode.

### Poll cadence (`PollCadence`, binding)

| Item | Period | Request |
|---|---|---|
| Busy valve (moving, calibrating, sync not settled) | 500 ms | `gvlvd` (v1) / `gvlvx` (v2) |
| Active valve, idle | 2 s | same |
| Inactive valve | 30 s | same |
| Each DS18 bus index | 10 s | `goned i` (one index per 10 s / count) |
| Each DS2438 bus index | 10 s | `gowvd i` |
| Sensor counts | 30 s | `gonec`, `gowvc` (list re-read on change) |
| STM health (v2) | 10 s | `gstat` |
| Version | 5 min | `gvers` |

Re-sync sequence (`ResyncStep`): `gproto`, `gvers`, `ghwin`, `gmotc`,
`gtlnm`, `gcalx` (v2), `gonec 255`, `gowvc 255`, `gvlon 255`, `gvlst` (v1),
then `gtgtp 0..11` (v1) or `gvlvx 0..11` (v2). It is interleaved with valve
polling: at most every second request is a re-sync step. Failed steps are
retried. Triggers: ESP boot, STM reboot detected, STM reset (policy or user),
link Down -> Up, after flashing.

Known v1 quirks the codec and model handle: the trailing comma in `gvlst`;
`gonec 0` means both "count 0" and "empty list"; `gvlon` error has the
`goned` prefix; `gvlon` may report garbage ids for unassigned sensors (resolved
against configured slots, never written back); temperature sentinels -500,
-1270 and 850; `stgtp` is ACKed but dropped while calibrating (delivery is
verified, section 6).

## 6. Target delivery and valve health

Target delivery (`ValveModel`), per valve:
```
Unknown --gtgtp/gvlvx--> Synced (desired := STM target, source stm)
Synced --setDesiredTarget(new)--> Pending
Pending --nextTargetPush (active, known, !calibrating, >= 2 s since last push)--> AwaitAck
AwaitAck --stgtp ack--> AwaitVerify --read-back equal--> Synced
                                     --read-back differs--> Pending
AwaitAck --timeout--> Pending; after 5 pushes -> Failed (TargetNotConfirmed, kHealthTargetUnconfirmed)
Failed --5 min--> Pending
any --STM reboot/reset/flash--> Pending (desired kept) or Unknown (no desired)
```
Read-back: `gtgtp` on v1, `gvlvx` on v2 (it carries the target). Because the
STM drops `stgtp` during calibration, a Pending push waits while
`calibrating` is true, and read-back mismatches are retried after the
calibration ends. Inactive valves never get pushes, and
`setDesiredTarget` rejects them (HTTP 409, MQTT reject event).

Health flags (`HealthFlag`), recomputed on every update:

| Flag | Condition |
|---|---|
| Blocked | status 9 |
| Failed | status 4 |
| NoValve | status 6 and valve active |
| CalibRetries | `calibRetries > 0` |
| EarlyStop | v2 `earlyStops` > value at ESP boot / last STM reboot |
| CmdRejected | v2 `cmdRejected` > baseline |
| Stale | active and no `gvlvd`/`gvlvx` for 60 s |
| TargetUnconfirmed | sync Failed |
| TempFailed | an assigned sensor (`temp1`/`temp2` != -500) reports -1270, 850 or out of range |

`HealthMonitor::onValve` turns transitions into events (section 13). Each
condition fires once when it starts and once (`ValveRecovered`, or the
calibration outcome) when it ends, so a stuck condition never floods the log.

`systemState` (MQTT `common/state`): 2 = link Down or an active valve
Blocked/Failed; 1 = link Unknown/Degraded/Booting/Suspended or any other
health flag on an active valve; 0 = otherwise.

## 7. Config schema

One struct, `vdm::Config` (`config.h`). The key path is what
`setConfigValue` takes and what `writeConfigJson` emits. Arrays are 1-based in
paths (`valves.3.name`) and 0-based JSON arrays in the export. IPv4 values are
dotted strings in JSON. Secrets are write-only: the export has `"<key>Set":
true|false`, and an empty string leaves the secret unchanged unless the
request says to clear secrets. "Legacy source" is the NVS namespace/key that
`importLegacyConfig` reads (specs/04 §5). `-` means the key is new.

| Key | Type | Range / rule | Default | Legacy source |
|---|---|---|---|---|
| `schema` | int | read-only | 1 | - |
| `station` | string | 1..20, `isSafeName` | `VdMot` | `sysCfg/stName` |
| `net.iface` | int | 0 auto, 1 ethernet, 2 wifi | 0 | `netCfg/ethwifi` |
| `net.dhcp` | bool | | true | `netCfg/dhcp` |
| `net.ip` | IPv4 | non-zero when !dhcp | 0.0.0.0 | `netCfg/staticIp` |
| `net.mask` | IPv4 | contiguous, non-zero when !dhcp | 0.0.0.0 | `netCfg/mask` |
| `net.gateway` | IPv4 | non-zero when !dhcp | 0.0.0.0 | `netCfg/gw` |
| `net.dns` | IPv4 | any | 0.0.0.0 | `netCfg/dnsIp` |
| `net.ssid` | string | 0..32 printable ASCII; required when iface = wifi | "" | `netCfg/ssid` |
| `net.wifiPassword` | secret | 8..63 when ssid set | "" | `netCfg/pwd` |
| `net.reconnectTimeoutMin` | int | 0..240 (0 = never restart) | 5 | `netCfg/netConnTO` |
| `time.ntpServer` | host | 0..64, `isHostName` or IPv4; "" disables SNTP | `pool.ntp.org` | `netCfg/timeServer` |
| `time.tzName` | string | 0..49 printable | `Europe/Berlin` | `tZCfg/tZ` |
| `time.tzPosix` | string | 1..49 printable, no spaces | `CET-1CEST,M3.5.0,M10.5.0/3` | `tZCfg/tZCode` |
| `syslog.level` | int | 0 off, 1 warning+, 2 info+, 3 debug | 0 | `netCfg/syslogEnable` |
| `syslog.server` | IPv4 | non-zero when level > 0 | 0.0.0.0 | `netCfg/sysLogIp` |
| `syslog.port` | int | 1..65535 | 514 | `netCfg/sysLogPort` (0 -> 514) |
| `web.user` | string | 0..64 printable, no ':' | "" | `netCfg/userName` |
| `web.password` | secret | 0..64; user and password both set or both empty | "" | `netCfg/userPwd` |
| `web.protectRead` | bool | | false | - |
| `mqtt.mode` | int | 0 off, 1 MQTT, 2 MQTT + HA (needs `separate`) | 0 | `protCfg/dataProt` |
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
| `valves.N.name` (N 1..12) | string | 0..10 `isSafeName`, unique, not equal to another valve's number segment | "" | `valvesCfg/valves` blob |
| `valves.N.active` | bool | | false | same blob |
| `temps.N.name` (N 1..34) | string | 0..10 `isSafeName` | "" | `tempsCfg/temps` blob |
| `temps.N.active` | bool | active needs an id | false | same |
| `temps.N.offset` | number, °C | -10.0..10.0, stored as tenths, rounded half away from zero | 0.0 | same (int tenths) |
| `temps.N.id` | 1-Wire id | "" or `hh-hh-hh-hh-hh-hh-hh-hh`; unique across slots | "" | same (`ID[25]`) |
| `volts.N.name` (N 1..8) | string | 0..10 | "" | `voltsCfg/volts` blob |
| `volts.N.active` | bool | active needs an id | false | same |
| `volts.N.offset` | number | finite, -1000..1000 | 0 | same (float) |
| `volts.N.factor` | number | finite, -1000..1000, != 0 | 1 | same (float) |
| `volts.N.unit` | string | 0..8 `isSafeName` | "" | same (`unit[9]`) |
| `volts.N.id` | 1-Wire id | as temps | "" | same |
| `calib.dayMask` | int | 0..127, bit i = weekday i (bit0 Sunday), 0 = off | 9 (Sun + Wed) | `valvesCfg/dayOfCalib` |
| `calib.hour` | int | 0..23 | 0 | `valvesCfg/hourOfCalib` |
| `calib.minute` | int | 0..59 | 0 | - |
| `persistLog` | bool | | true | - |

Not in the ESP config, but held by the STM and edited through `/api/stm/motor`:
motor characteristics (`gmotc`/`smotc`: low/high factor 10..40, startOnPower
0..100, minCounts 0..60000, maxCalibRetries 0..2), learnAfterMovements
(`gtlnm`/`stlnm`: 0 or 50..65534), breakaway (v2 `gcalx`/`scalx`), and the
valve to sensor mapping (`gvlon`/`stvls`; the ESP resolves ids to slots for
display and only writes a mapping the user changed).

Dropped legacy keys (never imported): `sysCfg/CF` (°C only),
`protCfg/brokerInterval`, `brokerMQTO`, `brokerMQToPos`, `brokerMQF` bit0 and
bit1, all of `valvesCtrlCfg`, `msgCfg`, `motorCfg`, `valvesCfg/movCalib`.

Apply semantics (`POST /api/config`): the glue copies the active config,
applies every key with `setConfigValue` (the first failure aborts with 400 and
the key path), runs `validateConfig` (a failure gives 400 and the path), then
`storage::applyConfig`. Live effects: MQTT reconnects on any `mqtt.*` or
`station` change; logger sinks reconfigure; the stm task reloads the active
mask and slot ids; `net.*`, `station` and WiFi credential changes schedule an
ESP restart after the response (the STM keeps running).

## 8. Legacy import

`storage::loadConfig` runs `importLegacyConfig` exactly once: when
`vdmrev/cfg` is missing and `vdmrev/imported` is not 1. The result is saved,
`imported` is set, and the legacy namespaces are never written or erased (a
downgrade still finds them). Per-key rules are in `legacy_import.h`. Blob
sizes must match exactly (144 / 1496 / 480 bytes), otherwise the whole blob is
rejected. The report goes into the `ConfigImported` event (imported/rejected
counts, first rejected key). `Misc/MiscLC` becomes `vdmrev/lastCal`.
`/HADiscovery.cfg` (legacy discovery topic list) is used once by the MQTT
task: every line for which `discoveryTopicIsCurrent` is false gets an empty
retained publish, then the file is renamed to `/HADiscovery.cfg.done`.

## 9. Persistence and memory

NVS namespace `vdmrev`:

| Key | Type | Content |
|---|---|---|
| `cfg` | blob <= 4096 | `encodeConfig`: "VDMC", u16 schema, u16 length, fields LE, CRC32 |
| `imported` | u8 | 1 = legacy import done (or factory reset) |
| `boots` | u32 | boot counter |
| `calSlot` | u32 | last scheduled calibration slot (yyyymmdd) |
| `lastCal` | i64 | epoch of the last calibration command |
| `haDrop` | u8 | 1 = legacy DROP discovery entities deleted |

A config blob that fails CRC, is invalid, or has a newer schema is not
overwritten automatically. The device boots with defaults and raises
`ConfigDefaults` (Error). The next explicit save replaces the blob.

LittleFS (partition `spiffs`, mounted at `/littlefs`, formatted only when
mounting fails, which raises `FsFormatted`):

| Path | Content | Limit |
|---|---|---|
| `/stm/<name>.bin` | uploaded STM images, name `[A-Za-z0-9._-]{1,31}` | 512 KiB each, at most 3 plus `last_good.bin` |
| `/stm/<name>.bin.part` | upload in progress, renamed when complete | |
| `/log/events.log`, `/log/events.1.log` | event log lines (`formatEventLine`) | 64 KB each |
| `/HADiscovery.cfg(.done)` | legacy discovery topic list | read once |

RAM budget (skeleton: 100 KB static DRAM of 320 KB):
- Allocated once at boot and never freed: event ring 512 x 48 B = 24 KB
  (`logger::begin`) and web response slots 2 x 12 KB (`web::begin`).
  Everything else is static or on a task stack.
- Big objects stay off the stacks: `vdm::Reply` (1 KB), `StmSnapshot`
  (5.5 KB), `Config` (1.9 KB), `DiscoveryContext` (2.6 KB).
- No per-operation heap: PubSubClient's buffer is set once (1280 B). HTTP
  responses use the slot pool through `beginResponse_P`, and the slot is
  released in `onDisconnect`. POST bodies go into one static 4 KB buffer.
  The only per-request allocations are AsyncWebServer's own request and
  response objects, which it frees after each request.
- `LowHeap` (Warning) when the free heap drops below 30 KB, at most once per
  hour.

## 10. MQTT topics

Connection: PubSubClient over WiFiClient (ETH or WiFi), MQTT 3.1.1, QoS 0.
Client id = station (legacy; empty -> `VdMot`). User and password are sent
only when both are set. Keep-alive `mqtt.keepAliveS`. Socket timeout 5 s and
TCP connect 3 s, both independent of keep-alive. Reconnect back-off
2 s, 4 s, ... up to 60 s. Buffer 1280 B. LWT: `<main>status` = `offline`,
retained. After connecting, `online` (retained), subscriptions, then a full
publish. Every `publish()` result is checked; failures are counted in
`/api/status`.

`<main>` = (`pathAsRoot` ? `/` : "") + (station != "" ? station + `/` :
`VdMotFBH/`). `<V>`, `<T>`, `<S>` = item name with ' ' -> '_', or the 1-based
index when the name is empty (temps and volts use the config slot index).
Compat topics get `/value` appended when `separate` is on; commands are then
`<topic>/set` (and `<topic>/set/set` is also accepted for ioBroker).

### 10.1 Compat topics (exact legacy names and payloads)

| Topic (before suffix) | Payload | When |
|---|---|---|
| `<main>common/ip` | dotted IPv4 | first full publish after each (re)connect |
| `<main>common/state` | `ok`/`info`/`error` (plainText) or `0`/`1`/`2` | full publish; on change (`systemState`) |
| `<main>common/uptime` | `%ud %u:%02u:%02u` | if `upTime`; full publish; on-change mode at most every `minDelayS` |
| `<main>common/message` | message of the latest Warning+ event (`formatEventMessage`), "" if none | full publish; on change |
| `<main>valves/<V>/target` | desired target 0..100 | active valves with a known target |
| `<main>valves/<V>/state` | `valveStatusText` (plainText) or status number | active valves |
| `<main>valves/<V>/calibration/date` | `formatCalibDate` of the last observed calibration end of that valve | after the first observed end since ESP boot |
| `<main>valves/<V>/calibration/repetitions` | `calibRetries` | active valves |
| `<main>valves/<V>/diag/meanCurrrent` (sic) | mean current mA | if `diag` |
| `<main>valves/<V>/diag/openCount` | `formatLegacyCounter(oc)` | if `diag` |
| `<main>valves/<V>/diag/closeCount` | `formatLegacyCounter(cc)` | if `diag` |
| `<main>valves/<V>/diag/deadZoneCount` | dc (signed) | if `diag` |
| `<main>valves/<V>/diag/moves` | `formatLegacyCounter(moves)` | if `diag` |
| `<main>valves/<V>/temp1`, `temp2` | `formatTemp`: tenths + slot offset, `21.5` (`21,5` with germanDecimal) or `failed` | when the STM reports an assigned sensor (raw != -500) |
| `<main>valves/<V>/actual` (new, compat tree) | position 0..100 | active valves |
| `<main>temps/<T>/id` | 1-Wire id | active slot with id; unassigned to a valve or `allTemps` |
| `<main>temps/<T>/value` | `formatTemp` or `failed` (not on bus / invalid / stale > 60 s) | same |
| `<main>sensors/<S>/id` | id | active volt slot with id |
| `<main>sensors/<S>/value` | `formatVolt((vad/100 + offset) * factor)` or `failed` | same |
| `<main>sensors/<S>/unit` | configured unit | same |

Retain = `mqtt.retained` for all compat topics. Cadence
(`PublishScheduler`): periodic mode publishes everything every
`publishIntervalS`. On-change mode publishes changed items at most every
`minDelayS` per item and at least every `publishIntervalS`, plus a full
publish every `publishIntervalS`. Slots: 0 common, 1..12 valves, 13..46
temps, 47..54 volts, 55 STM diagnostics. A full publish is spread over
several loop passes, with `loop()` called between valves, so incoming
commands are never starved.

Dropped (never published or subscribed; HA configs deleted once, section 11):
`common/heatControl`, `common/parkPosition`, `valves/<V>/tTarget`, `tValue`,
`control/*`, `window/*`.

### 10.2 Command topic

| Topic | Payload | Effect |
|---|---|---|
| `<main>valves/<V>/target` + (`/set` [`/set`] when separate; "" or `/set` otherwise) | integer 0..100, also `55.0`; `OPEN` = 100, `CLOSE` = 0 | `SetTarget` (source mqtt) if the valve is active |

Topic parsing follows `parseTargetCommandTopic`: segment max 10 chars, name
match first, then a strict 1..12 number. Rejected commands (unknown valve,
inactive, bad payload, queue full) raise `MqttCommandRejected` and are counted.
With MQTT + HA, `homeassistant/status` = `online` triggers a discovery
re-send.

### 10.3 New topics (never suffixed)

| Topic | Payload | Retain | When |
|---|---|---|---|
| `<main>status` | `online` / `offline` (LWT) | always | connect / LWT |
| `<main>diag/valves/<V>/lastMove` | `{"dir":"open","req":N,"cnt":N,"stop":"endstop","peak":N,"ms":N}` (peak in 0.1 mA) | `retained` | v2, when `moveSeq` changes |
| `<main>diag/valves/<V>/earlyStops` | int | `retained` | v2, on change |
| `<main>diag/valves/<V>/cmdRejected` | int | `retained` | v2, on change |
| `<main>diag/valves/<V>/calState` | 0 idle, 1 started, 2 in progress | `retained` | v2, on change |
| `<main>diag/valves/<V>/profile` | `writeProfileJson` | never | v2, when a new `gprof` arrives |
| `<main>diag/stm/proto` | 1 / 2 | `retained` | on change |
| `<main>diag/stm/uptime` | seconds | `retained` | v2, every full publish |
| `<main>diag/stm/resets` | int | `retained` | v2, on change |
| `<main>diag/stm/rxOverflow` | int (STM counter) | `retained` | v2, on change |
| `<main>diag/stm/parseErr` | int (STM counter) | `retained` | v2, on change |
| `<main>diag/stm/link` | `linkStateName` | `retained` | on change |
| `<main>diag/calibration/active` | `0` / `1` | always | on change (any valve calibrating) |
| `<main>events` | `writeEventJson` | never | Warning+ and calibration outcomes, `EventRateLimiter` (1 per valve+code per 10 min, 30/h) |

`newDiag` off suppresses `diag/*`. `events` off suppresses `events`.

## 11. HA discovery

Runs only in mode MQTT + HA. It is sent on every connect when
`haDiscoveryOnConnect` is on, on `homeassistant/status` = `online`, and on
request (`POST /api/mqtt/discovery`). The MQTT task sends one message per
loop pass, 20 ms apart, calling `loop()` in between. The first run on a device
(`vdmrev/haDrop` != 1) first walks `DropListIterator` and the stale lines of
`/HADiscovery.cfg`, then sets `haDrop`.

Topic: `homeassistant/<component>/<station>/<objectId>/config`, retained.
Common payload parts: `name`, `unique_id`, `state_topic`, `command_topic`
(when commandable), `availability_topic` = `<main>status` with `online` and
`offline`, and `device` = {identifiers: station, name: station, sw_version:
firmwareVersion(), hw_version "2.0", model "VdMot Revamped", manufacturer
"Lenti84/Surfgargano", configuration_url `http://<ip>/`}. Everything is
JSON-escaped.

KEEP entities, with legacy object id, name and unique_id (`<st>` = station,
`<R>` = valve segment; unique_id has ' ' -> '_'):

| Component | objectId | name | unique_id | state / command | Extra | Gate |
|---|---|---|---|---|---|---|
| text | `state` | `state` | `<st>.common.state` | `common/state/value` / `.../set` | icon mdi:state-machine | always |
| text | `message` | `message` | `<st>.common.message` | `common/message/value` / `.../set` | mdi:message | always |
| text | `uptime` | `uptime` | `<st>.common.uptime` | `common/uptime/value` / `.../set` | mdi:timelapse | `upTime` |
| text | `ip` | `ip` | `<st>.common.ip` | `common/ip/value` / `.../set` | mdi:message | always |
| text | `valves_state_<R>` | `valves.<R>.state` | `<st>.valves.<R>.state` | `valves/<R>/state/value` / `.../set` | mdi:state-machine | active |
| valve | `valves_target_<R>` | `valves.<R>.target` | `<st>.valves.<R>.target` | `valves/<R>/target/value` / `.../set` | `reports_position: true`, mdi:valve, no state_class | active |
| sensor | `valves_temp1_<R>`, `valves_temp2_<R>` | `valves.<R>.temp1/2` | `<st>.valves.<R>.temp1/2` | `valves/<R>/temp1/value` | temperature, measurement, °C | sensor assigned |
| text | `valves_calibration_date_<R>` | `valves.<R>.calibration.date` | `<st>.valves.<R>.calibration.date` | `.../calibration/date/value` / `.../set` | mdi:timelapse | active |
| text | `valves_calibration_repetitions_<R>` | `valves.<R>.calibration.repetitions` | `<st>.valves.<R>.calibration.repetitions` | ... | mdi:valve | active |
| text | `valves_diag_{openCount,closeCount,deadZoneCount,moves,meanCurrrent}_<R>` | `valves.<R>.diag.<x>` | `<st>.valves.<R>.diag.<x>` | `valves/<R>/diag/<x>/value` / `.../set` | mdi:valve | `diag` |
| sensor | `temps_<T>` | `temps.<T>` | `<st>.<1-Wire id>` | `temps/<T>/value/value` | temperature, measurement, °C, mdi:thermometer | published temp slot |
| sensor | `volts_<S>` | `volts.<name>` | `<st>.<1-Wire id>` | `sensors/<S>/value/value` | unit = configured unit; device_class voltage only when unit is V/mV | active volt slot |

The components stay as they were in the legacy firmware (text stays text), so
existing HA entity ids survive. The only changes from legacy are escaping,
availability, and dropping the invalid `device_class: volume` /
`state_class` on text, number and valve entities.

New entities (`newDiag`):

| Component | objectId | unique_id | State topic |
|---|---|---|---|
| sensor | `valves_actual_<R>` | `<st>.valves.<R>.actual` | `valves/<R>/actual/value` (unit %) |
| sensor | `diag_earlyStops_<R>` | `<st>.diag.<R>.earlyStops` | `diag/valves/<R>/earlyStops` (v2) |
| sensor | `diag_cmdRejected_<R>` | `<st>.diag.<R>.cmdRejected` | `diag/valves/<R>/cmdRejected` (v2) |
| sensor | `diag_lastStop_<R>` | `<st>.diag.<R>.lastStop` | `diag/valves/<R>/lastMove`, `value_template: {{ value_json.stop }}` (v2) |
| sensor | `diag_stm_link` | `<st>.diag.stm.link` | `diag/stm/link` |
| sensor | `diag_stm_uptime` | `<st>.diag.stm.uptime` | `diag/stm/uptime` (v2, duration, s) |
| binary_sensor | `diag_calibration_active` | `<st>.diag.calibration.active` | `diag/calibration/active` (payload_on `1`) |

DROP entities deleted once (empty retained payload), for every valve index
and both segment forms: `climate/<st>/climate_<R>`,
`number/<st>/valves_control_{dynOffs,min,max}_<R>`,
`select/<st>/valves_window_state_<R>`, `switch/<st>/valves_window_state_<R>`,
`number/<st>/valves_window_target_<R>`, `select/<st>/heatControl`,
`number/<st>/parkPosition`.

## 12. HTTP API

Port 80. JSON bodies up to 4 KB (413 when larger, 409 while another body is
being received). Errors use `{"error":"<code>","detail":"..."}`. Auth is HTTP
Basic, checked with `checkBasicAuth`, and is enabled when `web.user` and
`web.password` are both set. 10 failures within 60 s lock auth for 60 s (429).
Read-only GETs are public unless `web.protectRead` is on. `/` and the static
assets are always public. Routing is `matchApiRoute`; `{n}` = valve 1..12.

| Method | Path | Auth | Request | Response |
|---|---|---|---|---|
| GET | `/`, `/index.html`, `/app.js`, `/app.css` | no | | gzip asset, ETag, 304 on If-None-Match |
| GET | `/api/status` | read | | `writeStatusJson` |
| GET | `/api/valves` | read | | `writeValvesJson` (always 12 entries) |
| POST | `/api/valves/{n}/target` | yes | `{"target":0..100}` | 202 `{"valve":n,"target":t}`; 400 range; 409 inactive; 503 queue full |
| POST | `/api/valves/{n}/calibrate` | yes | | 202 |
| POST | `/api/valves/{n}/assembly` | yes | | 202 |
| POST | `/api/valves/{n}/service-move` | yes | `{"dir":"open"\|"close","counts":1..10000,"maxmA":5..60}` | 202; 409 on a v1 STM |
| POST | `/api/valves/{n}/sensors` | yes | `{"slot1":0..34,"slot2":0..34}` (0 = none; slot must hold an id) | 202 (`stvls` + `masns` + `gvlon 255`) |
| GET | `/api/valves/{n}/profile` | read | | `writeProfileJson`; 404 when none |
| POST | `/api/valves/{n}/profile` | yes | | 202 (fresh `gprof`, v2) |
| POST | `/api/valves/calibrate` | yes | | 202 (`staln 255`) |
| POST | `/api/valves/assembly` | yes | | 202 (`staop 255`) |
| POST | `/api/valves/detect` | yes | | 202 (`stdet 255`) |
| GET | `/api/sensors` | read | | `writeSensorsJson` |
| POST | `/api/sensors/scan` | yes | | 202 (`stons`, then lists) |
| GET | `/api/events` | read | `since` (seq, default 0), `minSeverity` (name), `valve` (1..12), `limit` (1..50, default 50) | `writeEventsJson` |
| GET | `/api/config` | yes | | `writeConfigJson` (no secrets) |
| POST | `/api/config` | yes | partial config JSON (same structure), optional `"clearSecrets":true` | 200 new config; 400 `{"error":"invalid","detail":"<key path>"}` |
| GET | `/api/config/export` | yes | | same as GET config, as an attachment `vdmot-config.json` |
| GET | `/api/stm/motor` | read | | `writeMotorJson` |
| POST | `/api/stm/motor` | yes | `{"motor":{"lowC","highC","startOnPower","noOfMinCount","maxCalReps"},"learnMovements":n,"breakaway":{...}}` (each part optional) | 202; 400 range |
| POST | `/api/stm/reset` | yes | `{"confirm":true}` | 202 (NRST pulse, `StmResetByUser`) |
| GET | `/api/stm/images` | yes | | `[{"name","size","crc32","version"}]` |
| POST | `/api/stm/images` | yes | multipart, one file, <= 512 KiB | 201 image info; 413; 507 no space; 409 while flashing |
| DELETE | `/api/stm/images/{name}` | yes | | 204; 404; 409 while flashing |
| POST | `/api/stm/flash` | yes | `{"image":"x.bin","mode":"normal"\|"blank","force":false}` | 202; 409 busy; 400 invalid image (`validateImage` error name) |
| GET | `/api/stm/flash` | read | | `writeFlashStatusJson` |
| POST | `/api/stm/flash/abort` | yes | | 202 |
| POST | `/api/ota/esp` | yes | multipart firmware `.bin` (Content-Length required) | 200 then restart after 1 s; 400/500 with `ota::uploadError()` |
| POST | `/api/system/reboot` | yes | | 202, restart after 1 s (the STM is not reset) |
| POST | `/api/system/factory-reset` | yes | `{"confirm":"factory-reset"}` | 202, erase `vdmrev`, restart |
| POST | `/api/mqtt/reconnect` | yes | | 202 |
| POST | `/api/mqtt/discovery` | yes | `{"action":"publish"\|"delete"\|"republish"}` | 202; 409 when mode != MQTT + HA |
| GET | `/api/log` | yes | | text/plain: `events.1.log` then `events.log` (chunked) |

Uploads stream straight to LittleFS (`.part` file, the write result checked
on every chunk, free space checked up front) or to the OTA partition. They
never go through the JSON body buffer. Only one upload or flash runs at a
time.

## 13. Event codes

Numbers are external API: never renumber, only append. Severity is the
default from `eventDefaultSeverity`. "MQTT" means the event can reach
`<main>events` (Warning or above, or a calibration outcome).

| Code | Name | Severity | Valve | arg1 / arg2 / text |
|---|---|---|---|---|
| 100 | boot | Info (Warning for reset reasons panic, int_wdt, task_wdt, wdt, brownout) | - | reset reason / boot count / fw version |
| 101 | config_imported | Info | - | imported / rejected / first rejected key |
| 102 | config_saved | Info | - | revision / - / source |
| 103 | config_defaults | Error | - | reason / - / - |
| 104 | fs_formatted | Warning | - | -1 = format failed |
| 105 | esp_ota_started | Info | - | size |
| 106 | esp_ota_done | Info | - | size / - / version |
| 107 | esp_ota_failed | Error | - | Update error |
| 108 | app_marked_valid | Info | - | seconds after boot |
| 109 | reboot_requested | Info (Warning for 2 and 4) | - | reason 0 user, 1 ota, 2 net watchdog, 3 factory reset, 4 rollback |
| 110 | low_heap | Warning | - | free / min free |
| 111 | time_synced | Info (first), Debug after | - | step s |
| 112 | calib_time_missing | Warning | - | slot key |
| 200 | net_up | Info | - | 1 eth, 2 wifi / - / IP |
| 201 | net_down | Warning | - | interface |
| 202 | mqtt_connected | Info | - | |
| 203 | mqtt_disconnected | Warning | - | PubSubClient state |
| 204 | mqtt_command_rejected | Warning | - | valve 1-based / reason / text |
| 205 | ha_discovery_sent | Info | - | configs / deletes |
| 206 | auth_failed | Warning | - | failures in window / - / client IP |
| 300 | link_up | Info | - | |
| 301 | link_degraded | Info | - | consecutive timeouts |
| 302 | link_down | Error | - | consecutive timeouts |
| 303 | stm_reset_by_policy | Error | - | consecutive timeouts / span s |
| 304 | stm_reset_by_user | Info | - | |
| 305 | stm_reboot_detected | Warning | - | cause 1 uptime, 2 resets, 3 v1 heuristic, 4 link recovered |
| 306 | stm_version | Info | - | proto / hw id / version |
| 307 | stm_incompatible | Error | - | - / - / version |
| 308 | stm_rx_overflow | Warning | - | total / side 0 esp, 1 stm |
| 309 | stm_parse_errors | Warning | - | total / side |
| 310 | stm_queue_full | Warning | - | command |
| 311 | stm_flash_started | Info | - | size / - / image |
| 312 | stm_flash_done | Info | - | ms / - / new STM version |
| 313 | stm_flash_failed | Critical | - | FlashError / address / phase |
| 400 | target_set | Info | yes | target / TargetSource |
| 401 | valve_state_changed | Debug | yes | old / new status |
| 402 | valve_blocked | Error | yes | calibRetries |
| 403 | valve_failed | Error | yes | |
| 404 | valve_no_valve | Warning | yes | |
| 405 | valve_recovered | Info | yes | previous bad status |
| 406 | calib_started | Info | yes (or all) | 1 scheduled |
| 407 | calib_ok | Info (MQTT: calibration outcome) | yes | oc / cc |
| 408 | calib_retry | Warning | yes | calibRetries |
| 409 | calib_failed | Error | yes | calibRetries |
| 410 | early_stop | Warning | yes | total / stop reason |
| 411 | cmd_rejected | Warning | yes | total |
| 412 | target_not_confirmed | Warning | yes | desired / attempts |
| 413 | valve_stale | Warning | yes | seconds |
| 414 | service_move_done | Info | yes | counted / stop reason |
| 500 | temp_sensor_failed | Warning | - | slot / raw / id |
| 501 | temp_sensor_recovered | Info | - | slot |
| 502 | sensor_count_changed | Info | - | count / 0 temp, 1 volt |
| 503 | volt_sensor_failed | Warning | - | slot / raw |
| 600 | scheduled_calibration | Info | - | slot key / minutes late |

Event text line (`formatEventLine`): `2026-09-23T14:03:05Z WARNING early_stop v3 valve 3: early stop (total 2, endstop)`,
or `+123s` in place of the time before SNTP sync. Syslog: RFC 5424, facility
local0, app `vdmot`, msgid = code name. Syslog level 1 sends Warning+, level 2
Info+, level 3 everything.

## 14. Calibration schedule

`CalibScheduler` (app task, every 10 s) with `calib.dayMask`, `calib.hour`
and `calib.minute`. Grace window 120 min. A slot fires at most once per local
date (key yyyymmdd, persisted in `vdmrev/calSlot`), and the key only moves
forward. DST: spring forward fires at the first minute after the gap; fall
back fires once. Without valid time nothing fires; after 1 h of uptime without
time, `CalibTimeMissing` is raised once per boot. Firing sends
`Calibrate(kAllValves)` (`staln 255`; the STM runs one motor at a time),
stores `lastCal`, and logs `ScheduledCalibration`. Manual calibrations are not
tied to the schedule.

## 15. STM flasher

`StmFlasher` runs inside the stm task (`step()` every 2 ms). While it runs,
`LinkPolicy` is suspended and nothing else uses the UART. Sequence and timing
(`FlashOptions` defaults, binding):

1. Validating: `validateImage` (size 1..512 KiB, and within the detected chip's
   flash once the PID is known; SP in RAM; odd reset vector inside the image;
   `DEADBEEF` and `BEEFIT` strings present unless `force`; padded to a
   multiple of 4 with 0xFF; CRC32).
2. Resetting: UART 115200 8E1, NRST asserted 100 ms, released.
3. Handshake (normal mode): first `DEADBEEF` 20 ms after release, then every
   100 ms, for up to 2.5 s. `BEEFIT` is found with a sliding window. Blank
   mode (BOOT0 held by hand) skips this step.
4. Sync: wait 250 ms, then `0x7F`. ACK or NACK both count as synced (NACK
   means already synced). 3 attempts, 1 s each.
5. GetId `0x02`: the PID must be 0x423, 0x431 or 0x433 and match the image's
   SP/size, else UnknownChip / ImageChipMismatch.
6. Erasing: `0x44` with the sector list from `sectorsForImage` (F401/F411:
   16, 16, 16, 16, 64, 128, ... KiB). Timeout 60 s. No mass erase.
7. Writing: `0x31` 256-byte blocks from block 1 upwards, block 0 (the vector
   table) last. Every ACK is awaited with a 1 s timeout. A failed block is
   retried 3 times with command, address and data.
8. Verifying: `0x11` read-back of every block, byte compare. The first
   mismatch address is reported.
9. On a write or verify failure the whole erase, write and verify is
   repeated up to 2 times in the same ROM session, without a reset.
10. Starting: NRST pulse, UART 8N1.
11. WaitingApp: after 4 s, `gvers` every 1 s for up to 15 s. The reply must
    match the image's version string when one was found (else
    AppVersionMismatch). No reply gives AppNotResponding.

Percent (monotonic): validating to getid 0..5, erasing 5..15, writing 15..75
by bytes, verifying 75..95, starting and waiting 95..99, done 100. The legacy
status code (`legacyFlashStatus`): Idle 0; Validating to GetId 1; Erasing 3;
Writing 4; Verifying, Starting and WaitingApp 5; Done 6; Failed 8. The ESP
never restarts after flashing, whether it succeeds or fails. Afterwards the
link resumes with a 5 s hold-off and a full re-sync, and desired targets are
re-pushed. On success the image is copied to `/stm/last_good.bin`.

## 16. OTA and restart

- `verifyRollbackLater()` returns true, so a new image boots in
  PENDING_VERIFY. `OtaValidator`: it is marked valid after 120 s of network
  up and STM link Up without interruption, or at 10 min of uptime with the
  network up. If neither happens by 15 min, the ESP calls
  `esp_ota_mark_app_invalid_rollback_and_reboot()`.
- ESP OTA upload: `ota::uploadBegin`, `uploadWrite` and `uploadEnd`. There
  is one upload at a time, and none while an STM flash is running.
  Content-Length must match the bytes written. On success the ESP restarts
  1 s after the response.
- Restarts always go through `ota::requestRestart(reason, delay)`, which logs
  `RebootRequested`, flushes the log file and calls `esp_restart()`. The STM
  is never reset by an ESP restart.
- Network watchdog (legacy `netConnTO`): `reconnectTimeoutMin` minutes
  without an IP -> restart (reason 2). 0 disables it.
- Factory reset: hold GPIO2 low for 1 s at boot, or
  `POST /api/system/factory-reset`. It erases `vdmrev` only (the legacy
  namespaces stay, and `imported` is set so they are not imported again).

## 17. Tests and mutation

- `test/native`: `test_<module>.cpp` per core module, doctest, ASan/UBSan,
  `-Werror`. `test_smoke.cpp` covers linkage, version, number helpers, the
  JSON writer and config defaults.
- Mutation: `tools/mutation/esp32.json` (added in the CI PR) lists every
  `lib/core/src/*.cpp`. The threshold is 85 %. Surviving mutants are either
  killed with new tests or documented as equivalent with a reason.
- Glue: compile-checked by `pio run` for both envs. On-device behaviour is
  verified with the manual checklist in `docs/revamped/INSTALL.md` (CI PR).

Implementation state of this skeleton: `common`, `version` and `json_writer`
are implemented. `config` has real defaults (`setDefaults`), and
`writeErrorJson` is real. Every other core function is a stub that returns
the documented failure value (false, 0, empty, None) until its module is
implemented. The glue is wired end to end: boot, tasks, TWDT, storage and
import path, logger with file and syslog sinks, net, UART task loop, MQTT
connect/LWT/commands, HTTP static assets, `/api/status`, reboot and MQTT
reconnect. Other API routes answer 501 until the web module is implemented.
