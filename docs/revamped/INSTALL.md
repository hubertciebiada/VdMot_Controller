# VdMot Revamped: build, install, upgrade, recovery

> Unofficial firmware (fork of VdMot_Controller). Version **2.1.0-revamped**.
> Read this page completely before flashing.

## 1. Which files

A GitHub release (tag `v2.1.0-revamped`) contains:

| File | For |
|---|---|
| `VdMot-Revamped_2.1.0-revamped_ESP32-WT32-ETH01.bin` | ESP32 (WT32-ETH01), application image. **Use this one.** |
| `VdMot-Revamped_2.1.0-revamped_ESP32-WT32-ETH01_nodigest.bin` | the same image without the appended SHA-256 digest, for tools that need it (produced as in the hc-version CI) |
| `VdMot-Revamped_2.1.0-revamped_STM32F411_C2.bin` | STM32 BlackPill **F411**, controller hardware **C2** (also C3/C4) |
| `VdMot-Revamped_2.1.0-revamped_STM32F411_C1.bin` | F411, hardware C1 |
| `VdMot-Revamped_2.1.0-revamped_STM32F401_C2.bin` | BlackPill **F401**, hardware C2 (also C3/C4) |
| `VdMot-Revamped_2.1.0-revamped_STM32F401_C1.bin` | F401, hardware C1 |
| `SHA256SUMS`, `manifest.json` | checksums and asset list |

Check the download: `sha256sum -c SHA256SUMS --ignore-missing`.

Choosing the STM image:
- Hardware revision: C3 and C4 boards use the **C2** image. Every 2.1 image
  carries its revision (`VDM-HW:C1` / `VDM-HW:C2`), and the running STM 2.1
  reports its own in `gvers`. The dashboard flasher compares them and refuses
  an image for the other revision (`board_mismatch`). When the running STM
  does not report a revision (1.x, 2.0.0, or no STM application in blank
  mode), you choose the board in the flash dialog. Images without a marker
  (1.x, 2.0.0) flash with a warning.
- The chip is shown by the dashboard (Maintenance → STM32 → Chip: `0x431` =
  F411, `0x423`/`0x433` = F401). The flasher refuses an F411 image on an F401
  chip (`image_chip_mismatch`). An F401 image also runs on an F411 chip, but
  use the F411 image there.

## 2. Build locally (optional)

Toolchain as in CI: PlatformIO core 6.1.19 (platforms and libraries are pinned
in the `platformio.ini` files).

```sh
# STM32 (envs: STM32_release_C1, STM32_release_C2, STM32F411_release_C1, STM32F411_release_C2)
cd software_stm32
pio run -e STM32F411_release_C2          # -> .pio/build/STM32F411_release_C2/firmware.bin

# ESP32 (env wt32-eth01_revamped; wt32-eth01_revamped_dev = debug build, version 2.1.0-revamped-dev)
cd software_esp32_revamped
pio run -e wt32-eth01_revamped           # -> .pio/build/wt32-eth01_revamped/firmware.bin

# Native unit and glue tests, mutation (Linux container, same as CI; Docker needed)
bash tools/native/docker.sh test stm32
bash tools/native/docker.sh test esp32
bash tools/native/docker.sh mutate esp32 --files lib/core/src/lease_client.cpp

# Package local builds like a release
python3 tools/release/package.py --tag v2.1.0-revamped --from-builds . --out dist
```

The ESP build generates `src/generated/web_assets.h` (gzipped dashboard) on
every build. The digest-less ESP image is made with
`esptool.py --chip esp32 elf2image --flash_mode dio --flash_freq 80m --flash_size 4MB --dont-append-digest -o firmware_nodigest.bin firmware.elf`.
`package.py` needs all six images; it fails when one is missing.

## 3. Upgrade

The partition table is **unchanged** (app0/app1 1280 KB each, LittleFS
1472 KB), so the ESP is upgraded with a normal OTA, from the legacy web UI or
from the 2.0.0 dashboard; no USB cable is needed.

Recommended order: **ESP first, then STM.**
- The new ESP works with every STM from **1.4.0** on (protocol 1, 2 or 3), so
  the system keeps running between the two steps. An STM **below 1.4.0** only
  gets targets from the new ESP; the dashboard shows "STM firmware is older
  than 1.4.0: update the STM" and refuses the other STM actions. Flash the STM
  right after the ESP (step 2), or flash it first with the legacy ESP.
- The new ESP's STM flasher validates the image, checks chip and board
  revision and verifies every block by read-back.
- The other order also works (the old ESP's STM update page can flash the
  revamped STM, and the revamped STM keeps the v1 protocol), but it is not the
  tested path.

### Before you start
1. Note or screenshot your legacy settings (network, MQTT, valve names, sensor
   assignment, motor parameters). The import is automatic, but a record
   helps if something is not taken over.
2. Make sure the device is reachable over Ethernet (or the configured WiFi).
3. Expect every valve to calibrate once after the STM update (see step 2).
4. MQTT: the client id changes to `<host>-<mac6>` (MQTT.md): update broker
   ACLs keyed on the old id first.
5. Scripts and `rest_command`s that call the HTTP API need the header
   `X-VdMot: 1` (API.md).

### Step 1: ESP
1. Open the legacy web UI → firmware update (`http://<device>/update`), or on
   2.0.0 Maintenance → ESP firmware, and upload
   `VdMot-Revamped_2.1.0-revamped_ESP32-WT32-ETH01.bin`.
2. The ESP restarts into the new firmware. On the first boot after the legacy
   firmware it imports the legacy settings once (event `config_imported` with
   the number of imported and rejected keys; details under Maintenance →
   import report) and starts with the same IP settings. A 2.0.0 installation
   keeps its settings.
3. Open `http://<device>/`. The header shows ESP `2.1.0-revamped` and the STM
   version with its protocol.
4. Leave it running. The new image is marked valid (event `app_marked_valid`)
   after 2 minutes in which all of these held without interruption:
   - the network is up and proven end to end (a gateway ping reply, the MQTT
     session, an SNTP sync, a request from the LAN, or a DHCP lease);
   - the ESP's own HTTP self-check (`GET /api/health`) answers;
   - the STM link is up, if it was up when the image was uploaded.

   If that does not happen within 15 minutes, the bootloader goes back to the
   previous firmware (section 5). The new image gets **one** boot: any reset
   before it is marked valid, except a restart you request in the dashboard
   while the checks pass, counts as a failed boot. A second ESP update is
   refused while the running image still waits (`GET /api/health` → `ota`
   shows the checks and the time left).

### Step 2: STM
1. Dashboard → Maintenance → STM firmware: upload the STM image (max
   512 KiB). The table shows its size, CRC32, version, board revision and the
   validation result.
2. Press Flash, mode **normal**. Progress runs through validating, erasing,
   writing, verifying and waiting for the application (about a minute; the
   ESP waits up to 60 s for the new application). The valves do not move while
   flashing. If the STM does not answer at 115200 baud, one more session runs
   at 57600 baud.
3. Afterwards the header shows STM `2.1.0-revamped_C2` (or `_C1`) with
   `proto 3`, and the valve cards show the extended data (last move, early
   stops, rejected commands, flags, failsafe). The image is kept as
   `last_good.bin`.
4. On its first start after 1.x or 2.0.0 the STM takes over the motor
   settings, sensor assignment and escalation from the EEPROM; it has no
   calibration records yet, so every valve calibrates once. From then on the
   calibrations are stored, and restarts no longer recalibrate (section 3.1).

Old STM images (1.4.x, 2.0.0) can be flashed the same way; they carry no
revision marker, so the flasher only warns.

### 3.1 Restarts and stored targets

- **STM 2.1** keeps the calibration of every valve in its EEPROM. After a warm
  reset (reset pin, e.g. an ESP restart with jumper X20; software; watchdog)
  it also keeps position, target and status: nothing moves. After a power-on
  every calibrated valve makes one reference move to an end stop at its first
  target (or when the failsafe applies), not a full calibration.
- **The ESP** keeps the desired targets in RTC memory (survives software
  restarts) and in NVS: written 5 min after the last change, at most 30 min
  after the first unsaved change, and before every planned restart. After a
  power loss the targets are therefore at most 30 min old. Event
  `targets_restored`; the valve shows target source `restored` until the STM
  confirmed it. After a detected STM restart the ESP pushes every desired
  target once.
- Every ESP restart and every user STM reset first waits until the STM has
  written its EEPROM (`eepst`, at most 10 s; event `stm_eeprom_wait_timeout`
  when it did not finish).
- Older STMs (1.x, 2.0.0) recalibrate every valve after each restart.

### Note on jumper X20 and STM resets

The ESP can reset the STM only with jumper X20 fitted (NRST through a
transistor on IO15). IO15 is an ESP strapping pin with a pull-up during the
ESP's own reset, so with X20 fitted every ESP restart (OTA, network settings,
watchdog) also resets the STM. The new ESP firmware never resets the STM on
purpose when the ESP boots; it resets it only on user request, for flashing,
or after a sustained link failure (>= 5 timeouts over >= 60 s, at most once
per 10 min). With STM 2.1 such a reset keeps positions and targets (section
3.1); with an older STM every valve recalibrates.

### 3.2 Failsafe

The failsafe is **on** after the update: default **60 min**, failsafe position
**50 %** for every valve.

- The ESP renews a lease on the STM while its **regulator** is alive:
  - MQTT off: always alive (the failsafe never triggers; targets come from the
    dashboard or scripts);
  - mode MQTT: alive while the broker connection is up;
  - mode MQTT + HA: alive while the broker connection is up and Home
    Assistant's status is not `offline` (enable retained birth and last will
    in HA, MQTT.md). An accepted MQTT command also counts as alive.
- Without renewal for the timeout, every active valve goes to its failsafe
  position (event `failsafe_active`, MQTT `failsafe` = 1, dashboard notice).
  When the regulator is back (alive for 2 min, or at once with an MQTT
  command), the valves return to their targets (`failsafe_ended`).
- **STM 2.1** runs the lease itself: the failsafe also works when the ESP is
  dead or rolled back. With an STM 2.0.0 or 1.x the ESP emulates it (the ESP
  sends the failsafe positions as targets), so it works only while the ESP
  runs; the emulation survives ESP restarts.
- Settings → Failsafe: timeout 0 (off) or 5..1440 min; per valve 0..100 % or
  "Hold" (the valve stays where it is). Inactive valves are held.
- **Manual override during a failsafe:** targets set in the dashboard or by
  HTTP are stored and applied once the regulator is back. To open a valve at
  once use **Assembly** (fully open, target 100 until the next target, excluded
  from the failsafe) or change its failsafe position.
- A blocked valve (failed calibration) also goes to its failsafe position and
  is calibrated again automatically after 1 h, 6 h, then every 24 h.
- The legacy firmware's MQTT timeout for PI valves (`brokerMQTO`/`brokerMQToPos`)
  is not imported; the import report shows its values.

### 3.3 Network settings on trial

A change of network settings that are in use (interface, DHCP, static IP,
mask, gateway, DNS; WiFi fields on WiFi) restarts the ESP and runs on trial:
the dashboard shows "Keep these settings" / "Revert now" with a countdown.
Without "Keep" within 2 min after the network came up (2 min after the boot
when it does not come up) the ESP goes back to the previous settings and
restarts (event `net_trial_reverted`). A restart during the trial reverts at
once. A static IP without DNS uses the gateway as DNS server.

## 4. After the upgrade: checklist

- [ ] Header: net, STM and MQTT chips green; time correct (NTP).
- [ ] Settings: station name, network, MQTT, valve names/active flags, temperature slots imported;
      Maintenance → import report read and dismissed.
- [ ] Settings → Failsafe: timeout and positions as wanted (0 = off).
- [ ] Valves: each active valve shows `idle` after calibration; setting a target
      moves it and the card shows target source `web` or `mqtt` and sync `synced`.
- [ ] Sensors: all 1-Wire sensors on the bus with plausible values; valve temperatures assigned.
- [ ] MQTT: `<main>status` = `online`; the legacy topics update; a target sent to
      `<main>valves/<name>/target/set` moves the valve (see [MQTT.md](MQTT.md)).
- [ ] MQTT broker: ACLs for the new client id; persistent-session limits (MQTT.md).
- [ ] Home Assistant: existing entities still work; climate/window/control entities of the
      legacy firmware are gone; new entities appear; birth message and last will retained.
- [ ] `rest_command`s and scripts send `X-VdMot: 1` (API.md).
- [ ] Events: `app_marked_valid` logged a few minutes after the update.
- [ ] Settings → calibration schedule (weekday mask, hour, minute) as wanted. With a
      schedule the STM's own time trigger is switched off.
- [ ] Optional: set a web user and password (Settings) to protect changes.
- [ ] Maintenance → Export config, keep the file.

## 5. Rollback and downgrade

**Automatic rollback (ESP).** A freshly installed ESP image runs in
"pending verify" state and is marked valid by the checks of step 1.4.
Otherwise, after 15 min, it marks itself invalid and the bootloader starts the
previous image. This needs a bootloader with rollback support: the
Arduino-ESP32 2.x bootloader of the legacy builds has it enabled; an older
bootloader flashed long ago by USB may not, and then no automatic rollback
happens.

**Downgrade ESP to legacy.** Dashboard → Maintenance → ESP firmware update:
upload the legacy `ESP32_firmware.bin` (any valid ESP32 application image is
accepted). The legacy firmware finds its own settings unchanged, because the
new firmware only reads the legacy NVS namespaces. Changes made in the new
firmware are **not** copied back. After the rollback:
1. Home Assistant: the kept entities keep working (they have no availability
   topic). The new entities show "unavailable": remove them with the legacy UI
   → HA discovery → **delete**, then **send** (the new firmware keeps
   `/HADiscovery.cfg` as the list of everything it published).
2. MQTT: the legacy firmware connects with the client id `VdMot` (the station
   name) again; adjust broker ACLs if needed.
3. STM 2.1 stays: the legacy ESP's `gvlvd` polls renew its lease, and when the
   ESP dies the valves go to their failsafe positions after 60 min. If the new
   ESP had switched the STM's own calibration timer off (because it had a
   schedule), the STM switches it on again (every 7 days) 24 h after the last
   ESP 2.1 command.

Upgrading again later does not re-import (the new firmware keeps its own
settings in NVS namespace `vdmrev`). A downgrade to ESP 2.0.0 keeps the
settings 2.0.0 knows; the 2.1 settings are kept for the next upgrade.

**Downgrade STM.** Flash a 1.4.x or 2.0.0 STM image with either ESP (new
dashboard → Maintenance, or the legacy STM update page). The 1.x layout in the
EEPROM stays byte-compatible; 1.x ignores the 2.x blocks, 2.0.0 ignores the
2.1 blocks. The motor factors stay valid (2.x uses the 1.x start-up range
10..40). A movement trigger set to 0 (off) in 2.x loads as 2000 in 1.x. With
the new ESP the failsafe is then emulated by the ESP. Upgrading the STM to 2.1
again finds the stored calibrations; motor settings changed under 1.x in the
meantime are replaced by the values last written by 2.1.

**Combinations:**

| ESP | STM | Works | Notes |
|---|---|---|---|
| 2.1 | 2.1 | fully | protocol 3, failsafe on the STM |
| 2.1 | 2.0.0 or 1.4.x | yes | protocol 2 / 1, failsafe emulated by the ESP, valves recalibrate after STM restarts |
| 2.1 | below 1.4.0 | targets only | update the STM |
| legacy or 2.0.0 | 2.1 | yes | protocol 1 / 2; STM lease renewed by the ESP's polls; STM failsafe 50 % after 60 min when the ESP dies |

## 6. Recovery

| Problem | Remedy |
|---|---|
| New ESP image does not come up | wait 15 min for the automatic rollback; power-cycle if it hangs |
| Wrong network settings, device unreachable | wait: without "Keep" the previous settings come back after about 2 min (at most about 5 min including the restarts). As a last resort use the factory reset |
| Factory reset | fit a jumper from **GPIO2 to GND** and power up; keep it for **5 s** while the ESP boots, then remove it. Erases only the new firmware's settings (namespace `vdmrev`); legacy settings are not imported again. The device starts with defaults (interface auto, DHCP, station `VdMot`, host name `VdMot`). The reset happens once per fitting: while the jumper stays fitted, later boots keep the settings (event `factory_reset_skipped`) |
| Forgot the web password | factory reset as above, or `POST /api/system/factory-reset` if you still have access |
| ESP does not boot at all | USB-serial (3.3 V) on the WT32-ETH01 header, IO0 to GND while powering up, then `esptool.py --chip esp32 erase_region 0xe000 0x2000` (otadata, so app0 boots) and `esptool.py --chip esp32 write_flash 0x10000 <image>.bin`. Do not erase NVS if you want to keep the settings |
| Config broken after an update | the device restores the last saved config from its backup (`config_restored`) or repairs the broken fields (`config_repaired`); only when both fail it boots with defaults (`config_defaults`). Restore with Maintenance → Import config |
| Network unreachable while the link is up | the ESP restarts its network interface after `net.reconnectTimeoutMin` (default 5 min) without proof of the network, and itself after another 5 min (longer on every further restart in the same outage) |
| STM flash failed / aborted | flash again from Maintenance. If the STM has no working application, use mode **blank** with the BlackPill BOOT0 jumper/button held so it starts in its ROM bootloader, choose the board; after "Flashed and verified" remove BOOT0 and press Reset STM |
| STM completely dead to the ESP | flash the BlackPill directly (ST-Link/SWD or USB DFU with BOOT0), using the `.bin` at address `0x08000000` |
| STM in safe mode (3 watchdog resets within 10 min; no valve moves) | dashboard "Leave safe mode" (or HA button), power-cycle, or wait 30 min; check the event log for the cause |
| Valve stays `blocked` | the valve sits at its failsafe position and is retried automatically. A stroke shorter than **minCounts** (Settings → motor, default 3000 pulses) blocks it: the warning "stroke close to minCounts" (`calib_stroke_short`) shows valves near that limit; lower minCounts for short-stroke heads. Otherwise check the head/adapter, raise the motor factors or enable breakaway escalation; use Service move to free a stuck pin |

Never power off the device while the STM is being flashed (the ESP postpones
its own restarts until the flash is done).

## 7. Syslog

Settings → Syslog: server IPv4, port (514), level 0 off, 1 warning and worse,
2 info and worse, 3 everything (debug). Messages are RFC 5424 over UDP:
`<pri>1 <UTC time> <host> vdmot - <event name> - <message>`, facility
**local0**, app name **vdmot**, host = the DHCP host name of the station, msgid
= the event code name.

This differs from the legacy firmware (host `VdMot`, app `SysLog`, facility
kern, levels 1..3 = debug detail): server filters on host, app or facility
must be adapted. An imported legacy level 1..3 becomes level 3.

## 8. Known hardware limitations [HW]

Items that still need a measurement on a device:
- **Short and inrush limits** of the STM (200 mA filtered in the presence test,
  250 mA raw during the motor start) are not yet confirmed for every actuator
  type and supply voltage. They are **report-only** by default: a trip is
  shown as fault 3 / 5 in the valve's STM data, the valve keeps working.
  Enforcement is a build switch (PROTOCOL_V2.md).
- **Motor coast pulses** after the motor is switched off are not counted; a
  partial move can end slightly further than reported until the next end
  stop (PROTOCOL_V2.md, "Known limitations").
- The IO15 strap behaviour with jumper X20 (STM held in reset during an ESP
  reset) follows from the schematic and was not measured on every board
  revision.
