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
sweep) and `"fill": [0.15, 0.35, 0.6]` (flows) or `"fill_pos": [525, 600]` (pulses). A fill session measures no sweep: for each flow it takes
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

## End stops without a reader (`endstop_test.py`)

Pulse counts only, no rotameter: per valve, one after the other on each controller (the
controllers run in parallel), the other loops stay on their targets.

```sh
python tools/flowbench/endstop_test.py --controller west=192.168.1.51 --out endstops.jsonl \
    --n positions.json            # {"west 1": 900, ...}: the pulses of the steps phase
python tools/flowbench/endstop_test.py --summary --out endstops.jsonl
```

| phase | moves | result |
|---|---|---|
| closed | close at 34 mA, then 5 × (open 300, close at 34 mA) | the pulses of each close: the repeatability of the closed end stop (the first close counts more) |
| steps | 2 × (one opening move of N, close), 2 × (N in 5 moves, close) | the pulses the counter misses per opening move: (way back after 5 moves − way back after 1 move) / 4 |
| open | open to the open end stop, then 5 × (close 300, open to the end stop) | the repeatability of the open end stop |
| zero | 2 × (0 %, the target back, 0 %, close at 34 mA) | how far the normal 0 % of the STM lies above the 34 mA stop |
| ruler | 5 × (close at 34 mA, open to the open end stop) | the spread of the counted stroke: does the 34 mA stop repeat (the first move after `--cold-s` idle: the cold motor) |
| top | open to the end stop, then 5 × (close K, open to the end stop), K = stroke − the position of 1.0 l/min | the same point reached from above: does it repeat better than from the closed stop |

| brake | per `--brakes` setting (`maxMs,gapMs,mult;...`, STM 2.2.9 `sbrak` through `POST /api/valves/{n}/brake`): a warm-up move, the steps phase, 5 × (open N, close at 34 mA), with `--pos` the top phase; at the end the brake of the STM after a start (`35,20,0`) | the missed pulses per opening move with and without the brake, the spread of one move, the brake pulses (counted − requested) |
| relative | with `--pos`: 3 × (close K, open to the end stop), 3 × (close K, open 300, open to the end stop), 3 × (close K, 5 pairs of open and close, open to the end stop) | what one more stop loses near the working point, how far a pair of relative moves shifts the valve (`relative` in the summary); counts alone do not show the backlash |

`--phases` picks the phases (default `closed,steps,open,zero`); `top` needs `--pos`
(`{"west 1": 810, ...}`, the pulses of 1.0 l/min). At the end the valve gets 0 % and then its
target again. Every move is a JSON line in `--out`.

## Checking the curves on the flow layer (`flowcheck.py`)

Needs firmware 2.2.8 with the flow layer (`/api/valves/{n}/flow`): the controller positions each
valve itself (a move to the reference end stop, one move away from it, the hold kept with
`skeep`), the tool only asks. Its own page (Polish), port 4681, data in `~/flowbench/flowcheck.db`.

```sh
python tools/flowbench/flowcheck.py --controller west=192.168.1.51 upload curves.json   # points, layer off
python tools/flowbench/flowcheck.py --controller west=192.168.1.51 --controller east=192.168.1.52
python tools/flowbench/flowcheck.py --sim --sim-auto                                    # no hardware
```

`--ha-power ENTITY` (with `HA_URL` and `HA_TOKEN` in the environment) shows the pump power on the
page and refuses a session below 5 W; an unknown power does not stop it. `--pick "west 3,east 7"`
marks the loops of the repeat session on the page.

Every session starts the same way: the target of every valve is noted, the loops outside the
session close (the curves were measured with one loop open, so the pump keeps its head), the layer
goes on for the session loops. The page marks a pump power above 70 W: near saturation (about 77 W here)
the head sags, so a reading there does not fit the curve. At the end, also after STOP or an error, the layer goes off and every
valve goes back to the noted target. A layer that lets go leaves the valve at `fallbackPct` while
the ESP keeps the old target as desired, and the ESP sends only a changed target, so a session loop
goes over another target first (5 % on the way to 0 %). A valve that does not get back is named on
the page. Stopping the tool with Ctrl-C does the same; killing it leaves the layers on.

An entry of the curves file can also carry `refSide` (`"closed"` or `"open"`), `strokeCounts`,
`offset` and `gain`; `upload` writes them with the points. With `"open"` the controller references
each positioning at the open end stop and closes `strokeCounts − counts`: the open end repeats
within 2 pulses, the closed end stop at `refmA` within 6..33.

Sessions (buttons on the page):

- **repeat**: the counts of 1.0 l/min 5 times on the marked loops (a loop with `refSide` `closed`
  closes first; with `open` every positioning starts at the open end anyway); the reader types what
  the float shows (no expected value on the card). The page keeps the readings, mean and standard
  deviation per loop (passes at 0.1 l/min), and the session ends with an offset-only fit.
- **house**: every loop on 1.0 l/min at once, one reading each: how the loops pull on each other.
  A typed reading gives an offset-only fit of that loop.
- **check**: one group of a manifold (3–4 loops) through a list of stops (`short`: 1 / 2 /
  0.5 l/min, the default; `mixed`: 0.25 / 0.5 / 1 / 1.5 / 2 l/min, each twice and never twice in a
  row; also `up`, `down`, `zigzag`). Each
  card shows the expected flow with `OK` (within 0.1 l/min), `Za dużo`, `Za mało` and a typed
  reading (`Nie płynie`, `Powyżej skali`). Too much / too little moves the valve by 0.1 l/min of
  the local slope (1.5 × after the same answer twice); a typed reading moves it by the secant of
  the curve. Only the corrected cards are asked again; when a stop had corrections, the group is
  confirmed once in its last setting, because one valve changes the flow of the others.

After a move the page counts down 15 s for the float to come to rest; the answers are open all the
time. A reading can be typed (with a comma or a dot) and saved with Enter. A valve that loses its
hold while its card waits ends the session, because the layer takes it to 0 % on its own.

After a check session the tool fits per loop `counts = offset + gain × (counts of the curve)` from
the OK and typed readings (least squares; the offset only with fewer than 3 points or a narrow
range) and shows it on the page; `Zapisz` writes it into the controller. A loop whose layer was off
gets it on for the session only.
