# VdMot Revamped: MQTT and Home Assistant

The new ESP firmware keeps the legacy topic tree, payloads and retain flags, so
existing Home Assistant entities, ioBroker/Node-RED flows and the Tortoise
add-on keep working. New data goes to new topics. Topics of removed features
(PI control, window logic) are no longer published or subscribed.

Source of truth: `software_esp32_revamped/lib/core/src/mqtt_topics.cpp`,
`ha_discovery.cpp` and `src/mqtt_client.cpp`.

## Connection

- MQTT 3.1.1, QoS 0. Client id = station name (`VdMot` when empty).
- User/password are sent only when both are set. Keep-alive from settings.
- Reconnect back-off 2 s, 4 s, ... up to 60 s.
- Last will: `<main>status` = `offline` (retained). After connecting:
  `<main>status` = `online` (retained), subscriptions, full publish.

Settings (Settings → MQTT, imported from the legacy config):

| Setting | Effect |
|---|---|
| mode | 0 off, 1 MQTT, 2 MQTT + Home Assistant discovery |
| separate | legacy "separate topics": `/value` appended to data topics, commands on `.../set` |
| pathAsRoot | leading `/` before the main topic |
| retained | retain flag for the legacy topics |
| plainText | text states (`idle`, `ok`) instead of numbers |
| germanDecimal | `21,5` instead of `21.5` |
| diag | legacy per-valve diagnostic topics |
| upTime | `common/uptime` |
| allTemps | publish temperature slots that are also assigned to a valve |
| onChange | publish changes (at most every `minDelayS` per item), plus a full publish every `publishIntervalS` |
| newDiag | new `diag/*` topics and HA diagnostic entities |
| events | `events` topic |
| haDiscoveryOnConnect | send HA discovery on every connect |

## Naming

- `<main>` = (`/` if pathAsRoot) + (`<station>/` or `VdMotFBH/` when the station name is empty).
- `<V>`, `<T>`, `<S>` = valve / temperature / voltage name with spaces replaced
  by `_`, or the 1-based index when the name is empty.
- `<x>` below means `<x>/value` when **separate** is on.

## Legacy topics (unchanged)

| Topic | Payload |
|---|---|
| `<main>common/ip` | IPv4, e.g. `192.168.1.51` |
| `<main>common/state` | `ok` / `info` / `error` (plainText) or `0` / `1` / `2` |
| `<main>common/uptime` | `3d 4:17:05` |
| `<main>common/message` | text of the latest warning-or-worse event, empty if none |
| `<main>valves/<V>/target` | target 0..100 |
| `<main>valves/<V>/state` | `idle`, `opens`, `closes`, `failed`, `unknown`, `no valve`, `full open`, `connected`, `blocked` (or the status number) |
| `<main>valves/<V>/calibration/date` | `Wednesday, September 23.2026 03:00:12` (end of the last calibration seen since the ESP booted) |
| `<main>valves/<V>/calibration/repetitions` | failed passes of the last calibration |
| `<main>valves/<V>/diag/meanCurrrent` (sic, three r) | learned mean current, mA |
| `<main>valves/<V>/diag/openCount`, `closeCount`, `deadZoneCount`, `moves` | calibration counts, dead zone, moves |
| `<main>valves/<V>/temp1`, `temp2` | assigned sensor temperature incl. offset, `21.5` or `failed` |
| `<main>temps/<T>/id`, `<main>temps/<T>/value` | 1-Wire id; temperature or `failed` (not on the bus, invalid, or older than 60 s) |
| `<main>sensors/<S>/id`, `value`, `unit` | DS2438 voltage sensors: `(raw/100 + offset) × factor` with 3 decimals, or `failed` |

Published for active valves only. Retain flag = the **retained** setting.

### Command topic

| Topic | Payload |
|---|---|
| `<main>valves/<V>/target/set` (separate on; `.../target/set/set` also accepted for ioBroker) | `0`..`100`, decimals like `55.0` accepted, `OPEN` = 100, `CLOSE` = 0 |
| `<main>valves/<V>/target` or `.../target/set` (separate off) | same |

`<V>` may be the valve name or its number 1..12. Commands for unknown or
inactive valves and invalid payloads are rejected and logged
(`mqtt_command_rejected`). No other topic is subscribed except
`homeassistant/status` (mode MQTT + HA: `online` triggers a discovery re-send).
The text entities of the legacy discovery have command topics, but the firmware
ignores them (as before).

### No longer published
`common/heatControl`, `common/parkPosition`, `valves/<V>/tTarget`,
`valves/<V>/tValue`, `valves/<V>/control/*`, `valves/<V>/window/*`.

## New topics

Never suffixed with `/value`. "v2" = only with a revamped STM (protocol 2).

| Topic | Payload | Retained | Published |
|---|---|---|---|
| `<main>status` | `online` / `offline` (LWT) | always | on connect / by the broker |
| `<main>valves/<V>/actual` (+`/value`) | believed position 0..100 | setting | with the valve data |
| `<main>diag/valves/<V>/lastMove` | `{"dir":"open","req":408,"cnt":395,"stop":"endstop","peak":299,"ms":15784}` | setting | v2, after each move |
| `<main>diag/valves/<V>/earlyStops` | count since STM start | setting | v2, on change |
| `<main>diag/valves/<V>/cmdRejected` | count since STM start | setting | v2, on change |
| `<main>diag/valves/<V>/calState` | `0` idle, `1` calibration requested, `2` running | setting | v2, on change |
| `<main>diag/valves/<V>/profile` | `{"valve":1,"count":n,"samples":[[count,current],...]}` | never | v2, when a new profile is read |
| `<main>diag/stm/proto` | `1` or `2` | setting | on change |
| `<main>diag/stm/link` | `unknown`, `up`, `degraded`, `down`, `booting`, `suspended` | setting | on change |
| `<main>diag/stm/uptime` | STM uptime, s | setting | v2, every full publish |
| `<main>diag/stm/resets`, `rxOverflow`, `parseErr` | STM counters | setting | v2, on change |
| `<main>diag/calibration/active` | `1` while any valve calibrates, else `0` | always | on change |
| `<main>events` | event JSON (below) | never | warnings and worse, and calibration outcomes; rate-limited |

Units: `lastMove.peak` and profile currents are in **0.1 mA** (299 = 29.9 mA);
`req` 65535 means "run to the end stop"; `stop` is one of `none`, `target`,
`endstop`, `early_endstop`, `timeout`, `undercurrent`, `safety_overcurrent`,
`aborted`.

`diag/*` topics need **newDiag**, `events` needs **events**.

Event JSON (same as the HTTP API):
```json
{"seq":42,"t":1790072393,"up":72180,"sev":"warning","code":408,"name":"calib_retry",
 "valve":6,"a1":2,"a2":0,"text":"","msg":"valve 6: calibration retry 2"}
```
`t` is Unix time (null before NTP sync), `up` the ESP uptime in seconds,
`valve` 1..12 or null. Rate limit: one message per valve and event code per
10 minutes, at most 30 per hour.

## Home Assistant discovery

Active in mode 2 (MQTT + HA). Sent on connect (if enabled), when HA publishes
`homeassistant/status` = `online`, and from Maintenance → MQTT. Topic:
`homeassistant/<component>/<station>/<objectId>/config`, retained.

Every entity has `availability_topic` = `<main>status` and the device
`{identifiers: <station>, name: <station>, model: "VdMot Revamped",
manufacturer: "Lenti84/Surfgargano", hw_version: "2.0", sw_version: "2.0.0-revamped",
configuration_url: "http://<ip>/"}`. unique_id = `<station>.<id>` with spaces
replaced by `_`.

### Kept entities (legacy object ids and unique_ids)

`<R>` = valve segment (name or number).

| Component | objectId | unique_id (after `<station>.`) | Condition |
|---|---|---|---|
| text | `state`, `message`, `ip`, `uptime` | `common.state`, ... | uptime only if upTime |
| text | `valves_state_<R>` | `valves.<R>.state` | active valve |
| valve | `valves_target_<R>` | `valves.<R>.target` | active valve; `reports_position`, command topic = target command |
| sensor | `valves_temp1_<R>`, `valves_temp2_<R>` | `valves.<R>.temp1/2` | sensor assigned; °C |
| text | `valves_calibration_date_<R>`, `valves_calibration_repetitions_<R>` | `valves.<R>.calibration.date/.repetitions` | active valve |
| text | `valves_diag_{openCount,closeCount,deadZoneCount,moves,meanCurrrent}_<R>` | `valves.<R>.diag.<x>` | diag |
| sensor | `temps_<T>` | 1-Wire id | published temperature slot; °C |
| sensor | `volts_<S>` | 1-Wire id | active voltage slot; `device_class: voltage` for unit V/mV |

Differences to the legacy discovery: availability added, JSON escaping, invalid
`device_class`/`state_class` removed from text entities; the valve entity uses
`device_class: water`.

### New entities (need newDiag)

| Component | objectId | unique_id (after `<station>.`) | State topic |
|---|---|---|---|
| sensor | `valves_actual_<R>` | `valves.<R>.actual` | `valves/<R>/actual` (%) |
| sensor (diagnostic) | `diag_earlyStops_<R>` | `diag.<R>.earlyStops` | `diag/valves/<R>/earlyStops` |
| sensor (diagnostic) | `diag_cmdRejected_<R>` | `diag.<R>.cmdRejected` | `diag/valves/<R>/cmdRejected` |
| sensor (diagnostic) | `diag_lastStop_<R>` | `diag.<R>.lastStop` | `diag/valves/<R>/lastMove`, `{{ value_json.stop }}` |
| sensor (diagnostic) | `diag_stm_link` | `diag.stm.link` | `diag/stm/link` |
| sensor (diagnostic) | `diag_stm_uptime` | `diag.stm.uptime` | `diag/stm/uptime` (duration, s) |
| binary_sensor (diagnostic) | `diag_calibration_active` | `diag.calibration.active` | `diag/calibration/active` (`device_class: running`) |

The per-valve diag entities are announced for every active valve; their values
arrive only with a v2 STM.

### Removed entities

Deleted once with an empty retained config (first run of the new firmware, in
mode 1 or 2), for every valve in both name and number form:
`climate/<st>/climate_<R>`, `number/<st>/valves_control_{dynOffs,min,max}_<R>`,
`select/<st>/valves_window_state_<R>`, `switch/<st>/valves_window_state_<R>`,
`number/<st>/valves_window_target_<R>`, `select/<st>/heatControl`,
`number/<st>/parkPosition`. Stale lines of the legacy `/HADiscovery.cfg` list
are deleted the same way.
