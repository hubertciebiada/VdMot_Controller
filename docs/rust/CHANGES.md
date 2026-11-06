# VdMot Revamped in Rust: changes from C++ 2.1.7

Every externally visible difference of the Rust firmware from VdMot Revamped C++ 2.1.7, newest
release first. Each entry links to its source of truth. Everything not listed behaves as in
2.1.7: the HTTP API ([docs/revamped/API.md](../revamped/API.md)), MQTT and Home Assistant
([docs/revamped/MQTT.md](../revamped/MQTT.md)), the UART protocol
([docs/revamped/PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md)), the settings and files in
NVS and LittleFS, the STM EEPROM and the STM warm state, in both directions: the C++ and the Rust
firmware read what the other wrote ([PORTING.md](PORTING.md#contract-parity)).

## 2.2.15-revamped

The STM32 code is that of 2.2.9; only its version number changes, so the versions of the two MCUs
stay equal.

### ESP32: a curve of up to 32 points

- `points` of `GET/POST /api/valves/{n}/flow` takes and shows 2..32 pairs (2..16 before); a 33rd
  answers `400 out_of_range` `points 2..32 (enabled needs them)`. The other rules of the points do
  not change. A flow equal to the flow of a point maps to exactly that point's counts, so a curve
  with a point at every target the regulator sets needs no interpolation for them
  ([API.md](../revamped/API.md)).
- NVS: `flow` stays the blob of 2.2.14. Of a curve of more than 16 points it holds 16, the first,
  the last and 14 evenly spaced by index between them, as count 16. The new key `flowpts` (its own
  magic, version and CRC, 22 B + 4 B per point of the longer curves) holds every point of those
  curves; with no curve above 16 points the save removes it. Without it, with a bad one or for a
  valve whose 16 points in `flow` are not the ones of its curve there (a version before 2.2.15
  saved another curve), a valve has the points of `flow`. Going back to 2.2.14 or 2.2.13 runs such
  a curve with those 16 points; after a later 2.2.15 every point is back on a valve whose curve
  the older version did not change ([DESIGN.md](../revamped/DESIGN.md) section 9).
- When `flow` was written and `flowpts` was not (a failed write or removal), the save answers
  `500 nvs`, RAM keeps the old set and the next save reads NVS first, as with `flowlift`.
- A valve is positioned again when a point in use changes, as before; the cells behind the points
  in use no longer count, since `flow` keeps no more than 16 of them.
- RAM: the configuration of a valve grows from 88 B to 152 B; its two copies of 12 (storage and
  the flow layer, both boot blocks on the heap) take 1536 B more. Static RAM does not change. A
  save or a load of the flow configuration takes one transient block of 2564 B (1006 B before).
  The image grows by 1952 B and stays 15.6 KB under its budget of 1,228,800 B.
- `tools/mock_api.py` takes up to 32 points.

### ESP32: a flow below the first flow of the curve

- A flow target above 0 and below the first flow above 0 of the curve lay on the line between the
  point before it and that point: with the points `[0,0]` and `[1070,0.5]` 0.3 l/min was 642
  pulses. It is now the position of that point, 1070 pulses, and `/flow/target` answers
  `clipped:true`. A curve whose first point has a flow kept that point's position for a flow below
  it before; such a flow is now `clipped` as well. A target of 0 still closes, and with R2 a
  position below `floorCounts` + 60 is raised to it after this rule, as before
  ([API.md](../revamped/API.md)).
- Such flows are all one position: a held valve that stands there serves another of them without
  a move.
- `tools/mock_api.py` maps and clips such a flow the same way.

## 2.2.14-revamped

The STM32 code is that of 2.2.9; only its version number changes, so the versions of the two MCUs
stay equal.

### ESP32: R2, a positioning that ends with an opening move

- The valve inserts are normally closed and have a hysteresis loop of 40..55 pulses: the direction
  of the last move picks its branch. The pulse count drifts with small closing moves and with
  moves below the seat, and a move to the open end stop resets it. In a test on six loops two
  positionings to the same target differed by at most 0.03 l/min with R2 (10 pairs with flow) and
  by at most 0.08 l/min with the end stop and one closing move (R1, 12 pairs).
- Two new members of `GET/POST /api/valves/{n}/flow`: `liftCounts` 0 or 60..1000 and
  `floorCounts` 0..10000 and below `strokeCounts`, both 0 by default. A `liftCounts` above 0 needs
  `refSide` `open` and a `floorCounts` above 0; each refusal has its own detail. With `liftCounts`
  0 a valve is positioned as in 2.2.13, so nothing changes until a valve gets a lift
  ([API.md](../revamped/API.md)).
- With `liftCounts` above 0 every positioning to a target between closed and the open end stop
  goes to the open end stop, closes to the target minus the lift and opens the lift (new step 7 of
  event 417). The lift is `liftCounts`, at most the target minus `floorCounts`. The held position
  adds the counted pulses of the lift. `ms` and `peak` of the `flow` block are those of the closing
  move, as the final move's were before: its peak shows a sticky valve.
- A target in l/min or in % below `floorCounts` + 60 goes to that position; a target in l/min
  answers `clipped:true`. `/flow/counts` is not raised: its lift ends at the floor at most, and
  none at or below it.
- A held valve with a lift serves a new target at most 2 pulses from the target it was positioned
  to where it stands, without a move. Several flows below `floorCounts` + 60 are one position, so a
  regulator stepping through them does not send the valve to the end stop each time.
- Any other new target other than 0 goes through the end stop: such a valve has no one move. A
  target of 0 still closes at once, and `minIntervalS` and `deadbandLpm` keep their meaning. A
  change of `liftCounts` or `floorCounts` positions the valve again.
- A closing move that meets the closed end stop (a lift from there would end at a position that
  looks right) or a lift that meets an end stop goes on from the open end stop once; a second such
  end fails the positioning at step 5 or 7 with the code -3 of event 417. `refDelta` of such a
  positioning is that of its first opening from the held position, null without one, never the
  one of an earlier positioning.
- NVS: `flow` stays the blob of 2.2.13, byte for byte; `liftCounts` and `floorCounts` of the 12
  valves go to the new key `flowlift` (58 B, its own magic, version and CRC). Without it or with a
  bad one every lift and floor is 0. Going back to 2.2.13 loses nothing: 2.2.13 ignores `flowlift`
  and runs every valve without a lift; after a later 2.2.14 the lifts are back, except on a valve
  whose stroke or reference side 2.2.13 changed in a way that does not allow them
  ([DESIGN.md](../revamped/DESIGN.md) section 9). When `flow` is written and `flowlift` is not,
  the save answers `500 nvs` and the next save reads NVS first, so RAM and NVS do not differ
  unnoticed.
- MQTT and Home Assistant do not change. `tools/mock_api.py` takes and shows both members with the
  rules of the firmware; its simulation has no lift.

## 2.2.13-revamped
<!-- release-notes: through 2.2.8-revamped -->

This release also carries 2.2.8, 2.2.9, 2.2.10, 2.2.11 and 2.2.12 below, which had no release of
their own. The STM32 code is that of 2.2.9; only its version number changes, and the flash only
keeps the versions of the two MCUs equal.

### ESP32: a failed positioning waits longer

- A failed positioning of the flow layer was tried again every 5 min without end, each try a
  sequence through the end stop. It now waits 5, 10, 20 min, then every 60 min. A new target goes
  out at once and starts the waits again, as do a success, `/flow/counts`, a changed curve, a new
  STM session and a restart of the ESP: the count lives in RAM. Event 417 and `HEALTH_FLOW_FAILED`
  do not change ([DESIGN.md](../revamped/DESIGN.md)).

### ESP32: the retained target of a valve with a flow target

- Without **separate** a retained `valves/<V>/target` older than a target in l/min turned that
  target into its % after a connect. The first message on that topic after a connect on such a
  valve is now ignored, whatever its value. `target/set` works as before
  ([MQTT.md](../revamped/MQTT.md)).

### ESP32: the dashboard lists the valves in rows and shows what is stored

- Valves is one row per valve: name, state, flow in l/min with its target, position in % of the
  stroke, supply, return, delta T, the target field. Below 1000 px wide a row takes three lines
  and the target field appears when the row is open. The state of the flow layer shows as a chip
  while the valve is not held (`positioning`, `position unknown`, `positioning failed`, `needs
  STM 2.2.8`); before this nothing showed a failed positioning. The position shows `–` while the
  layer holds none, not the 0 % the API reports then.
- A row opens to its actions, its diagnostics and the chart of its flow curve with the points, the
  held position and the stored flow settings. New in a row: where the target came from, the STM
  and drive targets, failsafe position, fault, automatic retries, last calibration, held pulses,
  the last positioning and the STM flags.
- New tab Stored, read only, each block named with the place it lies: flow curves (ESP NVS),
  calibration and failsafe (STM EEPROM), the sensors of the valves with raw, offset and temperature,
  the STM parameters, the ESP configuration state, stacks, log and link counters from
  `/api/status` and `/api/health`, the STM images (ESP LittleFS). It also names what cannot be read: the
  plug brake of a valve and the unused fields of the 1.x EEPROM layout.
- The API does not change. A flow document is read when a row opens or on Read all curves, one
  request after the other; the Stored tab reads its documents one after the other, once a minute
  while it is open. The panel grows by about 4.4 KB of gzip; with the nano printf below the image
  stays about 21 KB under its budget of 1,228,800 B.
- The field of a flow target has no upper limit in the browser: the limit it knew could be older
  than the curve. The device clips the flow and the message says so. Below 0 stays refused.
- Enter in a target field sends one target at a time, and Read all curves stays disabled until
  its last document came. A document that cannot be shown no longer stops those after it.
- A screen reader hears the name of each column in a row, and no arrow in the name of a valve. An
  empty cell of a row is no tab stop. An open table of points stays open when the held position
  changes. The axis of a small curve shows 0.05 where it showed 0.1 twice.

### ESP32: MQTT and Home Assistant of the flow layer

- New read topic `valves/<V>/flow` on a valve with the flow layer, one line of JSON:
  `{"state":"held","lpm":1.000,"targetLpm":1.000}`. `lpm` is the flow of the curve at the held
  position, `targetLpm` the target in l/min, each `null` while there is none. It goes out with
  the valve data, on every full publish and on a change at most every `minDelayS`, retained by the
  **retained** setting, with the `/value` suffix under **separate** like the data topics of a
  valve. 2.2.11 removed the topic of 2.2.10 with the drift model; `estLpm` and `holdMin` do not
  return.
- A valve the STM reports that does not show the topic (no layer, or inactive) gets one empty
  retained message on it per MQTT session, whatever **retained** says: a layer switched off leaves
  no state at the broker, also one switched off while the ESP was away, and a retained message of
  2.2.10 goes as well. That is at most 12 empty messages per connect.
- Home Assistant gets three entities on a valve with the flow layer. The number
  `valves_flowTarget_<R>` writes `valves/<V>/flow/set`, 0..`qmax` in steps of 0.1 L/min, QoS 1, and
  reads the target from `valves/<V>/flow`; the sensors `valves_flow_<R>` (L/min) and
  `valves_flowState_<R>` (diagnostic, enum) read the same topic. Discovery has at most 369
  entities (333 before), and a change of the layer of a valve (enabled, `qmax`) starts a run.
- [MQTT.md](../revamped/MQTT.md) corrected: `sync` of a valve with the layer says nothing about
  the positioning. New: restart and broker loss, and which flow events reach MQTT.
- [API.md](../revamped/API.md): `targetLpm` in the example of the valve entry.

### ESP32: newlib's nano printf

- `CONFIG_LIBC_NEWLIB_NANO_FORMAT=y`: `printf` and `scanf` of ESP-IDF and its libraries come from
  newlib's `libc_nano.a` to make the image smaller. Nano has no 64-bit, C99 or positional formats:
  three log texts of the WiFi and PHY libraries show wrong numbers on the serial log, nothing else
  in the image uses such a format ([GLUE-DESIGN-ESP.md](GLUE-DESIGN-ESP.md) 7, item 7).
- With it ESP-IDF no longer adds 512 B to four task stacks. `sys_evt` and `tiT` keep their 3072 B
  through `CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE` and `CONFIG_LWIP_TCPIP_TASK_STACK_SIZE`. `main`
  has 40 960 B instead of 41 472 B and `esp_timer` 3584 B instead of 4096 B
  ([GLUE-DESIGN-ESP.md](GLUE-DESIGN-ESP.md#21-threads-d3)).

### Documentation and tools

- [INSTALL.md section 5.1](INSTALL.md#51-back-to-227): the way back to 2.2.7, as far as its code
  confirms it.
- `tools/mock_api.py`: the stations are `1` and `2`, with 8 and 11 loops named `Loop n`.
- `.gitignore`: the output of the flow tools (`*.jsonl`, `*.db`, `flowbench.csv`, `curves.json`).

## 2.2.12-revamped

### ESP32: the stack of `main`

- `CONFIG_ESP_MAIN_TASK_STACK_SIZE` is 40 960 B, was 32 768 B. The frame of `main` grew from
  22 320 B to 26 704 B with the stored flow target, and the boot of the 2.2.11 image overflowed
  the stack before the logger. It restarted until the boot guard switched back. The stack is
  freed when `main` returns.

## 2.2.11-revamped

### ESP32: the flow layer positions on the stroke, with one move, without the drift

- Gone: the drift model. `driftD`, `holdMin`, `estLpm`, the MQTT topic `valves/<V>/flow`, the
  drift clock and the refresh of a held valve. `POST /api/valves/{n}/flow/target`, MQTT
  `valves/<V>/flow/set` and `HEALTH_FLOW_FAILED` stay. The NVS key `flowx` stays unused: nothing
  reads or writes it, a factory reset drops it. A `driftD` in a config patch is an unknown key.
- Changed meaning: with the layer running, `target`, `requested` and `actual` in the API, on MQTT
  and in Home Assistant are % of the stroke of the valve, not of `qmax`: position = pct x
  `strokeCounts` / 100, rounded; 0 is closed, 100 the open end stop. `actual` is the held pulses
  / `strokeCounts`. `lpm` is the flow of the curve at the held pulses, the top of the curve above
  its last point. `fallbackPct` stays a % of the STM's scale. With **separate** the target topic
  of such a valve shows its desired target instead of the STM's fallback.
- `strokeCounts` is required (1..10000) on an enabled valve, a constant of the curve. A stored
  configuration that is enabled with 0 comes back disabled, the other valves stay.
- A held valve reaches a new target with one service move and `skeep`, without a move to the end
  stop, and without event 416. An end stop on the way continues through the reference end stop.
  The end stop stays for closing, the first opening from 0 %, an unknown position, a lost hold, a
  changed curve, `/flow/counts` and 100 %. `refDelta` comes only from a positioning through the
  open end stop.
- A target in l/min no longer goes through a whole percent: `desired_ml` is kept beside the %,
  to 1 ml/min, and `flow.targetLpm` in `/api/valves` and `GET /api/valves/{n}/flow` shows it (null
  for a target in %). `POST .../target` and `target/set` end it. It survives a restart of the STM
  and of the ESP: the RTC block grows from 104 to 162 B with a record at its end, and NVS gets
  `flowtgt` next to `targets`, whose 46 B stay. An image before 2.2.11 ignores both and restores
  the target as the whole %, of the stroke and no longer of `qmax`.
- A flow above `qmax` is no longer an error: it is clipped to `qmax`, `clipped:true` on REST and
  accepted on MQTT. Between the top of the curve and `qmax` it is the top of the curve, `clipped`
  too. Negative values and non-numbers are refused as before. The error text is
  `lpm a plain number from 0`.
- `deadbandLpm` applies to a target in l/min only, against the flow of the held position. A
  target in % counts in pulses.
- `minIntervalS` defaults to 60 instead of 300. A stored configuration keeps its value.
- The panel shows a valve with a running layer in l/min, one decimal, with the % of the stroke
  beside it, and its field sets l/min in steps of 0.1 up to `qmax`. A legacy valve looks as before.
- The longest `/api/valves` answer is 16 299 B (16 071 before), the response buffer 16 384 B.

## 2.2.10-revamped

### ESP32: a target in l/min and the drift of a held position

- `POST /api/valves/{n}/flow/target` `{"lpm":2.5}` and the MQTT topic `valves/<V>/flow/set`
  (`2.5` or `2,5`, up to three decimals) set the target of a valve with the flow layer in l/min:
  a percentage of `qmax` from there on, on the same path as a target in percent. The answer of
  the POST gives the flow reached, the target and `clipped`. A valve without a running layer
  refuses: `409 flow_off` / `409 unsupported`, MQTT reject `unsupported`
  ([API.md](../revamped/API.md), [MQTT.md](../revamped/MQTT.md)).
- `driftD` per valve (0.1 pulses per e, NVS `flowx`, default 0) turns on the drift model: the
  estimate of the held flow, `estLpm` with the clock `holdMin` in `GET /api/valves/{n}/flow` and
  the new MQTT topic `valves/<V>/flow` (no Home Assistant entity; not in `/api/valves`). When the
  estimate falls `deadbandLpm` below the target and `minIntervalS` passed, the valve is positioned
  again. `/flow/counts` stays exact: no correction, no refresh. With `driftD` 0 a valve behaves as
  in 2.2.9, and a rollback to 2.2.8 reads NVS `flow` unchanged
  ([flow_layer.rs](../../software_esp32_rust/core/src/flow_layer.rs)).
- MQTT `valves/<V>/problem` is 1 while the flow layer failed to position the valve; `common/state`
  shows `info` then.
- The ESP subscribes to `valves/+/flow/set` too, and to the spelled-out filter of a segment with
  a `/` (42 subscriptions at most).
- Event 124 `flow_config_unreadable` has arg2 1 for NVS `flowx`, and then the set stays loaded:
  saves go on.

## 2.2.9-revamped

### STM32: a brake at the end of a service move

- New protocol-3 request `sbrak <idx|255> <maxMs> <gapMs> [<mult>]`: the service moves (`svmov`)
  of the valve end with a brake. On the stopping pulse the direction output flips with the motor
  still enabled, so the motor is driven against its motion for at most `maxMs` (0..100 ms; 0 = no
  brake) or until no pulse came for more than `gapMs` (0..100 ms; 0 = off) or for
  more than `mult` times the pulse interval before the stopping pulse (0..10; 0 = off), whichever
  comes first; then the motor is switched off and the direction output goes back. Without a
  brake the motor coasts 30..65 uncounted
  pulses after an opening move in the low-load part of the stroke (reference measurement on two controllers),
  with a scatter of about ±15; the pulses of the braking motor are counted (`lastCnt` exceeds
  `lastReq` by them). The end-stop limits do not judge the brake current (about twice the
  stall current, bounded by `maxMs`); `sstop` ends a brake with the move. Normal moves,
  calibration strokes and the coast probe are not braked. After a start every valve brakes with
  `35 20 0`: in a reference measurement on two controllers (5 moves per valve) a further opening move missed a median
  of 3 pulses instead of 28..60, the spread of one opening move of ~900 pulses from the closed
  end stop fell from sd 6..24 to a median of 8 pulses, and 60 20 0 was a little worse (median
  5 missed, spread 10); 60 10 0 was unstable and 80 30 0 drove a stopped motor backwards (up to
  8 brake pulses). `sbrak` changes it in RAM until the next STM reset ([PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md),
  [pulse.rs](../../software_stm32_rust/glue/src/motor/pulse.rs)).

## 2.2.8-revamped

### STM32: a service hold that does not run out

- New protocol-3 request `skeep <idx|255> <0|1>`: 1 keeps the service hold of a valve (the
  position a `svmov` or `sstop` left) until something ends it, instead of about 5.5 minutes;
  0 lets it run out again. Everything that ends a hold today ends the keep too (`stgtp`, also with
  an unchanged target, `staop`, `staln`, `stdet 255`, a calibration start, a refused start of the
  service move), and an expired lease ends it so that the failsafe applies when the remaining
  hold has run out. `gvlvy` flag bit 10 `svcKeep`. The keep lives in RAM: an STM reset ends it.
  A valve positioned in pulses (`svmov`) stays there without the periodic `sstop`, which also
  cancels the requested calibrations of the valve
  ([PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md), [app.rs](../../software_stm32_rust/glue/src/app.rs)).

### ESP32: the flow layer

- A valve with a measured curve (`POST /api/valves/{n}/flow`: `qmax`, the end-stop currents,
  the fallback stroke position, the interval and dead band, offset and gain, the reference side
  and 2..16 points of pulses and l/min) takes and reports its target as a flow: 0..100 % means
  0..qmax l/min on `POST /api/valves/{n}/target`, MQTT `valves/<V>/target` and in Home
  Assistant; `pos`/`actual` is the flow percentage of the held position. The ESP positions the
  valve with service moves from the reference end stop (a close at `refmA`, one opening move of
  the curve's pulses at `openmA`; with `refSide` `open` an opening move to the open end stop
  and one closing move of `strokeCounts` minus the pulses, the curve counting from the closed
  end stop on both sides; `preClose` warms the closed end stop first) and keeps it with the
  new STM command `skeep` (STM 2.2.8, flag
  `svcKeep` of `gvlvy`); the STM's own target of the valve is `fallbackPct`, where it goes
  when the hold is lost. `POST /api/valves/{n}/flow/counts` positions to given pulses for the
  verification tool. The valve entry of `/api/valves` gets a `flow` block (state, held pulses,
  flow, the `ms` and `peak` of the final move); events 416 `flow_positioned`, 417
  `flow_position_failed`, 418 `flow_hold_lost`; NVS `flow` (a stored set that cannot be read
  gives the defaults and event 124 `flow_config_unreadable`, and `POST /api/valves/{n}/flow`
  answers `500 nvs` instead of saving the defaults over it). Nothing changes for a valve
  without the layer, and an STM below 2.2.8 keeps every valve in normal mode (`state`
  `unsupported`). `failsafePct` and `sfspo` stay stroke percentages
  ([flow_layer.rs](../../software_esp32_rust/core/src/flow_layer.rs),
  [API.md](../revamped/API.md), [MQTT.md](../revamped/MQTT.md),
  [DESIGN.md](../revamped/DESIGN.md)).
- `POST /api/valves/{n}/brake` `{"maxMs","gapMs","mult"?}` sends the STM 2.2.9 command
  `sbrak` (plug braking at the end of a counted move, both times 0..100 ms, 0 = off, the
  multiplier 0..10 only when above 0, RAM only on the STM) and answers with the STM's reply:
  `200` ok, `400 stm_rejected` with the code, `504 stm_timeout` after 3 s. The only route that
  waits for the STM: the server serves nothing else meanwhile. Nothing is sent for a value
  outside the ranges (`400 out_of_range`), for `maxMs` above 0 with neither `gapMs` nor `mult`
  (`400 invalid`) and below STM 2.2.9 (`409 stm_unsupported`) ([API.md](../revamped/API.md)).

### ESP32: the drift of a valve in the flow layer

- `GET /api/valves/{n}/flow` gets `refDelta` and event 416 the text `ref +24`: with `refSide`
  `open`, the pulses the opening to the end stop counted above the pulses from the held position
  to it. It is the end of the closing move, which no record shows (11..28 pulses per valve with
  the brake, sd 1..7); a change of it over days is the drift of the valve
  ([API.md](../revamped/API.md)).

### ESP32: a 16 KiB response buffer

- The web server's response buffer is 16 KiB (12 KiB up to 2.2.7, the size of a C++ slot):
  `/api/valves` with every member at its longest (twelve valves, escaped names, both sensors,
  `ext` with `v3` and the coast, `calibrationEnd`, the `flow` block) is 16 071 B and
  `/api/sensors` with every slot assigned and every bus sensor seen 12 684 B; both answered
  `500 internal` "document too large" in 12 KiB. 4 KiB more heap, kept from the first request
  ([GLUE-DESIGN-ESP.md](GLUE-DESIGN-ESP.md)).

## 2.2.7-revamped

ESP32 release: a health warning about early stops ends with a successful calibration, and a new
STM image no longer needs a manual delete first. The STM32 firmware is the one of 2.2.6 with the
version 2.2.7-revamped: flash it to keep the versions of both MCUs equal; it needs no update
otherwise.

### ESP32: the health flags of the counters

- The health flags `earlyStop` and `cmdRejected` (`/api/valves` `health`, MQTT, the dashboard)
  compare `earlyStops` and `cmdRejected` with a baseline. 2.2.6 took it at the start of the ESP
  and after a restart of the STM, so a flag stayed until the next restart of the ESP (on a
  reference controller for two days, across the successful calibration that followed the
  early stops within minutes). A
  calibration that ends at idle (the event `calib_ok`) now moves both baselines to the current
  counts as well; a calibration that fails keeps them. The counts themselves (`earlyStops`,
  `cmdRejected`, MQTT `diag/valves/<V>/...`) do not change; the STM clears its calState bit 2
  (`calEarlyStop`) at the same moment
  ([valve_model.rs](../../software_esp32_rust/core/src/valve_model.rs),
  [DESIGN.md](../revamped/DESIGN.md)).

### ESP32: a full STM image store makes room

- `POST /api/stm/images` of a new name into a store with 3 uploaded images (besides
  `last_good`) removes one of them once the upload has completed, instead of `507`
  `too_many_images`: of the images with a version, the one with the lowest version except the
  lowest of all, which stays as the long-term fallback (on the reference controllers the C++
  2.1.7 image `stm217` beside the Rust releases); never the image `last_good` holds (the running
  one, same CRC and size) or one a `last_good` copy reads; of equal versions the one listed
  first counts as the lower. The answer `201` names it in `evicted` (null when nothing was removed), the
  dashboard says so. `507` `too_many_images` stays when no image can be removed: an image
  without a version string is never removed and never the fallback, and nothing is judged while
  an image is not scanned yet (seconds after the start of the ESP or a new `last_good`). A failed
  upload removes nothing ([images.rs](../../software_esp32_rust/glue/src/storage/images.rs),
  [API.md](../revamped/API.md)).

## 2.2.6-revamped

STM32 and ESP32 release: the daily 1-Wire re-scan of the STM no longer stalls its link to the
ESP, and the ESP no longer re-sends the Home Assistant discovery for it. Flash both; either MCU
can be updated first.

### STM32: the 1-Wire search in steps

- The search of `stons` and the re-scan 24 h after the last enumeration walk the bus one device
  per pass of the 10 ms loop: one ROM search pass, for a DS18 sensor also the reads of its power
  supply and resolution, at most about 35 ms. 2.1.7 walked the bus twice in two passes, the
  first one 0.4 to 0.8 s long with 12 to 15 sensors. The ESP waits 400 ms for a reply, so every
  re-scan cost a timeout, a resent request, a stray reply and a degraded link for one second:
  `link_degraded` and `link_up` in the event log once a day, at the time of day the STM started.
  The sensors (`gonec`, `goned`, `gowvc`, `gowvd`, the sensor indexes of the valves) keep the
  last enumeration until the search ends and change at once then; 2.1.7 reported 0 sensors for
  one pass in between. A search takes 2 x (devices + 1) + 1 passes, about 1 s with 15 sensors.
  It still starts only when no valve moves (the re-scan) or at once (`stons`), and runs to its
  end once started, like a temperature cycle. The enumeration at the start of the STM stays one
  walk, before the link starts
  ([ow_devices.rs](../../software_stm32_rust/glue/src/ow_devices.rs)).

### ESP32: no discovery run for an unsettled second

- An automatic Home Assistant discovery run after a change of the discovery inputs compares only
  settled STM inputs (`sensorsSettled`: link up, re-sync done, sensor grace over). The degraded
  second of the daily re-scan cleared them, the valve sensors left the inputs for that second,
  and a full run (142 to 161 retained configs on the reference controllers) followed on about
  one day in three. A change while the STM is not settled (a restart of the STM, `stons`) still
  starts one run once the inputs settled; an STM that never settles gets only the run after a
  connect (at most 120 s later), as before
  ([DESIGN.md](../revamped/DESIGN.md#11-ha-discovery)).

## 2.2.5-revamped

STM32 release: only the closing coast is learned. The ESP32 firmware is the one of 2.2.4 with the version
2.2.5-revamped: flash it to keep the versions of both MCUs equal; it needs no update otherwise.

### STM32: the closing coast only

- A calibration pass probes the coast once: after the third stroke reached the open end stop, a
  closing step of 200 pulses and the move back up. The probe at the closed end stop is gone: its
  move back counts the pulses the motor turns against the hard stop of the actuator until the
  end-stop threshold trips, not the coast (2.2.4, reference measurement on two controllers: more than 100 pulses
  and no sample on most valves). A pass takes two short moves less than in 2.2.4 (about 3 s
  at 0.25 pulses/ms). Normal move pairs refine the coast only from the open end stop (a closing
  step away from it and the move back), and only partial closing moves are shortened: an opening
  step works against the spring of the normally closed insert, which stops the motor within a
  few pulses. `gvlvy` value 26 `coastOpen` stays and is always 0; the ESP reports it as 0
  ([PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md#calibration-and-end-stop-detection)).

## 2.2.4-revamped

STM32 and ESP32 release: the STM learns the coast of each valve and shortens partial moves by it, the ESP
reports the learned coast. Flash both; either MCU can be updated first.

### STM32: the coast of partial moves

- A calibration pass measures the coast of the motor (the pulses it turns after its enable went
  off, which the STM cannot count) with a step of 200 pulses away from each end stop and the move
  back: four short moves more, a pass takes about 6 s longer (at the 0.25 pulses/ms of the
  reference measurement; a due temperature cycle can pause it after each move back as well).
  A pass sets the coast; normal move pairs (a step away from an end stop and the move back)
  refine it, and two pairs in a row that agree with each other but not with it replace it. A
  partial move then stops the learned coast of its direction earlier, by at
  most half of its pulses, so a valve opened or closed in many small steps stays where it is
  counted instead of drifting by the coast of every step. `lastReq` keeps the requested pulses,
  `lastCnt` counts about the coast less. The coast lives in RAM only: the EEPROM and the warm
  state stay those of 2.1.7, a start of the STM begins without a coast. `gvlvy` gets the values
  26 `coastOpen` and 27 `coastClose` (pulses, 0 = not learned)
  ([PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md#calibration-and-end-stop-detection)).

### ESP32: the learned coast

- The ESP reads `gvlvy` values 26 and 27 when the STM sends both and reports them: `/api/valves`
  `ext.coastOpen` / `ext.coastClose` (motor pulses, null below STM 2.2.4), MQTT
  `diag/valves/<V>/coastOpen` / `coastClose` (on change, nothing below STM 2.2.4), two Home
  Assistant diagnostic sensors per valve (`diag_coastOpen_<R>`, `diag_coastClose_<R>`, unknown
  below STM 2.2.4; 333 discovery configs instead of 309 for 12 valves) and the dashboard row Coast
  ([API.md](../revamped/API.md), [MQTT.md](../revamped/MQTT.md#new-entities)). An ESP 2.2.3
  ignores the two values: either MCU can be updated first.

## 2.2.3-revamped

STM32 release. The ESP32 firmware is the one of 2.2.2 with the version 2.2.3-revamped: flash it
to keep the versions of both MCUs equal; it needs no update otherwise.

### STM32: the open end stop of partial moves

- A partial opening move that starts at 50 % or above and stops at the end-stop threshold sets
  the position to 100 % and is never an early end stop (`lastStop` `endstop`); the valve then
  moves to its target. An opening step travels further than its counted pulses, and while
  heating the open end lay up to 6 % below the one of the last calibration before, so a valve
  opened in many small steps reached its open end at a counted 71..96 %: the counted position
  stayed, the next steps stopped early twice and started a calibration that closed the valve
  for about a minute while heating. Closing, a safety stop and a move of a blocked or failed
  valve keep the counted position. A valve that jams while opening above 50 % (a normally
  closed pin stuck while it is pushed in, an actuator stuck while it retracts) is now taken as
  open, without a warning
  ([PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md#calibration-and-end-stop-detection)).

## 2.2.2-revamped

STM32 release. The ESP32 firmware is the one of 2.2.1 with the version 2.2.2-revamped: flash it
to keep the versions of both MCUs equal; it needs no update otherwise.

### STM32: early end stops

- `earlyStops` (`gvlvx`, `/api/valves` `ext.earlyStops`, MQTT `diag/valves/<V>/earlyStops`) and
  calState bit 2 (`calEarlyStop`), and with them the ESP health flag `earlyStop` and the event
  `early_stop`, count only the early end stops the next move did not recover: the second early
  partial stop in a row (which requests a calibration, as before) and an early stop of a move to
  an end stop. The first opening of a valve that stood closed for a long time can stop early
  after about 1 % of the stroke; its retry reaches the target and no longer leaves a
  warning until the next calibration (the health flag: until the next ESP restart). `lastStop`
  `early_endstop` and the one retry do not change
  ([PROTOCOL_V2.md](../revamped/PROTOCOL_V2.md#calibration-and-end-stop-detection)).

## 2.2.1-revamped

ESP32 release. The STM32 firmware is the one of 2.2.0 with the version 2.2.1-revamped: flash it
to keep the versions of both MCUs equal; it needs no update otherwise. The C++ sources left the
repository branch; the C++ 2.1.7 release stays the way back ([INSTALL.md](INSTALL.md)).

### ESP32: MQTT

- The Home Assistant sensor Next calibration (`diag_calibration_next`) gets `value_template`
  `{{ value if value else 'None' }}`: its empty payload (no slot) shows as unknown instead of an
  "Invalid state message" warning in the HA log. The topic and its payload do not change
  ([MQTT.md](../revamped/MQTT.md#new-entities)).

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
- A confirmed image whose boots end four times in a row with a crash, a watchdog reset or a
  start-up hang, each within 10 min of its start, is left for the image of the other slot when
  that image verifies and is not the one that failed the last trial: a Rust image there runs on
  trial with the crashing image as its fallback, the C++ firmware just runs
  ([REVIEW-ESP-SAFETY.md](REVIEW-ESP-SAFETY.md) F5,
  [GLUE-DESIGN-ESP.md §6.2](GLUE-DESIGN-ESP.md#62-decision-at-boot)).
- An ESP upload during the trial is refused with `409 upload_failed` "image on trial"
  ([GLUE-DESIGN-ESP.md §4.7](GLUE-DESIGN-ESP.md#47-behaviour-that-changes), row 10).
- New route `POST /api/system/ota/switch-back` with `{"confirm":"switch-back"}`: `202
  {"result":"restarting"}`, then a restart into the image of the other slot, in any state;
  `400 confirm_required`, `409 busy` (upload or STM flash), `409 restarting`, `409 no_fallback`.
  The C++ firmware answers 404 there
  ([GLUE-DESIGN-ESP.md §6.3](GLUE-DESIGN-ESP.md#63-at-run-time), [§7](GLUE-DESIGN-ESP.md#7-open-risks-and-decisions) item 6).
- Events: restart reason 7 "switch back" (`reboot_requested` arg1 7, Info; C++ 2.1.7 names it
  "unknown"); `esp_ota_failed` arg1 -4 when the image before failed its trial (arg2 1 boot
  limit, 2 health, 3 crash loop after its confirmation). `esp_ota_failed` arg1 -3 (no switch
  possible, the image runs on as confirmed; arg2 3 out of a crash loop) also comes when the
  other slot holds the image that failed the last trial
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
decisions of [§8](GLUE-DESIGN-STM.md#decisions):

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

- An STM image above 16 KiB is flashed in two passes, one of sector 0 and one of sectors 1 and
  up: sector 0 first when the image carries a valid record of its application part (the Rust
  images, [GLUE-DESIGN-STM.md §5.11](GLUE-DESIGN-STM.md#511-the-record-of-the-application-part-b8)),
  last otherwise (the C++ images). The phases erasing, writing and verifying appear twice, the
  byte count runs over both passes, and the percentage counts the bytes written (60 points) and
  verified (20) of both passes, so it never stops for a pass. An erase failure of the pass above
  sector 0 reports the address 0x08004000
  ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations), stm_flasher).
- An interrupted flash leaves the STM without a bootable vector table only during the sector-0
  pass (about 2 s) instead of the whole flash: after an interruption of the other pass the STM
  waits in a Rust boot stage (the new one of a Rust image, or the running one when the C++ image
  replaces a Rust one) and answers the next flash; a flash from C++ to C++ keeps the C++ risk
  ([GLUE-DESIGN-STM.md §5.10](GLUE-DESIGN-STM.md#510-what-the-image-cannot-cover-and-the-half-flashed-image), [§8](GLUE-DESIGN-STM.md#decisions) D9).
- The bytes of sector 0 are read and checked before its erase, and the sector-0 pass has retries
  of its own. When the pass fails after its erase, the ESP does not reset the STM (it would not
  start again): the flash shows the new phase `sector0_pending` (legacy status 8, event
  `stm_sector0_pending`), keeps the STM in its ROM bootloader and repeats the pass every 30 s,
  or at once when the flash is started again (any image). The flash abort is refused then
  ([REVIEW-ESP-SAFETY.md](REVIEW-ESP-SAFETY.md) F4, [INSTALL.md §4.4](INSTALL.md#44-when-something-fails)).
- While the STM's sector 0 is erased and not written, the ESP refuses its restarts: reboot,
  factory reset, switch back, uploads and the flash abort answer `409 stm_sector0_pending`, MQTT
  `cmd/restart` is rejected ("stm sector 0 pending"), and the automatic restarts (network
  watchdog, rollback, heap guard, network settings) wait and log `restart_deferred`
  ([GLUE-DESIGN-ESP.md §4.7](GLUE-DESIGN-ESP.md#47-behaviour-that-changes), row 12).

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
