# VdMot STM32 UART protocol v2 (firmware 2.0.0-revamped)

Protocol v2 adds commands to protocol v1. The v1 request and reply formats are
unchanged, except that `smotc` now answers out-of-range values with
`smotc err`; the changed behaviour of v1 commands is listed below.
An ESP that only speaks v1 keeps working with this firmware, and an ESP that
speaks v2 detects an older STM firmware because `gproto` gets no answer.

## Framing

115200 8N1, one request per line, every reply is one line terminated by CR LF.

A client that may talk to firmware 1.x (every ESP, because it detects the
firmware only by its answers) must send every request, v1 and v2, as

    cmd␠arg1␠arg2␠...␠\n        (CR LF instead of LF is fine)

with a space after the command and after every argument and a final LF.
Firmware 1.x ends a line only at LF and takes a token only when a space
follows it: `stgtp 3 50\n` reaches it with one argument and is ignored, and a
line without any space (`gproto\n`) makes it run the command of the previous
request again.

Firmware 2.x accepts that form and also: CR, LF or CR LF as terminator, one or
more spaces between tokens and no trailing space. A line that is not
terminated within 100 ms after its last byte is dropped (counted in `gstat`
parseErr), so the bytes of a request cut off by an ESP restart do not spoil
the next request. The examples below leave out the trailing space.

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

    > gproto␠
    < gproto 2

Firmware 1.x does not answer `gproto␠\n` (no v1 command starts with
`gprot`). An ESP sends it once after the STM answered `gvers`; on a timeout
it stays in v1 mode.

### `gvlvx idx` – extended valve data

    > gvlvx 3
    < gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140

| field | meaning |
|---|---|
| idx | valve 0..11 |
| status | valve status in the `gvlvd` encoding: bits 0..6 as in `gvlst` (1 idle, 2 opening, 3 closing, 4 failed, 5 unknown, 6 open circuit, 7 full open requested, 8 present / calibration pending, 9 blocked), bit 7 (0x80) the calibration flag of `gvlvd`: set by `staln` and the movement trigger until that calibration ends; calibrations started by the time trigger or by the first target change of a valve found at start-up run with bit 7 clear, so a client detects a requested or running calibration with `calState & 3`, not with this bit |
| pos | believed position 0..100 % |
| target | target position 0..100 % as stored on the STM |
| meanCur | learned mean motor current in mA (20 until the first successful calibration) |
| oc, cc | pulses of the opening and the closing stroke of the last **successful** calibration (0 before) |
| dc | cc − oc, may be negative |
| cr | failed calibration passes of the last calibration |
| moves | position changes since the last calibration (as `gvlvd`) |
| calState | bit field, values 0..15: bits 0..1: 0 idle, 1 calibration requested, 2 calibration running; bit 2 (4): early end stop since the last successful calibration; bit 3 (8): the last calibration did not succeed. A client reads the state as `calState & 3` and the flags separately |
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
| maxmA | 5..60 | end-stop threshold in mA for this move (the 60 mA safety limit and the 100 mA hard limit stay active; like every limit they are not checked during the first 250 ms, see below) |

    > svmov 2 1 500 40
    < svmov 2 ok
    > svmov 2 1 500 70
    < svmov 2 err 1

Error codes: 1 invalid arguments (idx is -1 if it was not a valid valve
index), 2 valve state machine busy (a move or calibration is running or a
command is pending), 3 a calibration of the valve is pending (status 8,
calibration bit set, or a `staln`, time or movement trigger not started yet;
this includes a valve found at start-up until its first calibration): the
calibration would start right after the move and undo it. A client must
accept -1 as idx. The move starts within about 1 s; its result is reported
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
| parseErr | request lines dropped because of invalid characters, more than 5 arguments, or no terminator within 100 ms |
| eepState | 0 ok, 1 write pending, 2 the last write failed (retried after 30 s, 60 s, ... up to every hour), 3 the EEPROM could not be read (defaults in use for what could not be read, nothing is written; the read is retried like a write, and once it succeeds the values changed meanwhile are kept and stored, the rest is taken from the EEPROM) |

An uptime smaller than in the previous reply means the STM restarted.

### `gmotx` – ranges of the motor parameters

    > gmotx
    < gmotx 10 40 10 40 0 100 0 60000 0 2

Minimum and maximum of each `smotc` value in `smotc` order: low factor, high
factor (tenths: 17 = 1.7 × mean current), start-on-power %, minimum pulses
per calibration stroke, calibration repetitions (`smotc` also takes factors
up to 50 and applies them as 40, see below). The same table applies when
the values are loaded from the EEPROM at start-up; a stored value outside its
range loads its default (17, 17, 30, 3000, 2).

## Changed behaviour of v1 commands

- `smotc low high sop [minCnt [maxRetr]]`: every value is checked against its
  range in the `gmotx` table. All in range: reply `smotc` as before. A value
  out of range is not taken (its field keeps the current value), the values in
  range sent with it are applied and stored, and the reply is `smotc err`
  (1.x stored unchecked values). The legacy ESP always sends all five values
  and ignores `smotc err`, so a factor out of range does not lose a new start
  % or repetition count saved with it. Not a number or fewer than 3 values:
  `smotc err`, nothing is changed. More than 5 values: no reply (dropped like
  every request with too many arguments). The factors are limited to 10..40,
  the range 1.x kept across a restart: 1.x `smotc` also took 5..9 and 41..50
  (the legacy web page offers 0.5..5.0) and used them until the next start,
  then fell back to 17. Such a value stored in the EEPROM by 1.x still loads
  17. A factor below 10 puts the end-stop threshold under the running current
  and stops every move at once, so 5..9 is refused. A factor of 41..50 is
  applied as 40 (reply `smotc`, `gmotc` shows 40): with the 15 mA floor a
  factor of 40 already puts the threshold at or above the 60 mA safety limit,
  so a larger factor cannot stop the motor later.
- `stgtp idx pos`: the target is also taken while a calibration of the valve
  is requested or running (status bit 7 set); the calibration ends at the
  newest target. 1.x acknowledged such a target but dropped it, and as the
  calibration flag now stays set until the calibration ends (1.x cleared it
  after about 110 s), dropping would lose targets for the whole calibration.
- `stlnm n`: 0 switches the movement trigger off, 50..65534 are taken as
  they are, 1..49 are stored as 50 and larger values as 65534 (`gtlnm` shows
  the value in use). The same range is loaded at start-up, so the value
  survives a restart; 1.x took any value but loaded only 50..65534 and fell
  back to 2000 otherwise (also for 0).
- `eepst`: `eepst 1` only when the configuration is stored; `eepst 0` while a
  write is pending and also while writing fails or is disabled after a failed
  read (1.x replied 1 in these cases). `gstat` eepState tells the cause.
- `staop idx` / `staop 255`: a failed (4) or blocked (9) valve only gets the
  target 100 (counted in `cmdRejected`) and is not moved until a calibration
  (1.x drove it to the end stop and reported idle without a calibration).
- `staln idx` / `staln 255` and the movement trigger: the calibration starts
  as soon as the valve state machine is idle; 1.x waited until any target had
  changed since the start (the time trigger still waits for it).
- Requests that change the status or position of a valve (`staln`, `staop`,
  `sdetvlv 255`, time and movement trigger) take effect when no valve moves,
  so the end of a running move no longer overwrites them. 1.x lost them:
  the time trigger then waited another learning time, a movement trigger or
  `staln` left the valve ignoring `stgtp` until then, and `sdetvlv 255`
  during a move left that valve untested at a wrong position. `gvlvd` shows
  the new status up to about 0.2 s after the request, or after the running
  move. A time trigger that fires while the valve is calibrating is
  satisfied by that calibration.
- `gvlvd`, `gvlst`: a valve whose calibration failed keeps status 9 (blocked).
  1.x continued with the counts of the failed pass and reported idle.
- Position of failed (4) and blocked (9) valves: the STM no longer sets
  `actual = target` without moving the motor; the position stays where it
  was. The target the valve was last driven to (the target of the move that
  timed out, or of the calibration that blocked) is not counted, nor is a
  target equal to the position the valve stands at; every other target it
  gets while failed or blocked is counted once in `cmdRejected` (also one
  that arrived during the failing move or calibration, and the old target
  again after the valve was set to its current position) and, like any
  target change, lets the calibrations of valves found at start-up begin. A normal
  move that ends with a timeout leaves the position unchanged (1.x reported
  the target). Open-circuit valves (6) keep the 1.x behaviour (position =
  target).
- The EEPROM configuration gets a layout version 2 extension block behind the
  1.x fields (0x013C); 1.x images load with defaults for the new fields, and
  1.x firmware ignores the block.

## Calibration and end-stop detection

- End-stop thresholds are `max(meanCur, 15 mA) × factor` for normal moves,
  service moves use their own threshold. The closing strokes of a calibration
  use `max(meanCur, 20 mA) × factor`: never below the fixed 20 mA × factor
  (34 mA with factor 1.7) that 1.x used for the measured closing stroke, and
  higher for a valve whose learned mean current is above 20 mA (1.x kept
  34 mA for it too). The opening stroke uses `max(meanCur, 15 mA) × factor`
  (1.x: the learned mean without a floor). If valves still block, enable the breakaway
  escalation (`scalx`) or raise the high/low factor.
- The mean current is learned only from a successful calibration pass, from
  strokes with at least 4 samples (about 2 s of running); otherwise the
  previous value stays.
- A pass fails when a stroke has fewer than max(minCnt, 100) pulses. Failed
  passes never change counts, scaler or mean current. After maxRetr
  repetitions the valve is blocked (9) and not moved until the next
  calibration (time trigger, movement trigger or `staln`).
- The 60 mA safety limit trips after more than 10 consecutive 1 ms samples
  above it (1.x counted samples over the whole move, so short spikes in a long
  move added up to a false end stop). The 100 mA hard limit trips at the
  first sample above it.
- Every limit (end-stop thresholds, 60 mA safety limit, 100 mA hard limit)
  compares the filtered current (first-order filter, alpha 0.02). The filter
  is held at 0 during the first 250 ms after a motor start (inrush), so no
  limit trips then, not even for a jammed or shorted motor, and after it the
  filter needs some tens of milliseconds to reach a limit. This is the 1.x
  behaviour; the firmware has no current protection in that window.
- A move to an end stop (0 %/100 % target) that starts at least 50 % of the
  stroke away from that end and stops at a threshold or the safety limit
  after less than half of the travel expected from the learned stroke is an
  early end stop: `lastStop` 3 (or 6), `earlyStops` + 1 and
  calState bit 2. Moves before the first successful calibration are not
  checked.
