# VdMot Revamped in Rust: first flash and rollout

> Unofficial firmware (fork of VdMot_Controller). Version **2.2.0-revamped**, the Rust port of
> VdMot Revamped 2.1.7 for the ESP32 and the STM32 ([README.md](README.md)). Read this page
> completely before flashing.

This page is the procedure from C++ 2.1.7 to the Rust firmware and back. Everything it does not
mention works as in 2.1.7 and is described in the C++ procedure
[docs/revamped/INSTALL.md](../revamped/INSTALL.md): restarts and stored targets, the failsafe,
the network trial, the recovery table, syslog. What differs from 2.1.7 is listed in
[CHANGES.md](CHANGES.md).

## 1. Files

The release `v2.2.0-revamped` contains:

| File | For |
|---|---|
| `VdMot-Revamped_2.2.0-revamped_ESP32-WT32-ETH01.bin` | ESP32 (WT32-ETH01), application image. **Use this one.** |
| `VdMot-Revamped_2.2.0-revamped_ESP32-WT32-ETH01_nodigest.bin` | the same image without the appended SHA-256 digest, for tools that need it |
| `VdMot-Revamped_2.2.0-revamped_STM32F411_C2.bin` | STM32 BlackPill **F411**, controller hardware **C2** (also C3/C4) |
| `VdMot-Revamped_2.2.0-revamped_STM32F411_C1.bin` | F411, hardware C1 |
| `VdMot-Revamped_2.2.0-revamped_STM32F401_C2.bin` | BlackPill **F401**, hardware C2 (also C3/C4) |
| `VdMot-Revamped_2.2.0-revamped_STM32F401_C1.bin` | F401, hardware C1 |
| `SHA256SUMS`, `manifest.json` | checksums and asset list |

Check the download: `sha256sum -c SHA256SUMS --ignore-missing`. The STM image is chosen as for
2.1.7 ([docs/revamped/INSTALL.md §1](../revamped/INSTALL.md#1-which-files)).

Keep the files of the way back at hand, from the release `v2.1.7-revamped`: the ESP image
`VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin` and the STM image of your chip and board.

Give the STM images short names before you upload them, e.g. `stm220.bin` for 2.2.0 and
`stm217.bin` for the C++ 2.1.7 image. The dashboard keeps at most 31 characters of a name (a
longer one becomes its first 27 characters and `.bin`), so the four release names all arrive as
`VdMot-Revamped_2.2.0-revamp.bin`, without chip and board.

## 2. Order

1. ESP32: a bench unit first if you have one (section 3.1), then the controller (3.2 to 3.4).
2. STM32: the bench proof on a spare BlackPill (section 4.1, required), then the controller (4.2).

The ESP comes first: the Rust ESP flasher writes sector 0 of the STM image last (decision D9,
section 4.3), and the Rust ESP runs with the C++ 2.1.7 STM as well as with the Rust one. Do the
STM only after the ESP passed its checklist (section 3.4).

Before you start:

1. Both MCUs run C++ 2.1.7 ([docs/revamped/INSTALL.md](../revamped/INSTALL.md)).
2. Dashboard → Maintenance → Backup → **Export config**, keep the file.
3. A USB-UART adapter (3.3 V) for the one ESP failure the firmware cannot undo (section 3.5), and
   access to the BOOT0 button of the BlackPill for the STM failures (section 4.4).
4. While the new ESP image is on trial (section 3.3), leave the controller alone: no power
   cycle, no firmware upload, no STM flash.

## 3. ESP32

### 3.1 Bench unit (optional)

A spare WT32-ETH01 on a second controller board shows what the emulator cannot: the Ethernet PHY,
WiFi and a boot that loops before `main` ([README.md](README.md#what-is-proven-where)). Flash it
over USB-UART like the controllers ([assembly_kit/installation.md](../../assembly_kit/installation.md):
header X23, jumper X22 for the flash mode), with their bootloader and partition table and the C++
2.1.7 image:

```sh
esptool.py --chip esp32 write_flash 0x1000 software_esp32/bootloader_dio_40m.bin \
  0x8000 software_esp32/partitions.bin 0xe000 software_esp32/boot_app0.bin \
  0x10000 VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin
```

Then run sections 3.2 to 3.4 on it. The bench result stands for a controller only when its
bootloader is the controller's: this bootloader has no rollback support, and the boot guard of
section 3.3 is built for it.

### 3.2 Upload

Dashboard → Maintenance → **ESP firmware update**: upload
`VdMot-Revamped_2.2.0-revamped_ESP32-WT32-ETH01.bin`. The C++ firmware checks the image, writes it
into the other app slot and restarts after 1 s. When the STM link is up at the upload, the trial
of the new image requires it (section 3.3).

After the restart the header shows ESP `2.2.0-revamped`, and the event log shows
`boot (reset sw, count …, fw 2.2.0-revamped)`.

### 3.3 The trial

The controller's bootloader cannot roll an image back, so the Rust firmware does it itself: its
boot guard ([GLUE-DESIGN-ESP.md §6](GLUE-DESIGN-ESP.md#6-boot-guard)) runs the new image on trial,
with the C++ 2.1.7 image that uploaded it as the fallback.

| During the trial | The boot guard |
|---|---|
| 120 s in a row healthy: network up and proven, the HTTP self-check answers, the STM link up (only when it was up at the upload) | confirms the image: event `app_marked_valid`; the trial ends |
| any reset before the confirmation: crash, watchdog, network watchdog, **power cycle** | counts the boot; the **4th** unconfirmed boot switches to the C++ image before anything else runs |
| the start-up hangs for 60 s | restarts; the boot counts |
| 15 min without 120 s of health | restarts (event `reboot_requested`, rollback, with the missing checks) into the C++ image |
| a restart from the dashboard (or a network settings change) while the network, the HTTP self-check of the last 30 s and, when required, the STM link are up | confirms first, then restarts; otherwise the restart counts as a boot (also a restart over MQTT, `cmd/restart`, which proves no web server) |
| an ESP firmware upload | refuses it: `409 upload_failed` "image on trial" (it would overwrite the fallback) |
| `POST /api/system/ota/switch-back` | restarts into the C++ image (also after the confirmation) |
| after the confirmation: 4 boots in a row end with a crash, a watchdog reset or a start-up hang, each within 10 min | switches to the C++ image, which runs as usual; the Rust image stays in the other slot. Report the event log of the Rust image and upload it again once the cause is known |

Watch the trial with `GET /api/health`: `ota` is an object while the image is on trial (the
checks `net`, `http`, `stm`, `healthyForS`, `remainS`) and `null` once it is confirmed:

```sh
curl http://<device>/api/health
```

Switch back to the C++ image by hand (the dashboard has no button for it):

```sh
curl -X POST -H 'X-VdMot: 1' -H 'Content-Type: application/json' \
  -d '{"confirm":"switch-back"}' http://<device>/api/system/ota/switch-back
```

It answers `202 {"result":"restarting"}` and restarts (event `reboot_requested`, switch back);
`409 busy` while an upload or an STM flash runs, `409 restarting` while a restart waits,
`409 no_fallback` when the other slot holds no image.

After an automatic switch the header shows `2.1.7-revamped` again, and the C++ event log has its
own `boot` event (reset `sw`); the reason is not in it. The boot limit also counts power cycles
and the 15 min also run out during a network outage
([GLUE-DESIGN-ESP.md §6.6](GLUE-DESIGN-ESP.md#66-coverage)): in both cases the image may be fine.

### 3.4 First-flash checklist

1. The image is confirmed: event `app_marked_valid`, `ota` is `null` in `GET /api/health`.
2. **Switch back** to the C++ image with the `curl` call of section 3.3. It answers `202` and the
   header shows `2.1.7-revamped` after the restart. This proves that the Rust image takes a
   `POST` with a body and that the way back works.
3. **Upload the Rust image again**, from the C++ dashboard as in section 3.2. It runs a new trial
   (the switch back removed its confirmation) and is confirmed again: wait for
   `app_marked_valid`. This proves the way forward.
4. Let it run for **at least 10 minutes**.
5. Restart it: Dashboard → Maintenance → Restart and reset → **Restart ESP** (or
   `curl -X POST -H 'X-VdMot: 1' http://<device>/api/system/reboot`). It must come back: the
   dashboard answers, the new `boot` event says `reset sw`.
6. **Power-cycle** the controller. It must come back the same way.
7. The checks of the C++ procedure
   ([docs/revamped/INSTALL.md §4](../revamped/INSTALL.md#4-after-the-upgrade-checklist)): header
   chips, settings, valves, sensors, MQTT `<main>status` = `online`, Home Assistant entities,
   targets from the dashboard and from MQTT move the valves.
8. Maintenance → ESP32: note the heap after the boot with Ethernet idle. The acceptance figures
   (free heap, the minimum under load) are in
   [GLUE-DESIGN-ESP.md §2.3](GLUE-DESIGN-ESP.md#23-ram-budget-no-psram-263-kb-of-8-bit-heap);
   `GET /api/health` lists the free stack of every task (`tasks`, `minFree`).
9. Only with WiFi (interface WiFi, or Auto with WiFi settings as the fallback): start WiFi on both
   paths the firmware has and check that the controller stays up. Interface WiFi: the start-up
   builds the station (a restart is enough). Auto: unplug the Ethernet cable for at least a
   minute, the app thread builds the station after 30 s. Then read `minFree` of `app` in
   `GET /api/health`: the stack sizes were measured with WiFi off
   ([REVIEW-ESP-SAFETY.md F6](REVIEW-ESP-SAFETY.md)).

Steps 2 and 3 run the two paths every later update needs: `POST` requests (the switch back, the
upload) and an upload from the C++ firmware. Steps 4 to 6 test the one failure the boot guard
cannot catch: an image that loops in the ESP-IDF start-up before `main`, where the guard runs.
QEMU cannot show it: it cannot run a restart after the first minute of uptime
([GLUE-DESIGN-ESP.md §5.5](GLUE-DESIGN-ESP.md#55-end-to-end-in-qemu-vdm-esp-fw)).

### 3.5 When something fails

| Step | What you see | What to do |
|---|---|---|
| 3.2 upload | an error text in the dashboard | nothing has changed, the C++ firmware runs. `409 busy`: an upload or an STM flash runs, retry later. Otherwise check the file against `SHA256SUMS` and report the text |
| 3.2 to 3.3 | the controller does not answer for a few minutes | wait: the boot limit switches back within a few boots, the start-up deadline after 60 s per boot, the health limit after 15 min |
| 3.3 | back on `2.1.7-revamped` | the trial failed. Note the time, the C++ event log and the header chips, report them. An image that failed its trial gets a new trial when it is uploaded again |
| 3.3 | not confirmed, `ota.checks` shows a check `false` | fix that cause (network, HTTP, STM link); after 15 min the guard switches back |
| 3.4 step 2 | the switch back is not answered `202`, or the controller stays on `2.2.0-revamped` | stop the rollout, report; the image stays confirmed and runs |
| 3.4 step 3 | the upload from the C++ dashboard is refused, or the new trial fails | the C++ image runs; report the text or the C++ event log |
| 3.4 steps 5, 6 | no dashboard and no ping 5 minutes after the restart or the power cycle | the image loops before `main`. USB-UART: write the C++ 2.1.7 image to `0x10000` and erase otadata, as in "ESP does not boot at all" ([docs/revamped/INSTALL.md §6](../revamped/INSTALL.md#6-recovery)). The settings stay |
| any time | the Rust image misbehaves while its HTTP API answers | switch back (section 3.3) and report |
| any time | the image runs but its HTTP API does not answer | power-cycle; if HTTP stays dead, USB-UART as above |
| any time | a restart, the switch back or an upload answers `409 stm_sector0_pending` | an STM flash waits for its sector 0: **do not power-cycle**, see section 4.4 |

### 3.6 Back to C++ 2.1.7

- On trial or confirmed: the switch back of section 3.3 starts the C++ image that uploaded the
  Rust one, as long as no other upload replaced it.
- After the confirmation: Dashboard → Maintenance → ESP firmware update with
  `VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin`.

The C++ firmware reads the settings and files the Rust firmware wrote (same NVS and LittleFS
formats); the desired targets come from NVS, because the RTC records of the two firmwares differ
([CHANGES.md](CHANGES.md)). The switch back and an upload remove the confirmation of the Rust
image they leave: when the C++ firmware installs it again, it runs a new trial
([GLUE-DESIGN-ESP.md §6.1](GLUE-DESIGN-ESP.md#61-records)).

## 4. STM32

The ESP flashes the STM through its boot window (`DEADBEEF` -> `BEEFIT` -> ROM bootloader), so a
flash needs no access to the BlackPill, except in the cases of section 4.3. Flash with mode
**normal**, as in [docs/revamped/INSTALL.md](../revamped/INSTALL.md#step-2-stm); never power off
the controller while the STM is flashed (the valves do not move meanwhile).

### 4.1 Bench proof (required, D10)

Before the first controller ([GLUE-DESIGN-STM.md §5.9](GLUE-DESIGN-STM.md#59-proof-on-hardware)):
a spare BlackPill with the controller's chip (F401CC or F411CE) on a second controller board with
jumper X20 fitted, its ESP on the Rust firmware (the bench unit of section 3.1 serves).

1. Flash C++ 2.1.7 (`stm217.bin`); a blank BlackPill needs mode blank with BOOT0 held
   ([docs/revamped/INSTALL.md §6](../revamped/INSTALL.md#6-recovery)). Set a few targets.
2. Flash Rust (`stm220.bin`).
3. Flash C++ 2.1.7 again.
4. Flash Rust again.
5. Power-cycle: a cold start.
6. A logic analyzer on the 1-Wire line (PB10: reset and presence, write and read slots) and on
   I2C (PB6, PB7: the EEPROM), once with C++ 2.1.7 and once with Rust. The Rust 1-Wire read samples
   earlier after the falling edge than the C++ (risk R5,
   [GLUE-DESIGN-STM.md §8](GLUE-DESIGN-STM.md#risks)); compare the slots with sensors on the bus.

After each flash (steps 1 to 4) check:

- `gvers`: Maintenance → STM32 shows the new firmware (`2.2.0-revamped` or `2.1.7-revamped`),
  the board and protocol `v3`;
- the warm restore: the valves keep their status, position and target; no presence test and no
  calibration starts ([GLUE-DESIGN-STM.md §3.2](GLUE-DESIGN-STM.md#32-warm-state-and-reset-cells-in-no-init-ram)).

After the power cycle (step 5) every calibrated valve makes one reference move to its first
target, as with 2.1.7 ([docs/revamped/INSTALL.md §3.1](../revamped/INSTALL.md#31-restarts-and-stored-targets)).

The progress of a Rust ESP flash runs through erasing, writing and verifying twice: sectors 1 and
up first, then sector 0; the percentage holds while sector 0 is erased and written
([CHANGES.md](CHANGES.md)).

### 4.2 The controller

1. Conditions: the bench proof passed for the controller's chip; the controller's ESP runs the
   Rust firmware and passed its checklist (section 3.4).
2. Upload `stm220.bin` (and keep `stm217.bin` in the image list: the known-good image to go
   back to). Flash it.
3. Check `gvers` and the warm restore as in 4.1, then valves and sensors as in
   [docs/revamped/INSTALL.md §4](../revamped/INSTALL.md#4-after-the-upgrade-checklist).

### 4.3 Risks of the STM flash

- **The first flash needs the HSE (R12).** A C++ 2.1.7 STM whose 25 MHz crystal does not start
  hangs before its boot window, so the ESP cannot flash it and it does not run either until the
  crystal starts. Every Rust image opens its boot window without the crystal as well (decision D1),
  so only the first flash from C++ depends on it.
- **Sector 0 last (D9).** The Rust ESP flasher writes and verifies sectors 1 and up first, then
  sector 0 (about 2 s). The running image keeps its vector table until then. A Rust image keeps
  everything its boot window needs in sector 0 (image check D9): after an interruption in the
  first pass it still answers the ESP, which can flash again. The C++ 2.1.7 image has most of its
  code in the sectors the first pass erases, so treat any interruption of the first flash (C++ ->
  Rust) as a BOOT0 recovery. An interruption during the sector-0 pass needs BOOT0 with every image
  ([GLUE-DESIGN-STM.md §5.10](GLUE-DESIGN-STM.md#510-what-the-image-cannot-cover)).
- **Sector 0 pending.** When the sector-0 pass itself fails (a UART fault, not a power loss), the
  ESP does not reset the STM, which would not start again: the STM waits in its ROM bootloader,
  the flash shows the phase `sector0_pending` (event `stm_sector0_pending`), and the ESP repeats
  the pass every 30 s, or at once when the flash is started again (any image). Until sector 0 is
  written the ESP refuses its own restarts, the switch back, uploads and the flash abort
  (`409 stm_sector0_pending`, MQTT `cmd/restart` rejected) and lets the automatic ones wait
  (event `restart_deferred`). **Never power-cycle the controller while the phase is
  `sector0_pending`**: the STM would need BOOT0.

### 4.4 When something fails

| Situation | What to do |
|---|---|
| the flash is refused before it starts (`board_mismatch`, `image_chip_mismatch`) | wrong image for the board or the chip (section 1) |
| first flash: `handshake_timeout`, and the STM does not answer afterwards | the HSE did not start (R12). Power-cycle the controller; when the STM answers again (Maintenance → STM32), flash once more. Otherwise BOOT0, mode blank ([docs/revamped/INSTALL.md §6](../revamped/INSTALL.md#6-recovery)) |
| the flash of a Rust image was interrupted (ESP restart, power) | flash again, mode normal: the STM still answers the handshake unless the sector-0 pass was running |
| an interrupted first flash, or an interruption in the sector-0 pass | BOOT0, mode blank (as above) |
| the flash shows `sector0_pending` (event `stm_sector0_pending`); restarts answer `409 stm_sector0_pending` | **keep the power on.** The ESP repeats the sector-0 pass every 30 s; start the flash again (any image, mode normal) to repeat it at once. It ends `done` when sector 0 verifies. If it keeps failing, report the event log before anything else: the last way out is BOOT0, mode blank (as above) |
| `app_not_responding` or `app_version_mismatch` after the flash | Maintenance → Restart and reset → Reset STM, then check Maintenance → STM32 and the event log |
| bench: the warm restore fails (the valves recalibrate after a flash) | stop: no controller gets the Rust STM image (risk R3); report |
| bench: the 1-Wire or I2C timing differs from C++ beyond the slot limits | stop and report (R5) |
| safe mode (3 watchdog resets within 10 min) | as for 2.1.7 ([docs/revamped/INSTALL.md §6](../revamped/INSTALL.md#6-recovery)); the debug terminal (USART6) prints the last fault after its banner |

### 4.5 Back to C++ 2.1.7

Flash `stm217.bin` from the image list, mode normal. The warm state stays: the Rust and the C++
images keep it at the same addresses (decision D3).
