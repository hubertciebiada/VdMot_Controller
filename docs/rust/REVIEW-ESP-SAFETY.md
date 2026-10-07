# Review: OTA safety of the Rust ESP32 firmware

The bar: an OTA of the Rust ESP32 firmware onto a device in a closed cabinet never forces anyone
to open the cabinet (no serial reflash, no BOOT0 jumper). This review looks for every path that
breaks it: the boot before and after the boot guard, the guard's records and decisions, the
upload and the restart path, the STM flash run by the ESP, and the state that survives a
reboot. Code base: `revamped-rust` at 92718f5; the fixes are 574d728 and 8459b4f. ESP-IDF
behaviour is quoted from the v5.5.5 sources the firmware builds with (`vdmot-esp-idf` volume).

Severity: **High** a reachable sequence ends in a device that needs the cabinet opened;
**Medium** the same with an unlikely precondition, or a gap the design leaves open; **Low** a
degradation without physical access (an image reverts, a check is lost); **Info** behaviour
worth knowing, no fault.

## Summary

| # | Severity | Finding | Status |
|---|---|---|---|
| F1 | High | A manual switch back from a confirmed image confirmed its target without a trial | fixed, 574d728 |
| F2 | Medium | A restart over MQTT confirmed an image on trial whose web server never answered | fixed, 8459b4f |
| F3 | Low | An image left by an upload stayed confirmed: installed again later, it ran without a trial | fixed, 574d728 |
| F4 | Medium | A failure in the D9 sector-0 pass of an STM flash ends with an NRST pulse into an erased sector 0 | open (design) |
| F5 | Medium | No way back from a confirmed image that crash-loops or hangs before its web server runs | open (design, 6.6) |
| F6 | Medium | WiFi starts on stacks that were measured with WiFi off | open (device campaign) |
| F7 | Low | The first confirmation after a switch rewrites the active otadata sector in place | open |
| F8 | Low | The STM flash flag drops while the flash waits for the EEPROM gate | open (C++ parity) |
| F9 | Low | The trial proves GET only: an image whose POST path fails is confirmed | open (procedure) |
| F10 | Low | Trial detection needs the ELF SHA-256 in the app descriptor; an image without it is blind | open (build check) |
| F11 | Info | Upload and switch-back answers that do not match what happens | open (UX) |
| F12 | Low | Without a usable NVS the guard forgets which image failed last | open |
| — | — | Paths checked without a finding | see the last section |

## F1 — manual switch back confirmed its target (High, fixed)

**Scenario.** B is uploaded by the confirmed A, crashes after the guard at every boot; boot 4
switches back to A (otaOk = A). The operator presses switch back on A (`POST
/api/system/ota/switch-back`, to try B again or by mistake). `BootGuard::switch_to_fallback`
selected B and wrote `otaOk := B` before the restart; B booted confirmed (step 1 of 6.2), no
trial, no boot limit: it crashes at every boot with nothing to switch back to. The same with a
B that never ran: an upload whose MD5 check failed leaves a complete image that verifies
(`esp_ota_set_boot_partition` checks the image only), or one whose `set_boot` failed. And a
race: an upload may start after the switch back was accepted (`upload_begin` does not look at a
pending restart); when it ends between the restart path's read of reason 7 and its switch (the
log flush lies in between), the switch selected the slot the upload just wrote as the confirmed
image.

**Where.** `glue/src/boot_guard.rs` `switch_to_fallback` (now line 549) and
`switch_and_restart` (364); pinned by the old `a_manual_switch_back_works_in_any_state` and
`a_manual_switch_back_restarts_a_confirmed_image_into_the_other_one`.

**Fix.** From a confirmed image the switch removes `otaOk` after `set_boot`: the target boots on
trial with the switching image as its fallback, so three crashes or 15 min without health bring
the guard back. From an image on trial nothing changes (its fallback ran before the upload and
becomes `otaOk`; no ping-pong of two images that wait for a network that is down). Tests
`a_manual_switch_to_the_image_that_failed_its_trial_gives_it_a_new_trial` (the brick sequence,
now back to A), `a_manual_switch_to_the_cpp_firmware_leaves_no_confirmation_behind`.

## F2 — a restart over MQTT confirmed an unproven image (Medium, fixed)

**Scenario.** The new image's network and MQTT work, its web server does not answer (the
loopback self-check fails). During the trial `cmd/restart` arrives over MQTT (the restart button
of Home Assistant, an automation): reason 0, so `confirm_before_restart` confirmed the image
because the network was up (D16 assumed reason 0 came over HTTP). A confirmed image without a
web server takes no upload and no switch back.

**Where.** `glue/src/ota.rs` `service_restart` (now line 599).

**Fix.** A user restart (0, 3) confirms only with a passing self-check younger than 30 s
(`OtaValidator::http_ok`) besides the network and the required STM link; otherwise it counts as
a boot of the trial, so three of them switch back. The core `OtaValidator` keeps the C++
contract. Tests `service_restart_a_user_restart_without_a_passing_self_check_counts_as_a_boot`,
`an_mqtt_restart_of_an_image_whose_web_server_never_answered_does_not_confirm_it`; the C++ cases
of the confirming restarts run with a passing self-check.

## F3 — confirmation kept across another firmware (Low, fixed)

**Scenario.** Rust A is confirmed and uploads the C++ firmware (or switches to it). The C++
firmware ignores `otaOk` and runs for a while, writing its own state. Later it installs the same
build A again: `otaOk == A`, so A booted confirmed without a trial, on state it never ran with.

**Where.** `glue/src/ota.rs` `service_restart`, branch of reason 1 (now line 607).

**Fix.** The restart into an uploaded image removes `otaOk` (`BootGuard::leave_for_upload`, line
530); with F1 a manual switch from a confirmed image does the same. The uploaded image boots on
trial anyway (its own AppId). Tests `an_upload_of_the_cpp_firmware_leaves_no_confirmation_behind`,
`leaving_for_an_upload_removes_the_confirmation_only`.

## F4 — STM flash: a failed sector-0 pass resets an STM without a vector table (Medium, open)

The boards do not wire BOOT0: an STM image is re-flashed only through its own boot window
(GLUE-DESIGN-STM, requirement). D9 writes sector 0 last, so an *interruption* (ESP crash, power
loss) leaves the STM unbootable only during the sector-0 pass (~2 s). A *failure* in that pass
does too, and the flasher makes it final:

- After sector 0 is erased, every failure (NACK or timeout after the retries, `ImageRead` of a
  block, the CRC check of sector 0) goes to `fail()`, which pulses NRST and restores 8N1 because
  the STM was touched (`core/src/stm_flasher.rs` `fail` 1029, `op_failed` 1043, `step_pulse`
  1222). Before the pulse the STM still sits in its ROM bootloader and answers; after it, it
  boots an erased or half-written sector 0: no boot stage, no way back without BOOT0.
- The session retries are shared by both passes (`session_retries_count_over_both_passes`): a
  link flaky enough to use them up in the upper pass gives the sector-0 pass block retries only,
  the case in which erasing sector 0 is most dangerous.
- `ImageRead` in the sector-0 pass is never retried.

ESP side: no physical access needed. An ESP image on trial that required the STM link switches
back after 15 min; a confirmed one keeps running.

**Proposal** (core `stm_flasher` and the STM design, decision of the operator): once sector 0 is
erased, never pulse NRST on a failure: keep the ROM session and retry the sector-0 pass with a
budget of its own (time-bounded), retry image reads, and report a state "STM in its bootloader"
that a later flash request resumes at Sync without a reset; the link policy and the ESP restart
path leave NRST alone in that state (an ESP restart releases NRST and loses the session). Before
erasing sector 0, an upper pass that needed session retries could stop and leave the STM in the
recoverable state of the old boot stage.

## F5 — a confirmed image that crash-loops before its web server runs (Medium, open)

Design 6.6 lists "a defect that shows after the 120 s confirmation" as uncovered. While the web
server answers between two resets, the switch back is the remedy; the paths that end in a
cabinet visit are those that fail before it answers:

- a panic or a watchdog reset at every boot, after the confirmation (persistent state the image
  reads at boot, a rare path, an environment change): the guard counts no boots of a confirmed
  image;
- a hang in `setup` at every boot: the boot deadline (`glue/src/app.rs` `boot_deadline`, 299)
  restarts a confirmed image every 60 s forever;
- a hang or a panic in the app thread before `web_begin` (`app.rs` 633): the TWDT restarts it,
  the web server never starts, MQTT may still run.

The decoders of persisted data were read for unchecked lengths and indexes (the config blobs and
cfgx records, the network trial, target, lease and guard records, the image index, the log
rotation, the discovery list reader): none found. The class stays: the port is new code.

**Proposal** (design change, the operator's decision): a counter of consecutive short boots in
NVS (written in `main` after the guard, cleared after N minutes of uptime with the network up
and a passing self-check); above K (e.g. 6) the guard switches to the other slot when it
verifies, removing `otaOk` (the target runs a trial, F1), once per image (recorded like
`away`), so two images that both fail in this environment do not take turns.

## F6 — WiFi starts on stacks measured with WiFi off (Medium, open)

The stack sizes come from QEMU peaks with WiFi off (design 2.1, risk 12). WiFi is built on two
stacks: `main` (32 KB, peak 27.3 KB in QEMU) when the interface is WiFi (`net.begin`,
`glue/src/net.rs` 845), and the app thread (10 KB, peak 8.1 KB) for the Auto fallback after 30 s
without an Ethernet address and the back-off reconnects (`net.rs` `start_wifi` 440 →
`firmware/src/adapters/wifi.rs` `build` 60: `esp_wifi_init`, the netif, `esp_wifi_start`). An
overflow there is a crash at every WiFi start: on an Ethernet device with a WiFi fallback a
switch or cable outage turns into a reboot loop that ends only when Ethernet is back, on a
confirmed image (an interface change itself runs on a network trial and is reverted after one
crash). **Recommendation:** the device campaign of 2.1 starts WiFi on both paths and reads the
minima of `/api/health`; until then the app thread could get 2 KB more.

## F7 — first confirmation rewrites the active otadata entry (Low, open)

`esp_ota_set_boot_partition` writes the inactive otadata sector with state UNDEFINED (rollback
off) or NEW (the C++ image). The guard's confirmation calls `esp_ota_mark_app_valid_cancel_rollback`
(`firmware/src/adapters/ota.rs` 125; at every confirmed boot, `boot_guard.rs` 435, and at each
confirmation, 494), which rewrites the *active* sector in place when its state is not VALID:
erase, then write (ESP-IDF `app_update/esp_ota_ops.c` `esp_ota_current_ota_is_workable` and
`rewrite_ota_seq`). A power loss between the two invalidates the entry and the bootloader boots
the other slot (the previous selection). No cabinet visit: that image ran before; it confirms
itself or runs a trial; after a switch away from a failed image, that image gets three more
boots. It happens once per switch or upload, within one sector erase. The devices' bootloader
has no rollback, so the call matters only for a rollback-capable one. **Option:** mark valid
only when the active entry is NEW or PENDING_VERIFY (a port method that reads the state).

## F8 — the STM flash flag drops during the EEPROM gate (Low, open, C++ parity)

`request_flash` raises the flag at once ("the restart either made us refuse or sees the flag"),
but the next snapshot clears it: the flasher is still Idle while the session waits for the STM
EEPROM (`flash_pending`), and `publish_stm_snapshot` derives the flag from the phase only
(`glue/src/shared.rs` 157; C++ `app.cpp` 255 the same). For up to 10 s (`ResetGate::MAX_WAIT_MS`)
an ESP upload is accepted, the heap guard and restarts are not held back. A restart in that window is harmless
(the restart path checks the flag every pass and the erase starts seconds after the flasher).
An ESP upload then runs next to the STM flash: flash writes stall the cache and delay the UART2
interrupts, so STM replies get lost and retried, at worst into F4. **Fix candidate:**
`flash_active = !idle || s.flash_pending` (the web's STM commands then get 409 `flashing` during
the gate too), or a separate flag for the OTA host and the heap guard.

## F9 — the trial proves GET, not POST (Low, open)

The self-check is `GET /api/health` over loopback (`ota.rs` `self_check`). An image whose POST
body path fails on the device (the upload and the switch back are both POSTs) is confirmed after
120 s and can then take neither. The host tests and the QEMU scenarios run the same code
(esp_http_server and the adapter, OpenETH instead of the EMAC), so a device-only failure is
unlikely. **Recommendation:** the first-flash procedure on an accessible device includes an ESP
upload (the C++ image back) and a switch back before any cabinet device; or the self-check sends
a small POST (`POST /api/config?dryRun=1` with `{}`).

## F10 — trial detection needs the ELF SHA-256 in the image (Low, open)

`AppId` is `app_elf_sha256[..8]`, which `esptool elf2image --elf-sha256-offset 0xb0` writes
(`tools/rust/esp/docker.sh` does). An image built without it has `AppId` 0…0, as every legacy
1.4.x image in `releases/WTH32` has: two such Rust builds would look identical, and an update
between them would boot confirmed without a trial. **Recommendation:** the build and the release
packaging fail on an all-zero ELF SHA-256 at image offset 0xB0.

## F11 — answers that do not match what happens (Info, open)

- A second file part after a complete ESP image: the image is verified, selected and the restart
  requested, the answer is 400 `upload_failed` "one file per request"
  (`glue/src/web_server/uploads.rs` `first_data` 214, `end` 272). The client going away after
  the closing boundary of the file part restarts the device without an answer.
- `POST /api/system/ota/switch-back` answers 202 when the other slot has a readable description
  but no image that verifies (a partial upload): the switch is refused at the end of the restart
  path, event 107 -3, no restart (`web_server.rs` `switch_back` 1853).
- The restart path waits for an STM flash, not for a running ESP upload (C++ parity): a network
  watchdog or user restart cuts the upload; the confirmed image keeps running, its fallback slot
  is left partial.

## F12 — without a usable NVS the guard forgets which image failed last (Low, open)

With an NVS that cannot be opened or written the boot count lives in the RTC mirror and every
boot of a confirmed image is a trial (`otaOk` cannot be read). After a switch from T to F the
mirror's breadcrumb names T, but `plan` takes the switched-away image only from `otaTrial`
(`glue/src/boot_guard.rs` `plan`), and F's first trial boot overwrites the mirror: if F also
fails three times in a row, the guard switches to T again, and the two take turns. No cabinet
visit (each image runs a trial), and it needs a broken NVS and two failing images. **Fix
candidate:** take `away` from the breadcrumb when no trial record can be read.

## Paths checked without a finding

| Area | What was checked |
|---|---|
| `main` before the guard | only plain adapter values, the NRST release, `nvs_flash_init` (erase only on no free pages / new version, Arduino's rule) and the guard run before the boot is counted; no allocation that can fail, no blocking call; the IDF 5.5 startup and the devices' bootloader are covered by the QEMU boot chain (risk 7.1) |
| Guard: records | fixed-size encodings, magic, version and CRC checked; a short, long or damaged record reads as absent; the RTC mirror sits in `.rtc_noinit` at 0x50000000 (104 B, linker map of the release build) |
| Guard: switch target | `set_boot` goes through `esp_ota_set_boot_partition`, which validates the image first; an erased or partial slot is never selected; `verify` of the fallback at boot uses `esp_image_verify` (the C++ image passes: its `max_chip_rev_full` 0 means unset in IDF 5.5) |
| Guard: power loss | at every step of the boot decision, the switch, the confirmation and the factory reset: the worst outcome is a trial that restarts its count or one more trial; never a selected slot that does not verify |
| Guard: both slots Rust, otadata corrupt | both images run the same guard; with both otadata entries invalid the bootloader boots ota_0 and the guard recovers (a failed image runs a new trial and switches again) |
| Upload | sequential writes erase per sector; power loss or a cut client leaves otadata as it was; an MD5 failure or a failed last sector selects nothing; `esp_ota_end` verifies the whole partition (leftovers that complete an identical prefix give a valid, SHA-256-checked image, which runs on trial) |
| Restart path | waits for an STM flash; the rollback and the switch back keep their order with an upload restart; a refused switch keeps the image running as confirmed |
| Network watchdog | its restarts count as trial boots; the 15 min rollback comes before its second restart (5 + 20 min) |
| MQTT | a retained `cmd/restart` is cleared and echoed before it acts (button gate): no restart loop |
| Persisted state | decoders of NVS blobs, cfgx records, RTC records, LittleFS files read for unchecked lengths: none (see F5 for the class) |
| Timing | boot deadline 60 s, TWDT 30 s on IDLE0 and the three threads; every blocking wait of a thread is bounded |

## Verification

- Host: `bash tools/rust/docker.sh test software_esp32_rust`: core 1221, glue 972 (8 new
  cases), 1 ignored; clippy `-D warnings` of the workspace and `cargo fmt --check` clean.
- Firmware: no source changed; the glue items it uses (`BootGuard::boot`, `BootVerdict`,
  `BOOT_DEADLINE_MS`, the wiring) keep their signatures.
- Not run: the QEMU harness (its switch-back scenario switches to the C++ image, which ignores
  `otaOk`; its checks are unchanged) and the mutation gate. Files for the gate:
  `glue/src/boot_guard.rs`, `glue/src/ota.rs`.
