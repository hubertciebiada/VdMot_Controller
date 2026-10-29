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
to the manifold. Session files go to `~/flowbench` (`--out` changes it).

Try it without hardware first:

```sh
uv run tools/flowbench/flowbench.py --sim                       # two simulated controllers
uv run tools/flowbench/flowbench.py --sim --sim-auto --sim-speed 20   # also a simulated reader
```

## Before you start

- Stop every automation that sets valve targets (the room controller in Home Assistant). A
  target from MQTT ends the service hold and moves the valve; the tool stops with a message when
  a target that it did not set appears. Keep MQTT and HA online: a lost regulator starts the
  failsafe after its timeout.
- Set the pump to the mode and head that the room controller will use. The flow of one loop
  changes when other loops open, so the tool closes every valve on every controller and opens one
  loop at a time. Write the pump setting and the flow meter settings into the notes.
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
2. Service opens only (30 mA bound). The first one goes 200 pulses into the free travel, then
   60-pulse steps while nothing flows, then steps sized for about 0.15 l/min each (15 to 150
   pulses) from the slope of the last two points.
3. After each step the page waits for the reading: "Nie płynie" (0), a tap on the scale at the
   height of the float disc, or "Powyżej skali". The settle timer is advice only. The scale
   end, the labelled and the plain marks, and the direction (0 at the top, as on the manifold
   flow meters, or at the bottom) are set before the start.
4. The sweep ends at the top of the scale, after 3 steps with less than 0.05 l/min gain, at
   2600 pulses, or when a move stops early. A counted service close back to the end stop
   follows; up and down counts go into the session file.
5. The second sweep starts half a median step later, so the points of both sweeps interleave.
   A third sweep runs when the two disagree by more than 0.12 l/min (rms). The page can end a
   loop early or ask for one more sweep.

Opening steps only: an opening move stops within a few pulses against the insert spring, a
closing move coasts 50..80 pulses that the STM does not count (`software_stm32_rust/core/src/coast.rs`).
The first opening after the end stop also turns 44..98 pulses against the hard stop before the
spindle moves; it is one offset per sweep, the same for every point of that sweep.

The STM keeps a service position for about 5.5 minutes, then returns the valve to its target
(0). A reading after that is void and the sweep starts again; the page shows the time left.

## Data

`flowbench-<date>-<time>.jsonl`, one JSON object per line: `meta` (notes, scale, controller
versions, `oc`/`cc` of every valve), `point` (controller, valve, sweep, counted and requested
position, step, stop reason, reading kind, flow, tap position, both temperature sensors of the
valve), `sweep_end`, `void`, `log`. The page exports the points as CSV (`;`, decimal comma).

STOP stops every valve at once and ends the session; "Przywróć cele sprzed pomiaru" sets the
targets back to what they were at the start. A finished session restores them by itself.
