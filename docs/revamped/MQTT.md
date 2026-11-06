# VdMot Revamped: MQTT and Home Assistant

The new ESP firmware keeps the legacy topic tree, the retain flags and the HA
unique_ids, so existing Home Assistant entities, ioBroker/Node-RED flows and the
Tortoise UFH add-on keep working. A few legacy topics carry different content
(see "Same topic, different content"). New data goes to new topics. Topics of
removed features (PI control, window logic) are no longer published or
subscribed.

Source of truth: `software_esp32_rust/core/src/mqtt_topics.rs`,
`mqtt_policy.rs`, `mqtt_values.rs`, `ha_discovery.rs` and
`software_esp32_rust/glue/src/mqtt_client.rs`. Event codes: DESIGN.md section 13.

## Connection

- MQTT 3.1.1. Subscriptions: target commands and the HA status with **QoS 1**,
  the buttons `cmd/#` with QoS 0. Publications with QoS 0.
- Client id: `mqtt.clientId` when set (used as it is; some brokers accept at
  most 23 characters), else `<host>-<mac6>`: the DHCP host name of the station
  (at most 16 characters) and the last 3 bytes of the MAC, e.g. `VdMot-a1b2c3`.
  **Broker ACLs keyed on the legacy client id `VdMot` must be updated.**
- Persistent session (`cleanSession=false`), so the broker keeps QoS 1 target
  commands while the ESP is offline. The first connect after a change of the
  topic settings (station, root, names, overrides, MQTT settings) uses a clean
  session.
- User/password are sent only when both are set (as legacy; the dashboard warns
  about a user without password). Keep-alive from settings.
- Reconnect back-off 2 s, doubling up to 60 s; it starts again at 2 s only
  after a connection stayed up for 60 s.
- Last will: `<main>status` = `offline` (retained). After connecting:
  `<main>status` = `online` (retained), subscriptions, full publish.

Broker settings for the persistent session (Mosquitto): `max_queued_messages`
2000 or more (or 0), `persistent_client_expiration 7d` (or similar), so a long
ESP outage neither drops nor keeps commands forever. After a long outage the
queued commands are applied in order; the latest target wins. A station rename
changes the client id and leaves the old session at the broker until it
expires.

Settings (Settings → MQTT, imported from the legacy config):

| Setting | Effect |
|---|---|
| mode | 0 off, 1 MQTT, 2 MQTT + Home Assistant discovery (needs **separate**, not **germanDecimal**) |
| separate | legacy "separate topics": `/value` appended to data topics, commands on `.../set` |
| pathAsRoot | leading `/` before the main topic |
| rootTopic | main topic; empty = the station name |
| clientId | MQTT client id; empty = automatic |
| discoveryPrefix | HA discovery prefix, default `homeassistant` |
| retained | retain flag for the legacy topics |
| plainText | text states (`idle`, `ok`) instead of numbers |
| germanDecimal | `21,5` instead of `21.5` |
| diag | legacy per-valve diagnostic topics |
| upTime | `common/uptime` |
| allTemps | publish temperature slots that are also assigned to a valve |
| onChange | publish changes (at most every `minDelayS` per item), plus a full publish every `publishIntervalS` |
| newDiag | new `diag/*` topics and HA diagnostic entities |
| events | `events` topic and the HA event entity |
| haDiscoveryOnConnect | send HA discovery on every connect |

Per valve, temperature and voltage slot: **topic** (topic override, below).
The failsafe settings (Settings → Failsafe) are described in INSTALL.md; they
do not reconnect MQTT.

## Naming

- `<main>` = (`/` if pathAsRoot) + `rootTopic` (or the station name when empty) + `/`.
  The legacy import sets `rootTopic` = `VdMotFBH` when the legacy station name
  was empty (the legacy firmware published under `VdMotFBH/` then); the
  station becomes `VdMot`.
- `<V>`, `<T>`, `<S>` = the item's **topic** override when set; else its name
  with spaces replaced by `_`; else, for a valve, its number 1..12, and for an
  unnamed temperature or voltage slot, 1 + its **STM bus index** (as legacy).
  An unnamed sensor that is not on the bus is not published.
- The topic override takes legacy names that are not valid names in the new
  firmware (`/`, `"`, `\`); the import fills it. A `/` in the override makes a
  multi-level segment (`valves/Bad/WC/...`), as in the legacy firmware.
- `<x>` below means `<x>/value` when **separate** is on.

## Legacy topics

| Topic | Payload |
|---|---|
| `<main>common/ip` | IPv4, e.g. `192.168.1.51` |
| `<main>common/state` | `ok` / `info` / `error` (plainText) or `0` / `1` / `2` |
| `<main>common/uptime` | `3d 4:17:05` |
| `<main>common/message` | text of the latest warning-or-worse event, empty if none |
| `<main>valves/<V>/target` | target 0..100 (separate: the target the STM confirmed) |
| `<main>valves/<V>/state` | `idle`, `opens`, `closes`, `failed`, `unknown`, `no valve`, `full open`, `connected`, `blocked` (or the status number) |
| `<main>valves/<V>/calibration/date` | `Wednesday, September 23.2026 03:00:12` |
| `<main>valves/<V>/calibration/repetitions` | failed passes of the last calibration |
| `<main>valves/<V>/diag/meanCurrrent` (sic, three r) | learned mean current, mA |
| `<main>valves/<V>/diag/openCount`, `closeCount`, `deadZoneCount`, `moves` | calibration counts, dead zone, moves since the last calibration |
| `<main>valves/<V>/temp1`, `temp2` | assigned sensor temperature incl. offset, `21.5` or `failed`; temp1 is the supply of the valve's heating loop, temp2 the return |
| `<main>temps/<T>/id`, `<main>temps/<T>/value` | 1-Wire id; temperature or `failed` (not on the bus, invalid, or older than 60 s) |
| `<main>sensors/<S>/id`, `value`, `unit` | DS2438 voltage sensors: `(raw/100 + offset) × factor` with 3 decimals, or `failed`; published for every configured slot with an id, active or not |

Valve topics are published for active valves only. Retain flag = the
**retained** setting.

### Same topic, different content

These topics exist in both firmwares but carry other data. Automations that
parse them must be checked.

| Topic | Legacy firmware | New firmware |
|---|---|---|
| `common/message` | reset reason, or `stm not working` | message of the latest warning-or-worse event (e.g. `valve 3: blocked (calibration retries 2, failsafe 50 %)`), empty if none |
| `common/state` | fixed `info`; `error` after an STM reset | derived: `error` when the STM link is down, an active valve is blocked or failed, or the STM is in safe mode; `info` when the link is not up, the failsafe is active, or an active valve has any other health flag; else `ok` |
| `valves/<V>/calibration/date` | local time while a calibration runs | end of the last calibration observed since the ESP booted; nothing before the first one |
| `valves/<V>/target` (separate on) | the requested target | the target the STM confirmed (read-back); nothing while a restored target is not confirmed yet; the requested one is in `valves/<V>/requested` |
| `temps/<T>`, `sensors/<S>` of an unnamed slot | STM bus index + 1 | the same (2.0.0 used the slot number) |
| `sensors/<S>/...` | every configured slot | the same (2.0.0: active slots only) |

### Commands

| Topic | QoS | Payload | Action |
|---|---|---|---|
| separate on: `<main>valves/<V>/target/set` (`.../target/set/set` also accepted for ioBroker); separate off: `<main>valves/<V>/target` or `.../target/set` | 1 | `0`..`100`, optionally with a fraction (`.` or `,`, e.g. `43.7`, `43,7`), rounded half up; `OPEN` = 100, `CLOSE` = 0; `STOP` (STM 2.1: stops the valve where it is) | target of the valve |
| `<main>valves/<V>/flow/set` (with and without separate) | 1 | the flow in l/min: digits, optionally `.` or `,` and 1..3 digits, e.g. `2.5`, `2,5`, `0.125`; blanks around allowed, no sign or exponent, at most 16 bytes | target in l/min of a valve with a running flow layer, above `qmax` clipped to it, `0` closes (Rust 2.2.10) |
| `<main>cmd/valves/<V>/calibrate` | 0 | `PRESS` | calibrate the valve |
| `<main>cmd/calibrate` | 0 | `PRESS` | calibrate all valves |
| `<main>cmd/detect` | 0 | `PRESS` | valve detection |
| `<main>cmd/stmReset` | 0 | `PRESS` | reset the STM |
| `<main>cmd/restart` | 0 | `PRESS` | restart the ESP |
| `<main>cmd/stop` | 0 | `PRESS` | stop every valve (STM 2.1) |
| `<main>cmd/stmSafeExit` | 0 | `PRESS` | leave the STM safe mode (STM 2.1) |

Payload rules of a target in %: digits with at most one `.` or `,` separator,
blanks around allowed, no sign or exponent, at most 16 bytes; the value must lie
in 0..100.
`<V>` may be the configured segment (also a multi-level override) or the valve
number 1..12.

Rejected commands (unknown or inactive valve, invalid payload, STM command not
supported, app queue full, unconfirmed retained clear) are counted and logged
as `mqtt_command_rejected` (at most once per 10 s for the same valve and
reason); the text names the reason.

Retained commands: after a non-empty message on a command topic the ESP
publishes an empty retained message to that topic, so a retained command runs
once and not again at the next connect. Button actions run only after the
broker echoed that empty message back (within 5 s; otherwise they are
rejected with `clear not confirmed`). Targets act at once. The state form
`valves/<V>/target` without separate and the HA status topics are never
cleared. When the app queue is full, the latest target per valve is kept and
submitted again.

Other subscriptions (mode 2): `homeassistant/status` and, when different,
`<discoveryPrefix>/status` (see "Home Assistant"). The text entities of the
legacy discovery have command topics, but the firmware ignores them (as
before).

### No longer published
`common/heatControl`, `common/parkPosition`, `valves/<V>/tTarget`,
`valves/<V>/tValue`, `valves/<V>/control/*`, `valves/<V>/window/*`.

The firmware does not clear them. A retained message of the old firmware stays
at the broker until someone publishes an empty retained message to its topic.
None of these topics is a command topic, so that moves no valve.

## New topics

"v2" = only with a revamped STM (protocol 2 or 3), "v3" = STM 2.1.

| Topic | Payload | `/value` with separate | Retained | Published |
|---|---|---|---|---|
| `<main>status` | `online` / `offline` (LWT) | no | always | on connect / by the broker |
| `<main>stm/status` | `online` / `offline`: STM link up | no | always | on change and every full publish |
| `<main>failsafe` | `1` while the failsafe lease has expired (the regulator was silent for `failsafe.timeoutMin`; STM lease or ESP emulation), else `0` | no | always | on change and every full publish |
| `<main>valves/<V>/actual` | believed position 0..100 | yes | setting | with the valve data |
| `<main>valves/<V>/requested` | requested (desired) target | yes | setting | active valve with a target |
| `<main>valves/<V>/sync` | target delivery: `unknown`, `synced`, `pending`, `await_ack`, `await_verify`, `failed` | yes | setting | active valve |
| `<main>valves/<V>/failsafe` | `off`, `lease` (regulator silent), `blocked` (blocked valve at its failsafe position) | yes | setting | active valve |
| `<main>valves/<V>/problem` | `1` when the valve is blocked, failed, without contact, stale, its target is not confirmed, a temperature failed or its flow layer failed to position it, else `0` | yes | setting | active valve |
| `<main>valves/<V>/flow` | `{"state":"held","lpm":1.000,"targetLpm":1.000}`, see "Feedback for regulators"; empty when the valve has no layer | yes | setting, the empty message always | active valve with the flow layer, with the valve data |
| `<main>diag/valves/<V>/lastMove` | `{"dir":"open","req":408,"cnt":395,"stop":"endstop","peak":299,"ms":15784}` | no | setting | v2, after each move |
| `<main>diag/valves/<V>/earlyStops` | count since STM start (early stops the next move did not recover) | no | setting | v2, on change |
| `<main>diag/valves/<V>/cmdRejected` | count since STM start | no | setting | v2, on change |
| `<main>diag/valves/<V>/calState` | `0` idle, `1` calibration requested, `2` running | no | setting | v2, on change |
| `<main>diag/valves/<V>/coastOpen`, `coastClose` | coast the STM learned for an opening / a closing step, motor pulses; `0` = not learned since the STM started (`coastOpen` always `0` since STM 2.2.5) | no | setting | STM 2.2.4, on change |
| `<main>diag/valves/<V>/profile` | `{"valve":1,"count":n,"samples":[[count,current],...]}` | no | never | v2, when a new profile is read |
| `<main>diag/stm/proto` | `1`, `2` or `3` | no | setting | on change |
| `<main>diag/stm/link` | `unknown`, `up`, `degraded`, `down`, `booting`, `suspended` | no | setting | on change |
| `<main>diag/stm/version` | STM version, e.g. `2.1.0-revamped_C2` | no | setting | on change |
| `<main>diag/stm/started` | STM start time, UTC `2026-09-25T08:13:40+00:00` | no | setting | v2, when it moved by more than 60 s |
| `<main>diag/stm/uptime` | STM uptime, s | no | setting | v2, every full publish |
| `<main>diag/stm/resets`, `rxOverflow`, `parseErr` | STM counters | no | setting | v2, on change |
| `<main>diag/stm/lease` | `off`, `running`, `expired` | no | setting | on change |
| `<main>diag/stm/safeMode` | `1` / `0` | no | setting | v3, on change |
| `<main>diag/mqtt/eventsSuppressed`, `commandsRejected` | counters | no | setting | on change, at most every 10 s |
| `<main>diag/calibration/active` | `1` while any valve calibrates, else `0` | no | always | on change |
| `<main>diag/calibration/next` | next scheduled calibration, UTC, or empty | no | setting | on change |
| `<main>events` | event JSON (below) | no | never | see "Events" |

Units: `lastMove.peak` and profile currents are in **0.1 mA** (299 = 29.9 mA);
`req` 65535 means "run to the end stop"; `stop` is one of `none`, `target`,
`endstop`, `early_endstop`, `timeout`, `undercurrent`, `safety_overcurrent`,
`aborted`. `coastOpen` and `coastClose` are in motor pulses, the unit of
`openCount`/`closeCount`; the STM shortens partial closing moves by
`coastClose` (since STM 2.2.5 it learns no opening coast). An STM before
2.2.4 does not report them: the ESP publishes nothing for them then.

`diag/*` topics need **newDiag**, `events` needs **events**. `status`,
`stm/status`, `failsafe` and the per-valve `requested`, `sync`, `failsafe`,
`problem` are published whatever **newDiag** says: they carry safety state.

## Feedback for regulators

A regulator (Home Assistant thermostat, Tortoise UFH, Node-RED) that sends
valve targets should read back:
- `valves/<V>/actual` (HA `valves_actual_<R>`, always announced): the position
  the valve really has;
- `valves/<V>/problem`: the valve cannot follow its target;
- `valves/<V>/failsafe` and `failsafe`: the ESP or STM overrides the target
  because the regulator was silent (the target is applied again once the
  regulator is back);
- `stm/status`: the STM link.

With **separate**, `valves/<V>/target` is the target the STM confirmed, so a
regulator that compares it with what it sent sees a lost command. A valve with
the flow layer shows its desired target there: the STM's own is the layer's
fallback position.

A valve with the flow layer (Rust 2.2.8, `GET /api/valves/{n}/flow` in
API.md) takes and reports percentages of its stroke: `valves/<V>/target`,
`requested` and `actual` mean % of `strokeCounts`, 0 is closed and 100 the open
end stop. `actual` is the held position, 0 while it is unknown (after a restart,
until the valve is positioned again). `sync` says nothing about the positioning:
the layer delivers the target, so `sync` goes `synced` at the next read-back of
the STM, also while the valve still moves. The Home Assistant entities of the valve
carry the same values; `failsafe` and the failsafe position stay on the STM's own
scale.

Since Rust 2.2.10 such a valve also takes its target in l/min on
`valves/<V>/flow/set`. The layer positions on the curve for that flow, and
`target`, `requested` and `actual` show the position in % of the stroke, so two
flows in one whole percent look the same there. A target on `target/set` ends the
flow. Above `qmax` the flow is clipped to `qmax`, and between the top of the curve
and `qmax` the valve goes to the top of the curve. The ESP clears a retained
message on the topic, as on `target/set`. Without the layer running on the valve
the command is rejected `unsupported`, a payload that is no number is a payload
reject. Without **separate** the first message on `target` after a connect is
taken for the retained copy of a valve with a target in l/min: it is ignored,
whatever its %, and the flow stays. `target/set` works as ever.

`valves/<V>/flow` reports the layer in one line of JSON, the values of `flow.state`,
`flow.lpm` and `flow.targetLpm` in `GET /api/valves`:

```json
{"state":"held","lpm":1.000,"targetLpm":1.000}
```

- `state`: `unsupported`, `unknown`, `positioning`, `held`, `failed`;
- `lpm`: the flow of the curve at the held position in l/min, `null` while the
  position is not known;
- `targetLpm`: the target in l/min, `null` for a target in %.

It goes out with the valve data: on every full publish, and when one of the three
changes at most every `minDelayS`. With **separate** the topic is
`valves/<V>/flow/value`. A positioning that failed shows as `problem` = 1 and
event 417.

Only an active valve with the layer enabled publishes the topic. Any other valve
the STM reports gets one empty retained message on it per connection, whatever
**retained** says, so a layer switched off leaves no state at the broker, also one
switched off while the ESP was away, and a retained message of Rust 2.2.10 goes as
well. A subscriber of the topic must take the empty payload: it means no layer.

## Restart and broker loss

| Event | What happens |
|---|---|
| ESP restart on request (`cmd/restart`, dashboard, OTA) | `<main>status` = `offline`, retained, then a clean disconnect: the broker sends no last will |
| ESP crash or power loss | the broker sends the last will after 1.5 x keep-alive |
| Every connect | `<main>status` = `online`, the subscriptions, a full publish spread over the passes, and every `diag/*` value once. `common/ip` goes out once per connection. `valves/<V>/calibration/date` goes out only after a calibration seen since the boot, so its retained value stays over a restart. `diag/valves/<V>/profile` goes out only for a new profile |
| Commands sent while the ESP was away | the broker keeps QoS 1 messages of the persistent session and hands them over in order. A sender must publish with QoS 1: a QoS 0 message is not queued. Retained commands run once and are cleared |
| `flow/set` before the ESP knows the STM (first seconds after a boot, an STM reset) | the state of the layer is `unsupported` until the STM version is read, so the command is rejected and cleared. A regulator that repeats its target recovers with the next one. The stored target stays: it survives a restart of the ESP and of the STM |
| Broker away | no publish; reconnect with the back-off above. Events of that time are not sent later: they stay in the log (`GET /api/events`), `common/message` shows the latest. The failsafe lease is no longer renewed, so `failsafe.timeoutMin` later the valves go to their failsafe positions |
| Broker back without its retained messages | the next connect restores every topic above except `calibration/date` of a valve without a calibration since the boot, and the HA configs when `haDiscoveryOnConnect` is off |

## Events

Event JSON (same keys as the HTTP API plus `event_type`):
```json
{"seq":42,"t":1790072393,"up":72180,"sev":"warning","code":408,"name":"calib_retry",
 "event_type":"calib_retry","valve":6,"a1":2,"a2":0,"text":"","msg":"valve 6: calibration retry 2"}
```
`t` is Unix time (null before NTP sync), `up` the ESP uptime in seconds,
`valve` 1..12 or null.

Which events: every code has an MQTT class (DESIGN.md section 13): "always"
(calibration outcomes, failsafe active/ended, regulator back, STM safe mode),
"warning and worse", or never. Valve events of the same code within 2 s are
merged into one message with `"valve":null` and `"valves":[1,3,12]`, and the
message starts `valves 1, 3, 12: `. Rate limit by priority class: critical
events once per code and valve per minute, without hourly budget; errors 30
per hour and once per code and valve per 10 min; everything else 30 per hour
and once per code and valve per 10 min. Suppressed events are counted
(`diag/mqtt/eventsSuppressed`).

Of the flow layer, `flow_position_failed`, `flow_hold_lost` and
`flow_config_unreadable` reach MQTT. `flow_positioned` is an Info event of the class
"warning and worse": it stays in the log, although `event_types` lists it. `flow_curve_cut`
(Rust 2.2.15) stays in the log and is not in `event_types`: with the longest fields
the event types fill the payload of the event entity.

## Home Assistant

Discovery runs in mode 2 (MQTT + HA). It is sent on connect (if
haDiscoveryOnConnect), when HA comes back (`<prefix>/status` goes from
`offline` to `online`), and from Maintenance → MQTT (also in mode 1, then with
**separate** required). After a connect, when the STM reports other sensor
assignments, and when the flow layer of a valve is switched or gets another `qmax`, it
waits until the STM data has settled (link up, re-sync done,
30 s for the 1-Wire lists; at most 2 min), so the set goes out once with the
STM's sensor assignments instead of once per step of its start-up; a short
loss of the settled state without another assignment sends nothing. Topic:
`<discoveryPrefix>/<component>/<node>/<objectId>/config`, retained, where
`<node>` is the station name made HA-safe (letters, digits, `_`, `-`;
other characters transliterated or replaced by `_`).

**Recommended HA settings** (MQTT integration → options): birth message and
last will **retained**, with **QoS 1**. The ESP takes HA's `offline` as "the
regulator is gone" (the failsafe lease is no longer renewed) and `online` as
"back". Without retained birth/LWT a restarted ESP does not know the state
until HA sends one; the dashboard warns while HA reports offline and commands
still arrive. An accepted MQTT command also counts as "HA online".

Device block: `{identifiers: <station>, name: <station>, model: "VdMot
Revamped", manufacturer: "Lenti84/Surfgargano", hw_version: <STM board C1/C2,
absent while unknown>, sw_version: <ESP version>, configuration_url:
"http://<ip>/"}`. unique_id = `<station>.<id>` with spaces replaced by `_`,
built from the raw segment (an override `Bad/WC` gives
`VdMot.valves.Bad/WC.state`, the legacy value); object ids are HA-safe.

Availability:
- **kept entities** (below): none, as in the legacy firmware. After a rollback
  to the legacy firmware they keep working.
- **new entities**: `<main>status` (ESP), or `<main>status` and
  `<main>stm/status` together (`availability_mode: all`) for data that comes
  from the STM.

`<R>` = valve segment (HA-safe form in object ids).

### Kept entities (legacy object ids and unique_ids)

| Component | objectId | unique_id (after `<station>.`) | Condition |
|---|---|---|---|
| text | `state`, `message`, `ip`, `uptime` | `common.state`, ... | uptime only if upTime |
| text | `valves_state_<R>` | `valves.<R>.state` | active valve |
| valve | `valves_target_<R>` | `valves.<R>.target` | active valve; `reports_position`, command topic = target command, `qos: 1`, `payload_stop: STOP` with STM 2.1 |
| sensor | `valves_temp1_<R>`, `valves_temp2_<R>` | `valves.<R>.temp1/2` | sensor assigned; °C |
| text | `valves_calibration_date_<R>`, `valves_calibration_repetitions_<R>` | `valves.<R>.calibration.date/.repetitions` | active valve |
| text | `valves_diag_{openCount,closeCount,deadZoneCount,moves,meanCurrrent}_<R>` | `valves.<R>.diag.<x>` | diag |
| sensor | `temps_<T>` | 1-Wire id | published temperature slot; °C |
| sensor | `volts_<S>` | 1-Wire id | active voltage slot with an id; `device_class: voltage` for unit V/mV |

Temperature and voltage sensors use `value_template: {{ value | replace(',', '.') | float(None) }}`
(so `failed` shows as unknown) and `expire_after` = 3 × publishIntervalS, at
least 60 s. Differences to the legacy discovery: JSON escaping, invalid
`device_class`/`state_class` removed from text entities, the valve entity uses
`device_class: water`.

### New entities

Per valve:

| Component | objectId | Availability | Gate |
|---|---|---|---|
| sensor | `valves_actual_<R>` (position, %) | ESP + STM | always |
| binary_sensor (problem) | `valves_problem_<R>` | ESP | always |
| sensor (enum off/lease/blocked) | `valves_failsafe_<R>` | ESP + STM | always |
| sensor (enum) | `valves_sync_<R>` (target delivery) | ESP | always |
| button | `valves_calibrate_<R>` | ESP + STM | always |
| sensor (diagnostic) | `diag_earlyStops_<R>`, `diag_cmdRejected_<R>` | ESP + STM | newDiag |
| sensor (diagnostic, enum) | `diag_lastStop_<R>` | ESP + STM | newDiag |
| sensor (diagnostic) | `diag_calState_<R>` | ESP + STM | newDiag |
| sensor (diagnostic, measurement) | `diag_coastOpen_<R>`, `diag_coastClose_<R>` (no unit, motor pulses) | ESP + STM | newDiag |
| number | `valves_flowTarget_<R>` (flow target, L/min) | ESP + STM | flow layer enabled on the valve |
| sensor (measurement) | `valves_flow_<R>` (flow of the curve, L/min) | ESP + STM | flow layer enabled on the valve |
| sensor (diagnostic, enum) | `valves_flowState_<R>` (state of the layer) | ESP + STM | flow layer enabled on the valve |

`valves_flowTarget_<R>` writes `valves/<V>/flow/set` with QoS 1: `min` 0, `max` the
`qmax` of the valve, `step` 0.1, `device_class: volume_flow_rate`, `mode: box`,
`command_template: {{ '%.3f' | format(value) }}`. It reads `targetLpm` from
`valves/<V>/flow`, so it also shows a target set elsewhere, and unknown for a target
in %. `valves_flow_<R>` (`lpm`, `device_class: volume_flow_rate`) and
`valves_flowState_<R>` (`state`, `device_class: enum`) read the same topic. All three
use `value_template: {{ value_json.<field> if value else None }}`: `null` and the
empty message of a layer switched off are unknown in Home Assistant, not a template
error. `valves_target_<R>` stays the % of the stroke, and a value there ends the
target in l/min.

Per device:

| Component | objectId | Availability | Gate |
|---|---|---|---|
| binary_sensor (connectivity) | `diag_esp_online` (`<main>status`) | none | always |
| binary_sensor (connectivity) | `diag_stm_online` (`<main>stm/status`) | ESP | always |
| binary_sensor (problem) | `diag_failsafe` (`<main>failsafe`) | ESP + STM | always |
| button | `cmd_calibrate_all`, `cmd_detect`, `cmd_stm_reset`, `cmd_esp_restart` | ESP / ESP + STM | always |
| button | `cmd_stop`, `cmd_stm_safe_exit` | ESP + STM | STM 2.1 |
| event | `events` (`event_types` = every code that can reach MQTT) | ESP | events |
| sensor (enum) | `diag_stm_lease`, `diag_stm_link` | ESP + STM / ESP | newDiag |
| binary_sensor | `diag_stm_safeMode` | ESP + STM | newDiag |
| sensor | `diag_stm_proto`, `diag_stm_version`, `diag_stm_started`, `diag_stm_resets`, `diag_stm_rxOverflow`, `diag_stm_parseErr` | ESP + STM | newDiag |
| binary_sensor (running) | `diag_calibration_active` | ESP + STM | newDiag |
| sensor (timestamp) | `diag_calibration_next` | ESP | newDiag |
| sensor | `diag_mqtt_eventsSuppressed`, `diag_mqtt_commandsRejected` | ESP | newDiag |

`diag_calibration_next` uses `value_template: {{ value if value else 'None' }}`: its empty
payload (no next slot) shows as unknown, without an "Invalid state message" warning in the HA
log.

The per-valve diag values arrive only with a revamped STM, the coast values
only with STM 2.2.4 or later (before that the two coast entities show
unknown). The 2.0.0 entity `diag_stm_uptime` is removed (replaced by
`diag_stm_started`).

### Discovery list and removed entities

`/HADiscovery.cfg` (LittleFS) holds the list of discovery topics this device
published, one per line, in the legacy format. Every discovery run first
deletes (empty retained config) the listed topics that are no longer current
(renamed or deactivated valves, a changed prefix), then publishes and rewrites
the list; "delete" removes everything in the list. After a rollback, the
legacy firmware's "HA discovery → delete" therefore removes the new entities
too.

Deleted once (first run of the new firmware, modes 1 and 2), for every valve in
both name and number form: `climate/<st>/climate_<R>`,
`number/<st>/valves_control_{dynOffs,min,max}_<R>`,
`select/<st>/valves_window_state_<R>`, `switch/<st>/valves_window_state_<R>`,
`number/<st>/valves_window_target_<R>`, `select/<st>/heatControl`,
`number/<st>/parkPosition`. A 2.0.0 installation is migrated once to the 2.1
layout (availability removed from the kept entities, retired entities deleted).
