# VdMot STM32 UART protocol v2 (firmware 2.0.0-revamped)

Protocol v2 adds commands to protocol v1; it does not change a v1 command, except
that `smotc` now answers out-of-range values with `smotc err` (see below).
An ESP that only speaks v1 keeps working with this firmware, and an ESP that
speaks v2 detects an older STM firmware because `gproto` gets no answer.

## Framing

Same as v1: 115200 8N1, one request per line `cmd arg1 arg2 ...` terminated by
CR, LF or CR LF, arguments separated by one or more spaces (a trailing space is
optional). Every reply is one line terminated by CR LF.

- v2 replies are space-separated decimal integers without a trailing space.
- Numbers must be plain decimal digits; a sign, a fraction or a value outside
  the documented range makes the request invalid.
- Unknown commands get no reply (v1 behaviour). Invalid arguments get no reply
  unless the command documents an `err` reply.
- Lines longer than 127 characters and lines with bytes other than printable
  ASCII or TAB are dropped and counted (`gstat`).

## Commands

| cmd | args | reply |
|---|---|---|
| `gproto` | – | `gproto 2` |
| `gvlvx` | idx | `gvlvx idx status pos target meanCur oc cc dc cr moves calState earlyStops cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs` |
| `gprof` | idx | `gprof idx n c1:m1 ... cn:mn` |
| `svmov` | idx dir counts maxmA | `svmov idx ok` / `svmov idx err code` |
| `scalx` | enable stepPct maxmA | `scalx ok` / `scalx err` |
| `gcalx` | – | `gcalx enable stepPct maxmA` |
| `gstat` | – | `gstat uptime resets bootReason rxOverflow parseErr eepState` |
| `gmotx` | – | `gmotx lowMin lowMax highMin highMax sopMin sopMax minCntMin minCntMax retrMin retrMax` |

`idx` is always the 0-based valve index 0..11. A request for another index
gets no reply (except `svmov`).

### `gproto` – protocol version

    > gproto
    < gproto 2

Firmware 1.x does not answer. An ESP sends `gproto` once after the STM
answered `gvers`; on a timeout it stays in v1 mode.

### `gvlvx idx` – extended valve data

    > gvlvx 3
    < gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140

| field | meaning |
|---|---|
| idx | valve 0..11 |
| status | valve status as in `gvlst` (1 idle, 2 opening, 3 closing, 4 failed, 5 unknown, 6 open circuit, 7 full open requested, 8 present / calibration pending, 9 blocked); without the calibration bit 0x80 of `gvlvd` |
| pos | believed position 0..100 % |
| target | target position 0..100 % as stored on the STM |
| meanCur | learned mean motor current in mA (20 until the first successful calibration) |
| oc, cc | pulses of the opening and the closing stroke of the last **successful** calibration (0 before) |
| dc | cc − oc, may be negative |
| cr | failed calibration passes of the last calibration |
| moves | position changes since the last calibration (as `gvlvd`) |
| calState | bits 0..1: 0 idle, 1 calibration requested, 2 calibration running; bit 2 (4): early end stop since the last successful calibration; bit 3 (8): the last calibration did not succeed |
| earlyStops | early end stops of normal moves since start-up (see below) |
| cmdRejected | target changes the STM did not execute because the valve is failed (4) or blocked (9), since start-up |
| lastDir | direction of the last move: 0 open, 1 close |
| lastReq | requested pulses, 65535 = run to the end stop |
| lastCnt | counted pulses |
| lastStop | 0 none (no move since start-up), 1 target count reached, 2 end stop, 3 early end stop, 4 timeout, 5 undercurrent (open circuit), 6 safety overcurrent, 7 aborted (the motor could not be started) |
| lastPeak | largest filtered motor current of the move in 0.1 mA |
| lastMs | duration of the move in ms (0 when it did not start) |

The last move is any motor move: a normal position change, each of the three
strokes of a calibration, or a service move. All counters start at 0 after a
reset of the STM.

### `gprof idx` – current profile of the last move

    > gprof 7
    < gprof 7 5 0:0 1024:171 2048:176 3072:180 4012:402

`n` (0..32) pairs `count:current`: the pulse count and the filtered motor
current in 0.1 mA (absolute value) of the last move of the valve. The samples
are on an equally spaced pulse grid (1, 2, 4, ... pulses, chosen so that at
most 32 samples cover the move); the last pair is the point where the motor
stopped. The current is 0 during the first 250 ms (inrush time, not
evaluated by the end-stop detection). Kept in RAM only.

### `svmov idx dir counts maxmA` – service move

| arg | range | meaning |
|---|---|---|
| dir | 0, 1 | 0 open, 1 close |
| counts | 1..10000 | pulses to move |
| maxmA | 5..60 | end-stop threshold in mA for this move (the 60 mA safety limit and the 100 mA hard limit stay active) |

    > svmov 2 1 500 40
    < svmov 2 ok
    > svmov 2 1 500 70
    < svmov 2 err 1

Error codes: 1 invalid arguments (idx is -1 if it was not a valid valve
index), 2 valve state machine busy (a move or calibration is running or a
command is pending). The move starts within about 1 s; its result is reported
by `gvlvx` (lastStop 1 = all pulses moved, 2 = threshold reached) and
`gprof`.

After the move the believed position follows the counted pulses (converted
with the learned scaler), and the STM leaves the valve there until the next
`stgtp`, `staop` or `staln` for it, a calibration of it, or for about
5.5 minutes, then it returns to its target. A failed, blocked, open-circuit or pending-calibration status
is kept (only a calibration clears it); an idle valve becomes open circuit
after an undercurrent and failed after a timeout.

### `scalx enable stepPct maxmA` / `gcalx` – breakaway escalation

| arg | range | default |
|---|---|---|
| enable | 0, 1 | 0 |
| stepPct | 0..100 | 25 |
| maxmA | 20..60 | 50 |

    > scalx 1 25 50
    < scalx ok
    > gcalx
    < gcalx 1 25 50

When enabled, repetition n (n ≥ 1) of a failed calibration raises the end-stop
thresholds of its strokes by n × stepPct percent (linear: +25 %, +50 %, ...),
but not beyond maxmA; a threshold that is already above maxmA is not changed.
Normal moves always use the unescalated thresholds. Stored in the EEPROM.

A high threshold can let a latching valve adapter jump off without the
end stop being detected; keep maxmA well below the current at which the
adapter releases.

### `gstat` – health

    > gstat
    < gstat 86400 3 4 0 2 0

| field | meaning |
|---|---|
| uptime | seconds since the start of the application (does not wrap after 49 days) |
| resets | resets since the last power-on (0 after power-on or brown-out) |
| bootReason | cause of the last reset: 0 unknown, 1 power-on, 2 reset pin (e.g. by the ESP), 3 software (`reset`, after flashing), 4 independent watchdog, 5 window watchdog, 6 low power, 7 brown-out |
| rxOverflow | request lines dropped because they were too long |
| parseErr | request lines dropped because of invalid characters or more than 5 arguments |
| eepState | 0 ok, 1 write pending, 2 the last write failed, 3 the EEPROM could not be read at start-up (defaults in use, changes are not stored) |

An uptime smaller than in the previous reply means the STM restarted.

### `gmotx` – ranges of the motor parameters

    > gmotx
    < gmotx 5 50 5 50 0 100 0 60000 0 2

Minimum and maximum of each `smotc` value in `smotc` order: low factor, high
factor (tenths: 17 = 1.7 × mean current), start-on-power %, minimum pulses
per calibration stroke, calibration repetitions. The same table applies when
the values are loaded from the EEPROM at start-up; a stored value outside its
range loads its default (17, 17, 30, 3000, 2).

## Changed behaviour of v1 commands

- `smotc low high sop [minCnt [maxRetr]]`: all values are checked against the
  `gmotx` table. Valid: reply `smotc` as before. Out of range, not a number or
  fewer than 3 values: reply `smotc err` and nothing is changed (1.x stored
  unchecked values, or ignored the request without a reply). More than 5
  values: no reply (dropped like every request with too many arguments). Values 5..9 and
  41..50 of the factors now survive a restart (1.x fell back to 17 at the next
  start).
- `staln idx` / `staln 255`: the calibration starts as soon as the valve state
  machine is idle; 1.x waited until any target had changed since the start.
  A request that a running move overwrote is renewed.
- `gvlvd`, `gvlst`: a valve whose calibration failed keeps status 9 (blocked).
  1.x continued with the counts of the failed pass and reported idle.
- Position of failed (4) and blocked (9) valves: the STM no longer sets
  `actual = target` without moving the motor; the position stays where it
  was, and every new target is counted in `cmdRejected`. A normal move that
  ends with a timeout leaves the position unchanged (1.x reported the target).
  Open-circuit valves (6) keep the 1.x behaviour (position = target).
- The EEPROM configuration gets a layout version 2 extension block behind the
  1.x fields (0x013C); 1.x images load with defaults for the new fields, and
  1.x firmware ignores the block.

## Calibration and end-stop detection

- End-stop thresholds are `max(meanCur, 15 mA) × factor` for every move. The
  closing stroke of a calibration uses the learned mean current too (1.x used
  a fixed 20 mA, i.e. 34 mA with factor 1.7). For a valve with a learned mean
  current of 16–17 mA this lowers the closing threshold from 34 mA to about
  28 mA; enable the breakaway escalation (`scalx`) or raise the high/low
  factor if valves then block.
- The mean current is learned only from a successful calibration pass, from
  strokes with at least 4 samples (about 2 s of running); otherwise the
  previous value stays.
- A pass fails when a stroke has fewer than max(minCnt, 100) pulses. Failed
  passes never change counts, scaler or mean current. After maxRetr
  repetitions the valve is blocked (9) and not moved until the next
  calibration (time trigger, movement trigger or `staln`).
- The 60 mA safety limit trips after more than 10 consecutive 1 ms samples
  above it (1.x counted samples over the whole move, so short spikes in a long
  move added up to a false end stop). The 100 mA hard limit trips at once.
- A move to an end stop (0 %/100 % target) that stops at a threshold or the
  safety limit after less than half of the travel expected from the learned
  stroke is an early end stop: `lastStop` 3 (or 6), `earlyStops` + 1 and
  calState bit 2. Moves before the first successful calibration are not
  checked.
