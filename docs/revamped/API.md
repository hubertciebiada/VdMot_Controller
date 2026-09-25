# VdMot Revamped: HTTP API

The dashboard uses this JSON API; scripts can use it too. Base URL
`http://<device>/`, port 80. Source of truth:
`software_esp32_revamped/lib/core/src/json_api.cpp` (routes, documents),
`web_guard.cpp` (request guard), `legacy_http.cpp` (legacy aliases) and
`src/web_server.cpp` (handlers). Event codes: DESIGN.md section 13.

## Security

Every request to `/api/*` and to the legacy aliases passes a guard before
anything else (static files and `410` answers are not checked). The first
failing check answers:

| Check | Rule | Answer |
|---|---|---|
| Host | absent is fine. Port and a trailing `.` are ignored. An IPv4 must be the address the request came in on, the device's IP, or an IPv4 listed in `web.allowedHosts`; a name must be the device's host name (`<name>` or `<name>.local`, case-insensitive) or a name listed in `web.allowedHosts`. IPv6 literals fail | `403 host_not_allowed`, detail `<host>: use <device IP> or add the name to web.allowedHosts` |
| Origin | absent is fine; `null` fails; `http(s)://<host>` must pass the Host rule | `403 origin_not_allowed` |
| X-VdMot | every POST and DELETE on `/api/*` (uploads included) needs the header `X-VdMot: 1`. Not needed for GET/HEAD and for `POST /setvalve` | `403 header_required` |
| Content-Type | a POST with a body that is not an upload must be `application/json`; uploads must be `multipart/form-data` | `415 unsupported_media_type` |

The rules block DNS rebinding and cross-site requests from a browser: a web
page cannot set `X-VdMot` without a CORS preflight, which the device never
grants. Refusals are logged (`request_refused`, at most once per kind per
minute).

`web.allowedHosts` (Settings → Web): up to 4 host names or IPv4 addresses,
comma-separated, for names under which the device is reached besides its own
host name (e.g. a router DNS name `vdmot.fritz.box`).

Scripts and Home Assistant `rest_command`s must send the header, e.g.
```yaml
rest_command:
  vdmot_target:
    url: "http://192.168.1.51/api/valves/{{ valve }}/target"
    method: POST
    headers:
      X-VdMot: "1"
    content_type: "application/json"
    payload: '{"target": {{ target }}}'
```

## Authentication

- HTTP Basic auth, enabled when **both** `web.user` and `web.password` are set
  (Settings → Web). Without them every endpoint is open (as in the legacy
  firmware).
- With auth enabled, every change (POST/DELETE) and the config, image list,
  file list and log need credentials. The read-only GETs `status`, `valves`,
  `valves/{n}/profile`, `sensors`, `events`, `stm/motor`, `stm/flash` and
  `import-report` stay public unless `web.protectRead` is on. `GET /api/health`
  and the dashboard files (`/`, `/app.js`, `/app.css`) are always public.
- Wrong or missing credentials: `401` with `WWW-Authenticate: Basic realm="VdMot"`;
  failures are logged (`auth_failed`).
- Lockout per client address (8 addresses tracked): 10 failures within 60 s
  lock that address for 1 min, the next lockout 5 min, every later one 15 min;
  a successful login forgets the address. Locked: `429` with `Retry-After: <s>`
  and `{"error":"locked","detail":"too many failed logins from this address, retry in <n> s"}`;
  event `auth_locked`.

## Conventions

- Request bodies are JSON objects, at most 8 KB (`413 too_large` otherwise).
  Unknown keys are rejected where a handler lists its keys.
- `{n}` = valve number 1..12. Paths have no trailing `/`.
- Errors: `{"error":"<code>","detail":"<text>"}`.
- Actions that go to the STM answer `202 {"result":"queued"}`; the effect shows
  up in later reads and in the event log.
- Targets are JSON numbers 0..100; a fraction is rounded half up (`43.5` ->
  44); a value below 0 or above 100 (also `100.4`), a string or a bool is
  `400 out_of_range`.

Error codes:

| Status | error | When |
|---|---|---|
| 400 | `bad_request`, `invalid` (detail = key path), `out_of_range`, `unknown_key`, `confirm_required`, `invalid_image`, `bad_path`, `bad_name` | invalid request |
| 401 | – | credentials needed |
| 403 | `host_not_allowed`, `origin_not_allowed`, `header_required` | guard |
| 403 | `auth_required` | `?secrets=1` without web login |
| 403 | `protected` | file that cannot be deleted |
| 404 | `not_found` | unknown path, image, file, profile, import report |
| 405 | `method_not_allowed` | wrong method |
| 409 | `inactive` | target for an inactive valve |
| 409 | `unsupported` | needs STM protocol 2 |
| 409 | `stm_unsupported` | STM below 1.4.0 (every STM action except target, reset and flash), or stop / safe mode below protocol 3 |
| 409 | `flashing`, `busy`, `restarting`, `validating`, `idle` | STM flash, upload or restart state |
| 409 | `board_mismatch`, `board_required` | STM image for another board revision |
| 409 | `unknown` | motor or breakaway settings not read from the STM yet: send all values |
| 409 | `no_trial` | no network trial running |
| 409 | `disabled`, `separate_required` | MQTT discovery not possible |
| 410 | `gone` | legacy path; detail = the replacement |
| 411 | `length_required` | upload without Content-Length |
| 413 | `too_large` | body or file too large |
| 415 | `unsupported_media_type` | wrong Content-Type |
| 429 | `locked` | login lockout |
| 500 | `internal`, `io_error`, `nvs` | device error |
| 503 | `busy` | both response buffers in use (nothing applied); retry |
| 503 | `queue_full` | STM command queue full |
| 503 | `retry` | state changed while answering; retry |
| 503 | `unavailable` | no file system |

## Endpoints

### Status and data

| Method | Path | Auth | Response |
|---|---|---|---|
| GET | `/api/health` | never | health document (below) |
| GET | `/api/status` | read | status document (below) |
| GET | `/api/valves` | read | `{"valves":[...]}`, always 12 entries (below) |
| GET | `/api/valves/{n}/profile` | read | `{"valve":n,"count":k,"samples":[[count,current_0.1mA],...]}`; `404` when none |
| GET | `/api/sensors` | read | `{"temps":[...],"volts":[...]}`: per slot `slot`, `name`, `id`, `active`, `onBus`, `temp` (temps, °C incl. offset, null = failed) or `value` (volts, `(raw/100 + offset) × factor`, null = failed), `raw`, `age`, `valve` |
| GET | `/api/events` | read | query `since` (seq, default 0), `minSeverity` (`debug`/`info`/`warning`/`error`/`critical`), `valve` (1..12), `limit` (1..50, default 50). Response `{"first","last","next","dropped","events":[...]}`; pass `next` as `since` to poll |
| GET | `/api/stm/motor` | read | `{"motor":{"lowC","highC","startOnPower","noOfMinCount","maxCalReps"},"learnMovements":n,"breakaway":{...}\|null,"known":bool}` |
| GET | `/api/stm/flash` | read | flasher state (below) |
| GET | `/api/import-report` | read | legacy import report (below); `404` when there is none |
| GET | `/api/log` | yes | `text/plain`, previous + current log file (the log is flushed first) |

`/api/status`:
- `station` (first member);
- `esp`: `version`, `build`, `uptime`, `resetReason`, `boots`, `heap`
  (`free`, `min`, `largest`), `flash` (`used`, `size`);
- `time`: `valid`, `epoch`, `local`, `lastSync`;
- `net`: `state`, `ip`, `mask`, `gw`, `dns`, `mac`, `rssi`, `hostname`,
  `trial` (`null` or `{"remainS":n}` while a network change is on trial);
- `mqtt`: `state`, `rc`, `reconnects`, `publishFailures`, `clientId`,
  `haStatus` (`unknown`/`online`/`offline`);
- `stm`: `link`, `proto`, `version`, `build`, `hwId`, `chip`, `compatible`,
  `minVersion`, `support` (`unknown`/`ok`/`too_old`), link statistics, `status`
  (`gstat` values; with protocol 3 also `lease`, `leaseRemainS`, `leaseClient`,
  `leaseTimeoutMin`, `failsafeMask`, `safeMode`, `wdgResets`, `uartOre`,
  `uartFe`, `uartNe`, `rxDropped`, `cfgFlags` (names), `cfgEvents`,
  `eepWrites`, `tempAgeS`, `owScanAgeS`, `protectSuspended`), `espRx`
  (`overflow`, `malformed`), `lease` (`null` or `{"mode":"stm|esp","state":"off|running|expired","remainS","timeoutMin","failsafeMask","regulator":"alive|broker_down|ha_offline","regulatorLostS","configSynced","configFailed","configTrusted"}`),
  `learnTime` (STM time trigger in s, or null);
- `calibration`: `active`, `lastScheduled`, `nextSlot`, `next` (local ISO time
  or null);
- `auth`, `lastEventSeq`;
- `config`: `{"source":"stored|imported|defaults|defaults_after_error|backup","repairs":<mask>,"newerSchema":bool}`;
- `importReport`: true while a legacy import report is stored.

`/api/health` (for monitors; the OTA self-check uses it too):
```json
{"ok":true,"version":"2.1.0-revamped","uptime":1234,
 "heap":{"free":142336,"min":118420,"largest":90100,"minLargest":65536},
 "tasks":[{"name":"stm","stack":6144,"minFree":2100},{"name":"app","stack":8192,"minFree":3200}],
 "net":{"ip":true,"reachable":true,"proven":true,"pingArmed":true,"evidence":"ping","evidenceAgeS":12,"ifaceRestarts":0,"trial":null},
 "ota":null,
 "log":{"persist":true,"backlog":3,"flushes":12,"lastFlushAgeS":40,"lost":0,"failures":0}}
```
`ota` while a new ESP image waits for validation:
`{"stmRequired":bool,"checks":{"net":b,"http":b,"stm":b},"healthyForS":n,"remainS":n}`
(`remainS` = time left before the rollback).

Valve entry (`/api/valves`):
```json
{"idx":1,"name":"KorWitryna","active":true,"known":true,"state":1,"stateKey":"idle",
 "calibrating":false,"pos":15,"target":15,"targetSource":"mqtt","sync":"synced","stmTarget":15,
 "meanCur":16,"moves":3937,"oc":408,"cc":445,"dc":37,"cr":0,"health":[],"age":3,
 "sensors":[{"sensor":1,"slot":1,"name":"KorWitryna","temp":20.9}],
 "ext":{"calState":0,"calEarlyStop":false,"calLastFailed":false,"earlyStops":0,"cmdRejected":0,
        "lastMove":{"dir":"open","req":408,"cnt":395,"stop":"endstop","peak":29.9,"ms":15784},"moveSeq":1,
        "v3":{"flags":["calRestored"],"fault":"none","drive":15,"retryS":0,"retries":0}},
 "failsafe":{"state":"off","pct":50},
 "calibrationEnd":"2026-09-23T03:12:40"}
```
- `target` / `stmTarget`: null while unknown.
- `targetSource`: `none`, `stm`, `web`, `mqtt`, `restored` (from RTC/NVS after
  a restart), `assembly`.
- `sync` (target delivery): `unknown`, `synced`, `pending`, `await_ack`,
  `await_verify`, `failed`.
- `health`: `blocked`, `failed`, `noValve`, `calibRetries`, `earlyStop`,
  `cmdRejected`, `stale`, `targetUnconfirmed`, `tempFailed`, `failsafe`,
  `strokeShort` (a calibration stroke close to minCounts).
- `sensors`: `sensor` 1 or 2 (temp1/temp2), `slot` = config slot, `temp` null
  when failed.
- `ext` is null with a protocol 1 STM; `ext.v3` is null below protocol 3
  (flag and fault names: PROTOCOL_V2.md). `lastMove.peak` is in mA here (MQTT
  uses 0.1 mA).
- `failsafe.state`: `off`, `lease` (regulator silent), `blocked`; `pct` null =
  hold.
- `calibrationEnd`: end of the last calibration seen since the ESP booted, or
  null.

Flasher state (`GET /api/stm/flash`): `phase`, `status` (legacy code),
`percent`, `bytesDone`, `bytesTotal`, `bootloaderVersion`, `attempt`,
`error` (`{"code","phase","addr"}` or null), `startedMs`, `finishedMs`, `image`
(`name`, `size`, `crc32`, `version`, `hw`), `appVersion`, `board`
(`ok`/`untagged`/`mismatch`/`board_required`), `boardHw` (board of this
controller), `manualReset` (blank mode done: remove BOOT0, then reset the
STM), `baud`, `pending` (the flash waits for the STM EEPROM).

Import report (`GET /api/import-report`), written once by the legacy import:
```json
{"imported":57,"rejected":2,"ignored":14,"firstRejected":"valvesCfg/valves.5.name",
 "piValves":3,"windowValves":1,"dropped":["pi","window","messenger","ds18Timeout"],
 "legacyFailsafe":{"enabled":true,"timeoutMin":120,"pct":10},
 "rootTopic":"VdMotFBH",
 "renamed":[{"kind":"valve","n":3,"name":"Bad_WC","topic":"Bad/WC"}],
 "syslogDebug":true,"voltsBlob448":false}
```
`legacyFailsafe` shows the legacy MQTT timeout of PI valves; it is **not**
imported (the new failsafe starts with 60 min / 50 %).

### Valve actions

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/valves/{n}/target` | `{"target":0..100}` | `202 {"valve":n,"target":t}` (t rounded); `409 inactive` |
| POST | `/api/valves/{n}/stop` | – | 202; stops the valve where it is (STM 2.1) |
| POST | `/api/valves/stop` | – | 202; stops every valve (STM 2.1) |
| POST | `/api/valves/{n}/calibrate` | – | 202 |
| POST | `/api/valves/{n}/assembly` | – | 202 (fully open, target 100 until the next target; excluded from the failsafe) |
| POST | `/api/valves/{n}/service-move` | `{"dir":"open"\|"close","counts":1..10000,"maxmA":5..60}` | 202; `409 unsupported` on a protocol 1 STM |
| POST | `/api/valves/{n}/sensors` | `{"slot1":0..34,"slot2":0..34}` (0 = none; the slot must hold a sensor id) | 202 |
| POST | `/api/valves/{n}/profile` | – | 202, reads a fresh profile (protocol 2+) |
| POST | `/api/valves/calibrate` | – | 202, all valves |
| POST | `/api/valves/assembly` | – | 202, all valves |
| POST | `/api/valves/detect` | – | 202, valve detection |
| POST | `/api/sensors/scan` | – | 202, 1-Wire bus scan |
| POST | `/api/stm/safe-mode/leave` | – | 202 (STM 2.1) |

Calibrate, assembly, service move and profile are accepted for inactive valves
too. Stop and safe mode answer `409 stm_unsupported` below protocol 3. While the
failsafe is active, a target is stored and applied once the regulator is back;
to open a valve at once use assembly or change its failsafe position.

### Configuration

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/config` | – | full config, secrets replaced by `wifiPasswordSet` / `passwordSet` flags |
| POST | `/api/config` | partial config with the same structure, optional `"clearSecrets":true` | 200 with the new config plus `"restartRequired":bool,"netTrial":bool`; `400 {"error":"invalid","detail":"<key path>"}` |
| POST | `/api/config?dryRun=1` | same | 200 `{"restartRequired":bool,"netTrial":bool}`, nothing saved |
| GET | `/api/config/export` | – | config as download `vdmot-config.json` (no secrets) |
| GET | `/api/config/export?secrets=1` | – | with passwords, `vdmot-config-secrets.json`; only with web login enabled (`403 auth_required` otherwise) |
| POST | `/api/stm/motor` | any of `motor` (all five values unless read already), `learnMovements` (0 or 50..65534), `breakaway` `{enable,stepPct 0..100,maxmA 20..60}` (protocol 2+) | 202 |
| POST | `/api/system/network/confirm` | – | 202: keep the network settings on trial; `409 no_trial` |
| POST | `/api/system/network/revert` | – | 202: go back to the previous network settings now; `409 no_trial` |

An empty secret in `POST /api/config` keeps the stored one. The save is
validated first (for example: mode MQTT + HA needs `separate` and no
`germanDecimal`; valve topic segments and their HA ids must be unique). When
`restartRequired` is true the ESP restarts after the response: network
settings that are in use, or a station name that changes the host name. With
`netTrial` the new network settings run on trial: confirm them within 2 min
(dashboard "Keep these settings" or the confirm route), otherwise the ESP goes
back to the old ones. With jumper X20 an ESP restart also resets the STM
(INSTALL.md). MQTT settings reconnect MQTT; the failsafe settings are pushed to
the STM at once.

New config keys in 2.1: `web.allowedHosts`, `mqtt.rootTopic`, `mqtt.clientId`,
`mqtt.discoveryPrefix`, `failsafe.timeoutMin` (0 = off, or 5..1440, default 60),
`valves.N.failsafePct` (0..100, 255 = hold, default 50), `valves.N.topic`,
`temps.N.topic`, `volts.N.topic` (MQTT topic overrides, MQTT.md). The JSON
`schema` is 2; an ESP 2.0.0 after a downgrade still reads the settings it
knows.

Example:
```sh
curl -u admin:secret -X POST http://vdmot/api/config \
  -H 'X-VdMot: 1' -H 'Content-Type: application/json' \
  -d '{"calib":{"dayMask":9,"hour":3,"minute":15}}'
```

### STM firmware

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/stm/images` | – | stored images `[{"name","size","crc32","version","check","hw"}]`; `check` = validation result (`none` = ok, null while not checked yet), `hw` = board marker `C1`/`C2` or null |
| POST | `/api/stm/images` | multipart, one `.bin` file, <= 512 KiB | `201` image info; `413`; `507` no space or too many images; `409` while flashing |
| DELETE | `/api/stm/images/{name}` | – | `204`; `404`; `409` |
| POST | `/api/stm/flash` | `{"image":"<name>","mode":"normal"\|"blank","force":false,"board":"C1"\|"C2"}` | 202; `400 invalid_image` with the validation error; `409 board_mismatch` / `board_required`; `409` busy or restart pending |
| POST | `/api/stm/flash/abort` | – | 202 |
| POST | `/api/stm/reset` | `{"confirm":true}` | 202, pulses the STM reset after the STM stored its EEPROM (at most 10 s) |

Board check: an image with a `VDM-HW:C1`/`C2` marker must match the board of
this controller: the tag of the running STM, else `board`. Blank mode (STM in
its ROM bootloader, BOOT0 held) cannot ask the STM, so `board` is required
there. Images without a marker (1.x, 2.0.0) flash with a warning. `force:true`
overrides the check for the update handshake strings and the board checks.
After a blank-mode flash `manualReset` is true: remove BOOT0, then reset the
STM.

### ESP firmware, files and system

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/ota/esp` | multipart ESP32 application image; optional MD5 as query `md5`, form field `md5`, or header `X-Update-MD5` | `200 {"result":"ok","restart":true}`, restart after 1 s; error with the reason otherwise |
| POST | `/api/system/reboot` | – | 202, ESP restart after 1 s (after the STM stored its EEPROM) |
| POST | `/api/system/factory-reset` | `{"confirm":"factory-reset"}` | 202, erases the new firmware's settings and restarts |
| GET | `/api/files` | – | `{"total":B,"used":B,"truncated":bool,"files":[{"path","size","kind","deletable"}]}` (LittleFS) |
| DELETE | `/api/files?path=<path>` | – | `204`; `400 bad_path`; `403 protected`; `404`; `500 io_error` |
| DELETE | `/api/import-report` | – | `204` / `404`: dismisses the import report |
| POST | `/api/mqtt/reconnect` | – | 202 |
| POST | `/api/mqtt/discovery` | `{"action":"publish"\|"delete"\|"republish"}` | 202 (modes MQTT and MQTT + HA); `409 disabled` when MQTT is off; `409 separate_required` for publish/republish without `mqtt.separate` |

Only one upload or STM flash runs at a time; no ESP update is accepted during an
STM flash, and a second ESP update is refused while the running image still
waits for its validation (`GET /api/health` → `ota.remainS`).

```sh
# ESP update from the command line
curl -u admin:secret -H 'X-VdMot: 1' -F "file=@VdMot-Revamped_2.1.0-revamped_ESP32-WT32-ETH01.bin" \
  "http://vdmot/api/ota/esp?md5=$(md5sum VdMot-Revamped_2.1.0-revamped_ESP32-WT32-ETH01.bin | cut -d' ' -f1)"
```

## Legacy endpoints

Scripts written for the legacy firmware keep a few endpoints (guarded like the
API; `POST /setvalve` without `X-VdMot`):

| Method | Path | Response |
|---|---|---|
| GET | `/valves` | legacy valve list `{"valves":[{"idx","name","state","pos","meanCur","targetPos","link","moves","oc","cc","dc","cr","tIdxName1","temp1",...,"controlActive":0}]}` (valves with data only) |
| GET | `/temps` | `[{"id","name","temp"}]` of the published temperature slots |
| GET | `/volts` | `[{"id","name","unit","value"}]` of the active voltage slots |
| POST | `/setvalve` | `{"valve":1..12,"value":<number>}` (`Content-Type: application/json`) → `200 {"res":"ok"}` |

The other legacy paths answer `410 {"error":"gone","detail":"<replacement>"}`
for any method: `/netinfo`, `/sysinfo`, `/sysdyninfo`, `/update/identity` →
`/api/status`; `/netconfig`, `/protconfig`, `/valvesconfig`, `/tempsconfig`,
`/voltsconfig`, `/sysconfig`, `/sysLogCfg` → `/api/config`; `/motorconfig` →
`/api/stm/motor`; `/tempsensorsid`, `/voltsensorsid` → `/api/sensors`; `/fsdir` →
`/api/files`; `/fupload` → `/api/stm/images`; `/stmupdate` → dashboard;
`/stmupdstatus`, `/stmdoupdate` → `/api/stm/flash`; `/update` → `/api/ota/esp`;
`/cmd` → the matching `/api` action; `/valvesctrlconfig`, `/msgconfig`,
`/testPO`, `/testEmail`, `/ssidinfo`, `/auth` → removed.

## Development mock

`software_esp32_revamped/tools/mock_api.py` serves the dashboard with a
simulated API (standard library only) that follows the firmware, including the
request guard; `tools/test_mock_api.py` checks it.

```sh
python3 software_esp32_revamped/tools/mock_api.py --port 8080            # protocol 2 STM
python3 software_esp32_revamped/tools/mock_api.py --proto 1              # legacy STM 1.4.x
python3 software_esp32_revamped/tools/mock_api.py --proto 3              # STM 2.1: failsafe lease, stop, safe mode
python3 software_esp32_revamped/tools/mock_api.py --auth admin:secret    # with Basic auth
python3 software_esp32_revamped/tools/mock_api.py --scenario health,busy,queue,failsafe,safemode,tooold,haoffline
python3 software_esp32_revamped/tools/mock_api.py --import-report        # a legacy import report
python3 software_esp32_revamped/tools/mock_api.py --station west --station-name "Dom Północ"
```

Scenarios: `health` (stale, unconfirmed target, failed temperature), `busy`
(503 busy), `queue` (every second valve action answers 503 queue_full), `failsafe` (lease expired), `safemode`,
`tooold` (STM below 1.4.0), `haoffline` (HA reports offline).
