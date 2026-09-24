# VdMot Revamped: build, install, upgrade, recovery

> Unofficial firmware (fork of VdMot_Controller). Version **2.0.0-revamped**.
> Read this page completely before flashing.

## 1. Which files

A GitHub release (tag `v2.0.0-revamped`) contains:

| File | For |
|---|---|
| `VdMot-Revamped_2.0.0-revamped_ESP32-WT32-ETH01.bin` | ESP32 (WT32-ETH01), application image. **Use this one.** |
| `VdMot-Revamped_2.0.0-revamped_ESP32-WT32-ETH01_nodigest.bin` | the same image without the appended SHA-256 digest, for tools that need it (produced as in the hc-version CI) |
| `VdMot-Revamped_2.0.0-revamped_STM32F411_C2.bin` | STM32 BlackPill **F411**, controller hardware **C2** (also C3/C4) |
| `VdMot-Revamped_2.0.0-revamped_STM32F411_C1.bin` | F411, hardware C1 |
| `VdMot-Revamped_2.0.0-revamped_STM32F401_C2.bin` | BlackPill **F401**, hardware C2 (also C3/C4) |
| `VdMot-Revamped_2.0.0-revamped_STM32F401_C1.bin` | F401, hardware C1 |
| `SHA256SUMS`, `manifest.json` | checksums and asset list |

Check the download: `sha256sum -c SHA256SUMS --ignore-missing`.

Choosing the STM image:
- The hardware revision (C1/C2) cannot be detected by the firmware. C3 and C4
  boards use the **C2** image.
- The chip is shown by the dashboard (Maintenance → STM32 → Chip: `0x431` = F411, `0x423`/`0x433` = F401). The dashboard flasher
  refuses an F411 image on an F401 chip (`image_chip_mismatch`). An F401 image
  also runs on an F411 chip, but use the F411 image there.

## 2. Build locally (optional)

Toolchain as in CI: PlatformIO core 6.1.19 (platforms and libraries are pinned
in the `platformio.ini` files).

```sh
# STM32 (envs: STM32_release_C1, STM32_release_C2, STM32F411_release_C1, STM32F411_release_C2)
cd software_stm32
pio run -e STM32F411_release_C2          # -> .pio/build/STM32F411_release_C2/firmware.bin

# ESP32 (env wt32-eth01_revamped; wt32-eth01_revamped_dev = debug build, version 2.0.0-revamped-dev)
cd software_esp32_revamped
pio run -e wt32-eth01_revamped           # -> .pio/build/wt32-eth01_revamped/firmware.bin

# Native unit tests (host compiler + CMake, ASan/UBSan)
cmake -S software_stm32/test/native -B build/native-stm32 && cmake --build build/native-stm32 -j 4 \
  && ctest --test-dir build/native-stm32 --output-on-failure
cmake -S software_esp32_revamped/test/native -B build/native-esp32 && cmake --build build/native-esp32 -j 4 \
  && ctest --test-dir build/native-esp32 --output-on-failure

# Package local builds like a release
python3 tools/release/package.py --tag v2.0.0-revamped --from-builds . --out dist
```

The ESP build generates `src/generated/web_assets.h` (gzipped dashboard) on
every build. The digest-less ESP image is made with
`esptool.py --chip esp32 elf2image --flash_mode dio --flash_freq 80m --flash_size 4MB --dont-append-digest -o firmware_nodigest.bin firmware.elf`.
`package.py` needs all six images; it fails when one is missing.

## 3. Upgrade from the legacy firmware

The partition table is **unchanged** (app0/app1 1280 KB each, LittleFS
1472 KB), so the ESP is upgraded with a normal OTA from the legacy web UI; no
USB cable is needed.

Recommended order: **ESP first, then STM.**
- The new ESP works with the old STM 1.4.x (protocol v1), so the system keeps
  running between the two steps.
- The new ESP's STM flasher validates the image, checks the chip ID and
  verifies every block by read-back.
- The other order also works in principle (the old ESP's STM update page can
  flash the revamped STM, and the old ESP speaks protocol v1), but it is not
  the tested path.

### Before you start
1. Note or screenshot your legacy settings (network, MQTT, valve names, sensor
   assignment, motor parameters). The import is automatic, but a record
   helps if something is not taken over.
2. Make sure the device is reachable over Ethernet (or the configured WiFi).
3. Expect every valve to recalibrate at least once (see the note below).

### Step 1: ESP
1. Open the legacy web UI → firmware update (`http://<device>/update`) and
   upload `VdMot-Revamped_2.0.0-revamped_ESP32-WT32-ETH01.bin`.
2. The ESP restarts into the new firmware. On the first boot it imports the
   legacy settings once (event `config_imported` with the number of imported
   and rejected keys) and starts with the same IP settings.
3. Open `http://<device>/`. The header shows ESP `2.0.0-revamped` and the STM
   version with `proto 1` (old STM).
4. Leave it running for at least 2 minutes with the network and the STM link
   up (or 10 minutes with the network up): only then is the new image marked
   valid (event `app_marked_valid`). If it is not healthy within 15 minutes,
   the bootloader goes back to the legacy firmware (see section 5).

### Step 2: STM
1. Dashboard → Maintenance → STM firmware: upload the STM image (max
   512 KiB). The table shows its size, CRC32 and version.
2. Press Flash, mode **normal**. Progress runs through validating, erasing,
   writing, verifying and waiting for the application (about a minute). The
   valves do not move while flashing.
3. Afterwards the header shows STM `2.0.0-revamped…` with `proto 2`, and the
   valve cards show the extended data (last move, early stops, rejected
   commands, profile). The ESP re-sends all targets. The image is kept as
   `last_good.bin`.

Old STM images (1.4.x) can be flashed the same way.

### Note on STM resets and recalibration
The new ESP firmware never resets the STM on purpose when the ESP boots.
With jumper X20 fitted, however, the ESP pin IO15 has a pull-up during the
ESP's own reset, which most likely holds the STM in reset while the ESP
boots (hardware, not firmware). Expect an ESP restart, including an OTA
update, to restart the STM too, and the valves to recalibrate afterwards.
The ESP detects the STM restart and re-sends the targets. After STM
flashing, every valve recalibrates as well.

## 4. After the upgrade: checklist

- [ ] Header: net, STM and MQTT chips green; time correct (NTP).
- [ ] Settings: station name, network, MQTT, valve names/active flags, temperature slots imported.
- [ ] Valves: each active valve shows `idle` after calibration; setting a target
      moves it and the card shows target source `web` or `mqtt` and sync `synced`.
- [ ] Sensors: all 1-Wire sensors on the bus with plausible values; valve temperatures assigned.
- [ ] MQTT: `<station>/status` = `online`; the legacy topics update; a target sent to
      `<station>/valves/<name>/target/set` moves the valve (see [MQTT.md](MQTT.md)).
- [ ] Home Assistant: existing entities still work; climate/window/control entities of the
      legacy firmware are gone; new diagnostic entities appear.
- [ ] Events: `app_marked_valid` logged a few minutes after the update.
- [ ] Settings → calibration schedule (weekday mask, hour, minute) as wanted.
- [ ] Optional: set a web user and password (Settings) to protect changes.
- [ ] Maintenance → Export config, keep the file.

## 5. Rollback and downgrade

**Automatic rollback (ESP).** A freshly installed ESP image runs in
"pending verify" state. It is marked valid after 120 s with network and STM
link up without interruption, or after 10 min of uptime with the network up.
Otherwise, after 15 min, it marks itself invalid and the bootloader starts the
previous image. A manual restart (Restart button, network settings, factory
reset) with the network up marks the image valid first. This needs a
bootloader with rollback support: the Arduino-ESP32 2.x bootloader of the
legacy builds has it enabled; an older bootloader flashed long ago by USB may
not, and then no automatic rollback happens.

**Downgrade ESP to legacy.** Dashboard → Maintenance → ESP firmware update:
upload the legacy `ESP32_firmware.bin` (any valid ESP32 application image is
accepted). The legacy firmware finds its own settings unchanged, because the
new firmware only reads the legacy NVS namespaces. Changes made in the new
firmware are **not** copied back. The legacy firmware re-creates its Home
Assistant entities on its next discovery run. Upgrading again later does not
re-import (the new firmware keeps its own settings in NVS namespace `vdmrev`).

**Downgrade STM.** Flash a 1.4.x STM image with either ESP (new dashboard →
Maintenance, or the legacy STM update page). The STM 2.x EEPROM extension block
is ignored by 1.x. The motor factors stay valid (2.x uses the 1.x start-up
range 10..40). A movement trigger set to 0 (off) in 2.x loads as 2000 in 1.x.

The new ESP also runs an old STM; the old ESP also runs the new STM. Any mix
works, with the reduced feature set of the older side.

## 6. Recovery

| Problem | Remedy |
|---|---|
| New ESP image does not come up | wait 15 min for the automatic rollback; power-cycle if it hangs |
| Wrong network settings, device unreachable | factory reset: hold **GPIO2 low for 1 s** while the ESP boots. Erases only the new firmware's settings (namespace `vdmrev`); legacy settings are not imported again. The device starts with defaults (interface auto, DHCP, station `VdMot`, DHCP/mDNS host name `VdMot`) |
| Forgot the web password | factory reset as above, or `POST /api/system/factory-reset` if you still have access |
| ESP does not boot at all | USB-serial (3.3 V) on the WT32-ETH01 header, IO0 to GND while powering up, then `esptool.py --chip esp32 erase_region 0xe000 0x2000` (otadata, so app0 boots) and `esptool.py --chip esp32 write_flash 0x10000 <image>.bin`. Do not erase NVS if you want to keep the settings |
| STM flash failed / aborted | flash again from Maintenance. If the STM has no working application, use mode **blank** with the BlackPill BOOT0 jumper/button held so it starts in its ROM bootloader, then reset normally |
| STM completely dead to the ESP | flash the BlackPill directly (ST-Link/SWD or USB DFU with BOOT0), using the `.bin` at address `0x08000000` |
| Valve stays `blocked` | Calibrate the valve; if it blocks again, check the head/adapter; consider raising the motor factors or enabling breakaway escalation (Settings → motor); use Service move to free a stuck pin |
| Config broken after an update | the device boots with defaults and logs `config_defaults`; restore with Maintenance → Import config |

Never power off the device while the STM is being flashed (the ESP postpones
its own restarts until the flash is done).
