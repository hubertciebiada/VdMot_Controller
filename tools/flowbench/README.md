# flowbench

Measures the flow characteristic of each heating loop: flow in l/min against the actuator
position in pulses from the closed end stop. The flow comes from the loop's float rotameter on
the manifold; a phone page shows a model of the rotameter and the user taps where the float is.
The page is in Polish.

The result is a table per loop that a room controller can use to turn a wanted flow into a
valve position (for example a controller that works with heat power = flow × ΔT).

## Run

Python 3.10 or newer, standard library only.

```sh
uv run tools/flowbench/flowbench.py --controller west=192.168.1.51 --controller east=192.168.1.52
```

Open the printed address (`http://<PC>:4680`) on a phone in the same network and take the phone
to the manifold. Sessions go to `~/flowbench/flowbench.db` (see Data).

Try it without hardware first:

```sh
uv run tools/flowbench/flowbench.py --sim                       # two simulated controllers
uv run tools/flowbench/flowbench.py --sim --sim-auto --sim-speed 20   # also a simulated reader
```

The pump power can come from a Home Assistant power sensor (for example a smart plug). With
`--ha-power sensor.pump_power` and `HA_URL` / `HA_TOKEN` (a long-lived access token) in the
environment, every reading stores the power at that moment. A failed read stores no power and
does not stop the measurement.

## Before you start

- Stop every automation that sets valve targets (the room controller in Home Assistant). A
  target from MQTT ends the service hold and moves the valve; the tool stops with a message when
  a target that it did not set appears. Keep MQTT and HA online: a lost regulator starts the
  failsafe after its timeout.
- Set the pump to the mode and head that the room controller will use. The flow of one loop
  changes when other loops open, so the tool closes every valve on every controller and opens one
  loop at a time. Type the pump head into "Ciśnienie na pompie" and the pump mode and
  the flow meter settings into the notes.
- Every valve must be calibrated and idle. A scheduled calibration inside the session stops the
  service moves (`calib.dayMask`, `calib.hour` in the settings).
- Stop every automation that switches the circulation pump off (for example a smart socket that
  cuts the pump when all valves are closed). The tool closes all valves between loops.

The page has two modes: "Charakterystyka pętli" (the method below) and "Dobór ciśnienia pompy".

## Loops file

`~/flowbench/loops.json` (`--loops` changes it; `--sim` uses `loops.example.json`) gives each
loop its pipe length and the design heat of the floor it covers. The page uses it for the design
flow (Φ / (c·ρ·ΔT)) and the calculated head of each loop (Darcy–Weisbach in the pipe, laminar
64/Re, Blasius above Re 4000, linear between; the rotameter at Kvs 1.1 and the return valve at
Kvs 2.4). Without the file the design flows are typed on the page. The file holds room names;
keep it out of the repository.

## Pump head

The tool sets the read loops (by default the two with the highest calculated head) to 100 % and
every other loop to a part opening (15 %), so the pump works against a load near the design case.
Then, for each head:

1. Set the head on the pump, type it (and the socket power, if known) and press "Gotowe, ustawione".
2. The page waits 60 s, then asks for the rotameter of each read loop ("odczytaj przy tym
   ciśnieniu też wszystkie pozostałe pętle" adds the other loops).
3. A head passes when every read loop reaches its design flow or goes above the scale.

Start high and step down until a head fails. The result is the lowest passing head, the same
head with a flow margin (head × (1 + margin)², since the head grows with the square of the flow
in turbulent flow) and the head lost outside the loops (pump head minus the calculated loop
head at the measured flow): the pipes from the buffer and the manifold beams.

A higher head than needed narrows the valve band where the flow changes, so the loop
characteristic must be measured at the head the room controller will use.

## Method

Per loop, per sweep:

1. A service close to the closed end stop (34 mA, the calibration closing bound) is the
   reference.
2. Service opens only (30 mA bound). The first one goes 200 pulses into the free travel, or to
   two 60-pulse steps below the onset known from before (the last zero before the flow in the
   latest session of the same loop, else the median of the loops measured in this session).
   While nothing flows the step grows by half from 60 pulses (60, 90, 135, 150), then steps
   are sized for "Przyrost na krok" each (0.25 l/min on the page, 15 to 150 pulses) from the
   slope of the last two points; below 0.75 l/min they aim at 0.1 l/min.
3. After each step the page waits for the reading: "Nie płynie" (0), the float disc dragged to
   the value, or "Powyżej skali". The disc snaps to 0.1 l/min ("co 0,025" for 0.025) and stays
   on the last reading, so the next one is a short drag and "Zatwierdź odczyt". The page sends
   one reading per point. The settle timer is advice only. After a sweep the panel below the
   scale offers the next sweep as the main button while the tool wants one, so a tap meant
   for "Zatwierdź odczyt" does not end the loop. The scale
   end, the labelled and the plain marks, and the direction (0 at the top, as on the manifold
   flow meters, or at the bottom) are set before the start.
4. The sweep ends at the top of the scale, after 3 steps with less than 0.05 l/min gain, at
   2600 pulses, or when a move stops early. A counted service close back to the end stop
   follows (room for 100 coast pulses per step, and up to two more closes if it ends on its
   count); up and down counts go into the `sweeps` table.
5. The second sweep starts at the last zero reading of the first sweep, so it does not walk
   through the free travel again, and steps half the gap between that zero and the first flow
   of sweep 1 while nothing flows. A third sweep starts half such a step lower, so its points
   fall between those of the second.
   A third sweep runs when the two disagree by more than 0.12 l/min (rms). The page can end a
   loop early or ask for one more sweep.

Opening steps only: an opening move coasts a few pulses past its count against the insert
spring, a closing move coasts 50..80 pulses (`software_stm32_rust/core/src/coast.rs`). The STM
counts neither. After j opening steps the spindle is about j coasts further than the count; the
counted way back to the end stop holds the sum, so at the sweep end every point gets
`pos_est` = counted position + j × (way back − way up) / steps. `pos_est` is the count that one
move from the closed end stop shows for the same spindle position, which is how a valve later
goes to a position. The first opening after the end stop also turns 44..98 pulses against the
hard stop before the spindle moves; one move from the end stop has the same offset.

"Przebieg zamykający (histereza)" adds one closing sweep per loop after the opening sweeps: one
move from the end stop to where the last opening sweep reached the top of the scale, then
closing steps of 40 requested pulses with a reading after each, until the flow stops. The
one move goes to the `pos_est` of that point, since a single move has no summed coasts. The
counted way back to the end stop gives the sum of the closing coasts; each step gets an equal
part in `pos_est`. Opening and closing curves on `pos_est` show the hysteresis of the valve.

A session started through `POST /api/start` (for example with a JSON file and a small script)
can also set `"sweeps": 1` (one sweep per loop, no repeat), `"decide_s"` (the wait after a
sweep) and `"fill": [0.15, 0.35, 0.6]`. A fill session measures no sweep: for each flow it takes
the position from the latest curve of the loop in the database (`pos_est`, a sweep without a
zero starts from the end stop), sets it with one move from the closed end stop and puts the
disc on the expected flow; the reader confirms it or moves the disc. Fill points are where the
curve is steepest and a stepped sweep jumps: near the onset 100 pulses can add 1 l/min.

Firmware 2.2.7: the ESP reports a service move (`moveSeq`, `service_move_done`) only when the
last-move record of the STM differs from the one before, so two equal moves in a row look like
one. The tool asks one pulse more for a move equal to the last one, and takes the end of a move
from the record (direction and pulses asked), not from the event, which the ESP logs a second
later and can belong to the move before.

The STM keeps a service position for about 5.5 minutes, then returns the valve to its target
(0). While a reading is due, the tool sends a valve stop every 2 minutes; the STM stop leaves a
stopped valve where it is and restarts the hold, so a reading can take as long as it needs. If
the hold still runs out (a stop failed), the reading is void and the sweep starts again; the page
shows the time left.

## Data

Every session goes into one SQLite file, `~/flowbench/flowbench.db` (`--out` changes the
folder; `--sim` writes to `~/flowbench/sim`, so simulated sessions never mix with measured
ones). The tool writes each reading as it comes, so a stopped or crashed session keeps its rows.

| Table | One row per |
|---|---|
| `sessions` | session: start, end, `status` (running, done, stopped, error), `mode`, `pump_head_m`, notes, `meta` (JSON: scale, controller versions, `oc`/`cc` of every valve, head test settings) |
| `points` | loop reading: controller, valve, sweep, `direction` (open, close), counted and requested position, `pos_est` (corrected for the coast, set at the sweep end), step, stop reason, `kind` (value, zero, over, void), flow, tap position, both temperature sensors, `power_w` (pump power, with `--ha-power`) |
| `sweeps` | end of a sweep: reason, pulses up, counted pulses back to the end stop |
| `head_points` | pump head reading: head, socket power (typed, else from `--ha-power`), valve target, flow, design flow |
| `head_steps` | pump head step: passed or not, the flows, the recommendation |
| `events` | log line or void sweep |

A late reading voids its whole sweep: those points get `kind = 'void'` and the sweep runs again.
`pump_head_m` is required for a loop characteristic, since the flow at a position depends on
the head.

The latest characteristic of one loop:

```sql
SELECT p.pos_est, p.flow_lpm, p.kind, p.sweep, p.direction
FROM points p JOIN sessions s ON s.id = p.session_id
WHERE p.ctrl = 'west' AND p.valve = 3 AND p.kind <> 'void'
  AND s.id = (SELECT max(session_id) FROM points WHERE ctrl = 'west' AND valve = 3)
ORDER BY p.direction, p.pos_est;
```

The schema version is `PRAGMA user_version`. A newer tool brings an older file up to its schema
on start (`MIGRATIONS` in `flowbench.py`); an older tool refuses a newer file and stops. The page also
exports the points of the current session as CSV (`;`, decimal comma).

STOP stops every valve at once and ends the session; "Przywróć cele sprzed pomiaru" sets the
targets back to what they were at the start. A finished session restores them by itself.
