# VdMot Revamped: HTTP API

The dashboard uses this JSON API; scripts can use it too. Base URL
`http://<device>/`, port 80. Source of truth:
`software_esp32_revamped/lib/core/src/json_api.cpp` (routes, documents) and
`src/web_server.cpp` (handlers). The legacy web endpoints (`/valves`,
`/update`, `/stmupdate`, ...) do not exist in the new firmware.

## Authentication

- HTTP Basic auth, enabled when **both** `web.user` and `web.password` are set
  (Settings → Web). Without them every endpoint is open (as in the legacy firmware).
- With auth enabled, every change (POST/DELETE) and the config, image and log
  downloads need credentials. The read-only GETs `status`, `valves`,
  `valves/{n}/profile`, `sensors`, `events`, `stm/motor` and `stm/flash` stay
  public unless `web.protectRead` is on. The dashboard files (`/`, `/app.js`,
  `/app.css`) are always public.
- 10 failed logins within 60 s lock authentication for 60 s: every request that
  needs auth then gets `429`. Failures are logged (`auth_failed`).
- Wrong or missing credentials: `401` with `WWW-Authenticate: Basic realm="VdMot"`.

## Conventions

- Request bodies are JSON objects, at most 4 KB (`413` otherwise); only one
  body is received at a time (`409` `busy`). Unknown keys are rejected where a
  handler lists its keys.
- `{n}` = valve number 1..12. Paths have no trailing `/`.
- Errors: `{"error":"<code>","detail":"<text>"}`, for example `400 out_of_range`,
  `409 inactive`, `409 unsupported` (needs STM protocol v2), `409 flashing`,
  `503 queue_full` (STM command queue full), `503 busy` (response buffers in use).
- Actions that go to the STM answer `202 {"result":"queued"}`; the effect shows
  up in later reads and in the event log.

## Endpoints

### Status and data

| Method | Path | Auth | Response |
|---|---|---|---|
| GET | `/api/status` | read | `esp` (version, uptime, reset reason, boots, heap, flash), `time`, `net`, `mqtt` (state, reconnects, publish failures), `stm` (link, proto, version, chip, link statistics, `gstat` values), `calibration` (active, last scheduled, next slot), `auth`, `lastEventSeq` |
| GET | `/api/valves` | read | `{"valves":[...]}`, always 12 entries (below) |
| GET | `/api/valves/{n}/profile` | read | `{"valve":n,"count":k,"samples":[[count,current_0.1mA],...]}`; `404` when none |
| GET | `/api/sensors` | read | `{"temps":[...],"volts":[...]}`: slot, name, id, active, onBus, value, raw, age, assigned valve |
| GET | `/api/events` | read | query `since` (seq, default 0), `minSeverity` (`debug`/`info`/`warning`/`error`/`critical`), `valve` (1..12), `limit` (1..50, default 50). Response `{"first","last","next","dropped","events":[...]}`; pass `next` as `since` to poll |
| GET | `/api/stm/motor` | read | `{"motor":{"lowC","highC","startOnPower","noOfMinCount","maxCalReps"},"learnMovements":n,"breakaway":{...}\|null,"known":bool}` |
| GET | `/api/stm/flash` | read | flasher state: `phase`, `status` (legacy code), `percent`, `bytesDone/Total`, `chipId`, `chipName`, `error`, `image`, `appVersion` |
| GET | `/api/log` | yes | `text/plain`, previous + current log file |

Valve entry (`/api/valves`):
```json
{"idx":1,"name":"KorWitryna","active":true,"known":true,"state":1,"stateKey":"idle",
 "calibrating":false,"pos":15,"target":15,"targetSource":"mqtt","sync":"synced","stmTarget":15,
 "meanCur":16,"moves":3937,"oc":408,"cc":445,"dc":37,"cr":0,"health":[],"age":3,
 "sensors":[{"slot":1,"name":"KorWitryna","temp":20.9}],
 "ext":{"calState":0,"calEarlyStop":false,"calLastFailed":false,"earlyStops":0,"cmdRejected":0,
        "lastMove":{"dir":"open","req":408,"cnt":395,"stop":"endstop","peak":29.9,"ms":15784},"moveSeq":1}}
```
`ext` is `null` with a protocol v1 STM. Here `lastMove.peak` is in mA (MQTT
uses 0.1 mA). `sync` (target delivery): `unknown`, `synced`, `pending`, `await_ack`,
`await_verify`, `failed`. `health` lists the active flags: `blocked`,
`failed`, `noValve`, `calibRetries`, `earlyStop`, `cmdRejected`, `stale`,
`targetUnconfirmed`, `tempFailed`.

### Valve actions

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/valves/{n}/target` | `{"target":0..100}` | `202 {"valve":n,"target":t}`; `409 inactive` |
| POST | `/api/valves/{n}/calibrate` | – | 202 |
| POST | `/api/valves/{n}/assembly` | – | 202 (assembly position, fully open) |
| POST | `/api/valves/{n}/service-move` | `{"dir":"open"\|"close","counts":1..10000,"maxmA":5..60}` | 202; `409 unsupported` on a v1 STM |
| POST | `/api/valves/{n}/sensors` | `{"slot1":0..34,"slot2":0..34}` (0 = none; the slot must hold a sensor id) | 202 |
| POST | `/api/valves/{n}/profile` | – | 202, reads a fresh profile (v2) |
| POST | `/api/valves/calibrate` | – | 202, all valves |
| POST | `/api/valves/assembly` | – | 202, all valves |
| POST | `/api/valves/detect` | – | 202, valve detection |
| POST | `/api/sensors/scan` | – | 202, 1-Wire bus scan |

### Configuration

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/config` | – | full config, secrets replaced by `wifiPasswordSet` / `passwordSet` flags |
| POST | `/api/config` | partial config with the same structure, optional `"clearSecrets":true` | 200 with the new config; `400 {"error":"invalid","detail":"<key path>"}` |
| GET | `/api/config/export` | – | config as download `vdmot-config.json` (no secrets) |
| POST | `/api/stm/motor` | any of `motor` (all five values unless read already), `learnMovements` (0 or 50..65534), `breakaway` `{enable,stepPct 0..100,maxmA 20..60}` (v2) | 202 |

An empty secret in `POST /api/config` keeps the stored one. Changing
network settings or the station name restarts the ESP after the response
(the STM keeps running); MQTT settings reconnect MQTT.

Example:
```sh
curl -u admin:secret -X POST http://vdmot/api/config \
  -H 'Content-Type: application/json' -d '{"calib":{"dayMask":9,"hour":3,"minute":15}}'
```

### STM firmware

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/stm/images` | – | stored images `[{"name","size","crc32","version"}]` |
| POST | `/api/stm/images` | multipart, one `.bin` file, <= 512 KiB | `201` image info; `413`; `507` no space or too many images; `409` while flashing |
| DELETE | `/api/stm/images/{name}` | – | `204`; `404`; `409` |
| POST | `/api/stm/flash` | `{"image":"<name>","mode":"normal"\|"blank","force":false}` | 202; `400 invalid_image` with the validation error; `409` busy or restart pending |
| POST | `/api/stm/flash/abort` | – | 202 |
| POST | `/api/stm/reset` | `{"confirm":true}` | 202, pulses the STM reset (all valves recalibrate) |

`mode:"blank"` skips the application handshake (STM already in its ROM
bootloader, BOOT0 held). `force:true` only overrides the check for the
update handshake strings in the image.

### ESP firmware and system

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/ota/esp` | multipart ESP32 application image; optional MD5 as query `md5`, form field `md5`, or header `X-Update-MD5` | `200 {"result":"ok","restart":true}`, restart after 1 s; error with the reason otherwise |
| POST | `/api/system/reboot` | – | 202, ESP restart after 1 s |
| POST | `/api/system/factory-reset` | `{"confirm":"factory-reset"}` | 202, erases the new firmware's settings and restarts |
| POST | `/api/mqtt/reconnect` | – | 202 |
| POST | `/api/mqtt/discovery` | `{"action":"publish"\|"delete"\|"republish"}` | 202; `409` when mode is not MQTT + HA |

Only one upload or STM flash runs at a time; no ESP update is accepted during an
STM flash.

```sh
# ESP update from the command line
curl -u admin:secret -F "file=@VdMot-Revamped_2.0.0-revamped_ESP32-WT32-ETH01.bin" \
  "http://vdmot/api/ota/esp?md5=$(md5sum VdMot-Revamped_2.0.0-revamped_ESP32-WT32-ETH01.bin | cut -d' ' -f1)"
```

## Development mock

`software_esp32_revamped/tools/mock_api.py` serves the dashboard with a
simulated API (standard library only):

```sh
python3 software_esp32_revamped/tools/mock_api.py --port 8080            # protocol v2 STM
python3 software_esp32_revamped/tools/mock_api.py --proto 1              # legacy STM 1.4.x
python3 software_esp32_revamped/tools/mock_api.py --auth admin:secret    # with Basic auth
```
