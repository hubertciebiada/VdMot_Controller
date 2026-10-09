# VdMot STM32 UART protocol v2 and v3 (firmware 2.1.0-revamped)

Protocol 2 (firmware 2.0.0-revamped) and protocol 3 (2.1.0-revamped) add
commands to protocol v1. The v1 request and reply bytes are unchanged, with
two exceptions: `smotc` answers out-of-range values with `smotc err`, and
`stdet x` with x != 255 answers `stdet err`. The changed behaviour of v1
commands is listed below. An ESP that only speaks v1 keeps working with this
firmware, and an ESP that speaks v2/v3 detects an older STM firmware because
`gproto` gets no answer (1.x) or answers 2 (2.0.0).

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

- v2 and v3 replies are space-separated decimal integers without a trailing
  space.
- Numbers must be plain decimal digits; a sign, a fraction or a value outside
  the documented range makes the request invalid.
- Unknown commands get no reply (v1 behaviour). Invalid arguments get no reply
  unless the command documents an `err` reply.
- Lines longer than 127 characters and lines with bytes other than printable
  ASCII or TAB are dropped and counted (`gstat`).
- A new command never shares its first 5 characters with a v1 command and
  never starts with `ESP`.

**Extension rule.** Replies of the protocol 3 commands (`gvlvy`, `gstax`,
`glcfg`, `slhbt`) may get more numeric fields appended in later firmware; a
client must accept at least the documented count and ignore the rest
(ESP 2.1 does). `gvlvx` stays at exactly 19 values and `gstat` at exactly 6.

## Commands

Protocol 2:

| cmd | args | reply |
|---|---|---|
| `gproto` | – | `gproto 3` (2.0.0: `gproto 2`) |
| `gvlvx` | idx | `gvlvx idx status pos target meanCur oc cc dc cr moves calState earlyStops cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs` |
| `gprof` | idx | `gprof idx n c1:m1 ... cn:mn` |
| `svmov` | idx dir counts maxmA | `svmov idx ok` / `svmov idx err code` |
| `scalx` | enable stepPct maxmA | `scalx ok` / `scalx err` |
| `gcalx` | – | `gcalx enable stepPct maxmA` |
| `gstat` | – | `gstat uptime resets bootReason rxOverflow parseErr eepState` |
| `gmotx` | – | `gmotx lowMin lowMax highMin highMax sopMin sopMax minCntMin minCntMax retrMin retrMax` |

Protocol 3 (section "Protocol 3 commands"): `slhbt`, `slcfg`, `sfspo`,
`glcfg`, `gvlvy`, `gstax`, `sstop`, `gtlnt`, `ssafe`.

`idx` is always the 0-based valve index 0..11. A request for another index
gets no reply (except `svmov`, `sfspo` and `sstop`).

### `gproto` – protocol version

    > gproto␠
    < gproto 3

Firmware 1.x does not answer `gproto␠\n` (no v1 command starts with
`gprot`). How the ESP 2.1 uses it:
- `gproto` is the first step of every re-sync (ESP start, detected STM
  restart, link recovered, after flashing) and goes out alone, before
  `gvers`. It is sent with the normal retries (up to 3 attempts, 400 ms each);
  its timeouts never count towards "link down".
- No answer selects protocol 1; `gproto 2` or `gproto 3` selects that
  protocol (a higher value counts as 3).
- When a later `gvers` reports a revamped version while the link runs in
  protocol 1 (the probe may have hit the STM's start-up window), `gproto` is
  probed once more; an answer restarts the re-sync with the new protocol.
  `gvers` is read every 5 min, so a revamped STM does not stay in protocol 1.

### `gvlvx idx` – extended valve data

    > gvlvx 3
    < gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140

| field | meaning |
|---|---|
| idx | valve 0..11 |
| status | valve status in the `gvlvd` encoding: bits 0..6 as in `gvlst` (1 idle, 2 opening, 3 closing, 4 failed, 5 unknown, 6 open circuit, 7 full open requested, 8 present / calibration pending, 9 blocked), bit 7 (0x80) the calibration flag of `gvlvd`: set by `staln` and the movement trigger until that calibration ends; calibrations started by the time trigger, by an automatic retry or by the first target change of a valve found at start-up run with bit 7 clear, so a client detects a requested or running calibration with `calState & 3`, not with this bit. A blocked (9) valve stands at its failsafe position (see "Protocol 3 commands") |
| pos | believed position 0..100 % |
| target | target position 0..100 % as stored on the STM (the position the valve is driven to can differ, see `gvlvy` drive) |
| meanCur | learned mean motor current in mA (20 until the first successful calibration) |
| oc, cc | pulses of the opening and the closing stroke of the last **successful** calibration (0 before; restored from the EEPROM after a restart) |
| dc | cc − oc, may be negative |
| cr | failed calibration passes of the last calibration |
| moves | position changes since the last successful calibration of this valve (as `gvlvd`); `stlnm` also resets the counters of all valves |
| calState | bit field, values 0..15: bits 0..1: 0 idle, 1 calibration requested, 2 calibration running; bit 2 (4): early end stop since the last successful calibration; bit 3 (8): the last calibration did not succeed. A client reads the state as `calState & 3` and the flags separately |
| earlyStops | early end stops of normal moves since start-up (see below) |
| cmdRejected | target changes the STM did not execute because the valve is failed (4) or blocked (9), since start-up |
| lastDir | direction of the last move: 0 open, 1 close |
| lastReq | requested pulses, 65535 = run to the end stop |
| lastCnt | counted pulses |
| lastStop | 0 none (no move since start-up), 1 target count reached, 2 end stop, 3 early end stop, 4 timeout, 5 undercurrent (open circuit), 6 safety overcurrent, 7 aborted (the motor could not be started, or `sstop`) |
| lastPeak | largest filtered motor current of the move in 0.1 mA |
| lastMs | duration of the move in ms (0 when it did not start) |

The last move is any motor move: a normal position change, a failsafe or
reference move, each of the three strokes of a calibration, or a service
move. The counters start at 0 after a reset of the STM.

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
command is pending) or the STM is in safe mode, 3 a calibration of the valve
is pending (status 8, calibration bit set, a `staln`, time or movement
trigger, automatic retry or early-stop calibration not started yet, or a
valve without a valid calibration; this includes a valve found at start-up
until its first calibration): the calibration would start right after the
move and undo it. A client must accept -1 as idx. The move starts within
about 1 s; its result is reported by `gvlvx` (lastStop 1 = all pulses moved,
2 = threshold reached) and `gprof`. `sstop` ends it.

After the move the believed position follows the counted pulses (converted
with the learned scaler), and the STM leaves the valve there (service hold,
`gvlvy` flag `svcHold`) until the next `stgtp`, `staop` or `staln` for it, a
calibration of it, or for about 5.5 minutes, then it returns to its drive
target. A failed, blocked, open-circuit or pending-calibration status is kept
(only a calibration clears it); an idle valve becomes open circuit after an
undercurrent (and needs a full calibration, flag `recal`), and failed after a
timeout (fault 1) or an enforced inrush trip (fault 5).

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

## Protocol 3 commands

Answered only by firmware 2.1.0-revamped and later (`gproto 3`). Every request
has at most 2 arguments and at most 16 characters. The commands without
arguments (`glcfg`, `gstax`, `gtlnt`) ignore extra arguments.

| Command | Request | Reply (ok) | Error replies | Effect |
|---|---|---|---|---|
| `slhbt` | `slhbt <0\|1>` | `slhbt <lease> <remainS>` (`slhbt 0 0` while the timeout is 0) | `slhbt err` (not exactly one argument, value not 0/1) | marks a lease client; 1 renews the lease; renewing an expired lease ends the failsafe (the valves return to their targets) |
| `slcfg` | `slcfg <min>` (0 = off, or 5..1440) | `slcfg ok` | `slcfg err` | sets the lease timeout and stores it; marks a lease client; does not renew, except that a change from 0 to a non-zero value starts a fresh lease |
| `sfspo` | `sfspo <idx\|255> <pct\|255>` (pct 0..100, 255 = hold) | `sfspo <idx> ok` (`sfspo 255 ok` for all) | `sfspo <idx> err 1` (idx valid, pct missing, invalid or extra arguments), `sfspo -1 err 1` (idx missing or invalid) | failsafe position of one or all valves; effective at once; stored |
| `glcfg` | `glcfg` | `glcfg <timeoutMin> <fs0> ... <fs11>` (13 values) | – | marks a lease client |
| `gvlvy` | `gvlvy <idx>` | 25 values, below | none for an invalid idx (as `gvlvx`) | read; values 1..19 equal `gvlvx` of the same instant |
| `gstax` | `gstax` | 23 values, below | – | read; values 1..6 equal `gstat` |
| `sstop` | `sstop <idx\|255>` | `sstop <idx> ok` (also when nothing ran) | `sstop -1 err 1` | stops the running move, calibration or service move of idx (255: whatever runs), cancels the requested calibrations of idx (255: all), leaves the valve where it stopped (service hold) |
| `gtlnt` | `gtlnt` | `gtlnt <seconds>` (stored learn time; 0 = time trigger off) | – | read |
| `ssafe` | `ssafe 0` | `ssafe ok` | `ssafe err` (not exactly one argument, or value != 0) | leaves safe mode and clears the watchdog reset window |

    > gvlvy 3
    < gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0
    > gstax
    < gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0
    > sfspo 12 50
    < sfspo -1 err 1

(Valve 3: blocked (9) at its failsafe position 50 %, flags 66 = fsBlocked +
retry, fault 4, first automatic retry in 3540 s, none done yet.)

### Lease and failsafe

A lease client is an ESP that sent `slhbt`, `slcfg`, `sfspo` or `glcfg` within
the last 300 s. The lease expires when it was not renewed for the lease
timeout; then every valve whose failsafe position is not 255 is driven to it
(`gstax` lease 2, `gvlvy` flag `fsLease`). While no lease client is present,
`gvlvd` and `gvlvx` requests renew the lease, so a legacy or 2.0.0 ESP that
polls keeps it alive and a dead ESP lets it expire. A stored timeout of 0
switches the lease off.

The stored target is never changed by the failsafe. The valve is driven to
its **drive target** (`gvlvy` field 23):
1. failsafe position 255 (hold) -> the target;
2. status 9 (blocked) -> the failsafe position;
3. status 4 (failed) or 6 (open circuit) -> the target (the valve is not driven);
4. lease expired and no assembly hold -> the failsafe position;
5. otherwise the target.

Defaults: lease timeout 60 min, failsafe position 50 % (also for an STM that
starts for the first time after 1.x or 2.0.0).

### `gvlvy` fields

Fields 1..19 = `gvlvx`. Then:

| # | field | meaning |
|---|---|---|
| 20 | flags | bit field, below |
| 21 | fault | 0 none, 1 move timeout, 2 stroke timeout (calibration), 3 short circuit (presence test), 4 strokes too short (calibration blocked), 5 inrush trip; cleared by a successful calibration and by a presence test that finds the valve present or absent |
| 22 | fsPct | failsafe position 0..100, 255 = hold |
| 23 | drive | position the valve is driven to (see above) |
| 24 | retryS | seconds to the next automatic retry, 0 = none |
| 25 | retries | automatic retries since the fault began, 0..255 |

| Bit | Value | Name | Set when |
|---|---|---|---|
| 0 | 1 | fsLease | drive = failsafe because the lease expired |
| 1 | 2 | fsBlocked | drive = failsafe because the valve is blocked |
| 2 | 4 | uncalibrated | no valid calibration counts |
| 3 | 8 | needsRef | position not referenced; the next move goes to an end stop first |
| 4 | 16 | recal | full calibration required once the valve is present |
| 5 | 32 | calRestored | counts restored from the EEPROM, no calibration since start-up |
| 6 | 64 | retry | automatic retry scheduled |
| 7 | 128 | earlyPending | one early partial stop at the current drive target |
| 8 | 256 | assembly | `staop` hold |
| 9 | 512 | svcHold | left at a service move or `sstop` position |

Bits 10..15 are 0.

Automatic retries: a blocked (9) or failed (4) valve is calibrated again
(after a short: tested again) after 1 h, 6 h, then every 24 h. A blocked or
jammed valve is never moved or calibrated in a loop: after a move of a
faulted valve ends at an end stop, no further move in that direction is
made until its drive target changes (after an early stop exactly one retry).

### `gstax` fields

| # | field | meaning |
|---|---|---|
| 1..6 | as `gstat` | |
| 7 | lease | 0 off, 1 running, 2 expired (failsafe active) |
| 8 | leaseRemainS | seconds left while running, else 0 |
| 9 | leaseClient | 1 while a lease command arrived within the last 300 s |
| 10 | leaseTimeoutMin | 0, 5..1440 |
| 11 | failsafeMask | bit v = valve v is at its failsafe position because of the lease |
| 12 | safeMode | 0/1 |
| 13 | wdgResets | watchdog resets in the current window |
| 14..17 | uartOre, uartFe, uartNe, rxDropped | UART overrun, framing, noise errors and dropped bytes since start-up |
| 18 | cfgFlags | EEPROM repairs at start-up, below |
| 19 | cfgEvents | EEPROM loads since start-up that repaired or defaulted a checked block |
| 20 | eepWrites | successful EEPROM write steps since start-up |
| 21 | tempAgeS | seconds since the last complete temperature cycle |
| 22 | owScanAgeS | seconds since the last 1-Wire enumeration |
| 23 | sysFlags | bit 0: short and inrush limits suspended (see "Short and inrush protection") |

cfgFlags: bit 0 layout CRC mismatch, 1 shadow block missing, 2 settings block
corrupt, 3 safety block corrupt, 4 a sensor slot was invalid, 5 a calibration
block was invalid, 6 layout not verified (first start after 1.x), 7 EEPROM
read failed.

### Safe mode

3 watchdog resets within 10 min of uptime put the STM into safe mode: no valve
moves (`svmov` answers err 2), `gstax` safeMode 1. It ends with `ssafe 0`, a
power-on, or after 30 min of uptime.

### Short and inrush protection

The presence test checks for a short circuit (filtered current above 200 mA
for 3 samples, fault 3), and every stroke checks the raw current during the
inrush window (above 250 mA for 20 ms, fault 5). Both limits are tuning values
that still need a measurement on the hardware, so by default they only
**report**: the fault is set and counted, the valve keeps its status and the
move or test goes on. The switch `kProtectEnforce`
(`lib/core/include/vdm/protection_guard.h`) enables the enforcement: then a
short or an inrush trip ends with status 4 and an inrush-tripped calibration
stroke never counts as a calibration result. In both settings trips on 3
different valves within 600 s suspend both checks until the next STM start
(`gstax` sysFlags bit 0).

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
- `stgtp idx pos`: the target is always stored, also while a calibration of
  the valve is requested or running (the calibration ends at the newest
  target; 1.x acknowledged such a target but dropped it). The valve is driven
  to its drive target (lease failsafe, blocked valve). A `stgtp` ends the
  assembly hold and the service hold of the valve, and every accepted
  `stgtp` of a valve without a referenced position or without a valid
  calibration starts its reference move or calibration, also when the target
  did not change.
- `staop idx` / `staop 255`: sets the assembly hold (the lease failsafe skips
  the valve until its next `stgtp`); a pending first calibration survives and
  runs at the next target change. A failed (4) or blocked (9) valve only gets
  the target 100 (counted in `cmdRejected`) and is not moved until a
  calibration (1.x drove it to the end stop and reported idle without a
  calibration).
- `stdet 255`: valves found present calibrate fully, also when a calibration is
  stored in the EEPROM. `stdet x` with x != 255 answers `stdet err` (1.x
  answered `stdet ` and did nothing).
- `stlnt n`: stored in the EEPROM and readable with `gtlnt`; the countdown uses
  real elapsed seconds (1.x ran about 10 % late). While the stored value is 0
  and no lease client was seen for 24 h, the time trigger runs every 604800 s
  anyway, so an STM whose ESP was rolled back to a firmware without its own
  schedule keeps calibrating.
- `stlnm n`: 0 switches the movement trigger off, 50..65534 are taken as
  they are, 1..49 are stored as 50 and larger values as 65534 (`gtlnm` shows
  the value in use). The same range is loaded at start-up, so the value
  survives a restart; 1.x took any value but loaded only 50..65534 and fell
  back to 2000 otherwise (also for 0). `stlnm` still resets the movement
  counters of all valves.
- `stlnm`, `stvls`, `stsnx`, `stsny`, `smotc`, `scalx`, `stlnt`, `slcfg`,
  `sfspo`: the EEPROM is written only when a stored value changes, 3 s after
  the last change and at most 30 s after the first unsaved one.
- `eepst`: `eepst 1` only when the configuration is stored; `eepst 0` while a
  write is pending and also while writing fails or is disabled after a failed
  read (1.x replied 1 in these cases). `gstat` eepState tells the cause.
- `staln idx` / `staln 255` and the movement trigger: the calibration starts
  as soon as the valve state machine is idle; 1.x waited until any target had
  changed since the start. Targets have priority over waiting calibrations.
- Requests that change the status or position of a valve (`staln`, `staop`,
  `stdet 255`, time and movement trigger) take effect when no valve moves,
  so the end of a running move no longer overwrites them. 1.x lost them:
  the time trigger then waited another learning time, a movement trigger or
  `staln` left the valve ignoring `stgtp` until then, and `stdet 255`
  during a move left that valve untested at a wrong position. `gvlvd` shows
  the new status up to about 0.2 s after the request, or after the running
  move. A time trigger that fires while the valve is calibrating is
  satisfied by that calibration.
- `gvlvd`, `gvlst`: a valve whose calibration failed keeps status 9 (blocked).
  1.x continued with the counts of the failed pass and reported idle.
- `gvlvd`, `gvlvx` field moves: reset by every successful calibration of the
  valve; the movement trigger counts from there.
- Failed (4) and blocked (9) valves: the STM no longer sets `actual = target`
  without moving the motor. A blocked valve is driven to its failsafe position
  (with the counts of its last successful calibration, or the default scaler)
  and keeps status 9; with failsafe position 255 it stays where it is (0 %
  after the failed calibration). The target the valve was last driven to is
  not counted, nor is a target equal to the position the valve stands at;
  every other target it gets while failed or blocked is counted once in
  `cmdRejected` (also one that arrived during the failing move or
  calibration, and the old target again after the valve was set to its
  current position). A normal move that ends with a timeout leaves the
  position unchanged (1.x reported the target).
- Open-circuit valves (6) keep their position, are not driven (not even to the
  failsafe), and run a presence test when their stored target changes; a
  valve found present again runs a full calibration.
- After a restart, counts, scaler and mean current come back from the EEPROM.
  After a warm reset (reset pin, software, watchdog) also position, target and
  status: no presence test, no motor movement. After a power-on a valve with a
  stored calibration makes one reference move to an end stop at its first
  target (or when the lease failsafe applies) instead of a 3-stroke
  calibration.
- `goned` / `gvlvd` temperatures: a failed read is retried once in the same
  cycle; the last good value is reported for 2 more cycles, then -1270;
  85.0 °C is taken only after a reading of at least 75.0 °C; the bus is
  enumerated again every 24 h. During a long series of moves the motors pause
  (at most 3 s, between moves or calibration strokes) for a temperature cycle
  at least every 60 s.

## EEPROM layout

24LC64 (8192 bytes). Little endian; CRC-8/MAXIM and CRC-16/CCITT-FALSE.

| Range | Content | Written by |
|---|---|---|
| 0x0007-0x013B | 1.x layout (309 bytes), byte-identical to 1.x | 1.x, 2.x |
| 0x013C-0x014B | block A "settings", version 3: escalation, learn time, lease timeout, CRC-16 over the 1.x layout, CRC-8 | 2.0.0 (version 2), 2.1 |
| 0x0160-0x017F | block B "safety", version 1: failsafe positions, a copy of the 1.x motor fields, a copy of the lease timeout, CRC-8 | 2.1 |
| 0x0180-0x023F | blocks C0..C11 "calibration", 16 bytes each: open/close counts, mean current, flags, valve index, CRC-8 | 2.1 |

1.x ignores everything behind its layout, so a downgrade keeps working and an
upgrade again finds the blocks B and C. At start-up each block is checked on
its own: a corrupt block A is rebuilt, the 1.x motor fields are taken from the
block B copy when the layout CRC does not match, and sensor slots are checked
one by one by their ROM CRC. Every repair sets a bit in `gstax` cfgFlags.

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
  repetitions the valve is blocked (9), driven to its failsafe position and
  calibrated again by `staln`, the time or movement trigger, or the automatic
  retry.
- The 60 mA safety limit trips after more than 10 consecutive 1 ms samples
  above it (1.x counted samples over the whole move, so short spikes in a long
  move added up to a false end stop). The 100 mA hard limit trips at the
  first sample above it.
- Every limit (end-stop thresholds, 60 mA safety limit, 100 mA hard limit)
  compares the filtered current (first-order filter, alpha 0.02). The filter
  is held at 0 during the first 250 ms after a motor start (inrush), so these
  limits do not trip then. The inrush check (see "Short and inrush
  protection") watches that window on the raw current.
- A move to an end stop (0 %/100 % target) that starts at least 50 % of the
  stroke away from that end and stops at a threshold or the safety limit
  after less than half of the travel expected from the learned stroke is an
  early end stop. A partial move that ends at an end stop takes its position
  from the counted pulses (1.x set 0 or 100 %); when it stopped before 80 % of
  the requested pulses it is an early end stop too. An early end stop gives
  `lastStop` 3 (or 6), `earlyStops` + 1 and calState bit 2; after an early
  partial stop exactly one more move in the same direction is made for the
  same drive target (flag `earlyPending`), the second early stop in a row
  requests a calibration. Moves before the first successful calibration are
  not checked.

## Firmware update handshake

The ESP flasher (and the legacy ESP's STM update page) starts the ROM
bootloader through the running application: after an NRST pulse, the STM's
boot window listens at 115200 8E1 for `DEADBEEF`, answers `BEEFIT` and jumps to
the bootloader. The STM reads that window in **fixed 8-byte blocks** and
compares each block with `DEADBEEF`; it does not skip CR, LF or any other byte
and does not resynchronise. A stray byte at the reset therefore shifts every
later block. The ESP 2.1 sends `DEADBEEF\n` (9 bytes) every 100 ms: the 9-byte
period brings one block into alignment within 8 sends.

## Known limitations

- **Motor coast (TODO).** The pulse counter is detached when the motor enable
  (ENA) is switched off, and the L293 lets the motor coast with EN = 0. A
  partial move therefore ends a few pulses further than the STM counts, and
  the error adds up until the next end stop. The number of coast pulses is to
  be measured on the device; if it matters, the pulses are to be counted for
  about 200 ms after ENA off.
- The short-circuit and inrush limits (200 mA filtered, 250 mA raw) are not
  yet confirmed on the hardware and only report by default (see above).
