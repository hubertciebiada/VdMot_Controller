# Rust tooling

Every build and test of the Rust ports runs in a container; Docker is the only host
requirement (Windows, macOS, Linux). Porting rules and the per-crate commands:
[docs/rust/PORTING.md](../../docs/rust/PORTING.md).

| script | image | used for |
|---|---|---|
| `docker.sh` | `vdmot-rust:<hash of Dockerfile>`: Rust 1.99.0, clippy, rustfmt, cargo-mutants 27.1.0, the `thumbv7em-none-eabihf` target, `llvm-tools`, g++ 12, Python 3 | host tests, mutation gate, STM32 images, image check |
| `renode.sh` | `antmicro/renode:1.16.1` (D11) | the STM32 images in Renode: boot stage and application |
| `stm/golden/run.sh` | the native image (`tools/native/docker.sh`) | the C++ goldens of the glue_system suites |
| `esp/docker.sh` | `vdmot-rust-esp:<hash of esp/Dockerfile>`: the Xtensa toolchain `esp` 1.98.1.0 (espup 0.18.0), ldproxy 0.3.5, esptool 5.4.0, littlefs-python 0.19.0, Espressif QEMU 9.2.2 (esp-develop-20260417), mklittlefs 1.203.210628, Mosquitto 2.0.11 (Debian bookworm); ESP-IDF v5.5.5 is fetched by esp-idf-sys into the volume `vdmot-esp-idf` on the first build | ESP32 firmware images, their size, the QEMU harness |
| `mutation_gate.py` | - | per-file 95 % gate over a cargo-mutants run |

## STM32 images

Three steps from the repository root; each one needs the one before.

```
bash tools/rust/docker.sh fw            # 1. build
bash tools/rust/docker.sh image-check   # 2. check the files
bash tools/rust/renode.sh               # 3. run them in Renode
```

1. **fw** builds the four images of `software_stm32_rust/firmware` (features `f401|f411` x
   `c1|c2`, release profile) into `software_stm32_rust/firmware/images/`
   (`STM32F401_C1.elf`, `.bin`, `.map` ... `STM32F411_C2.*`, not in git), and the boot probe of
   each chip (B3): a link of the boot stage and the fault handlers whose panic handler calls a
   function that does not exist, so the build fails while any panic path is left in them.
2. **image-check** checks each image against the ESP's own code and the layout rules of
   docs/rust/GLUE-DESIGN-STM.md §5.8:
   - C1-C3 with the C++ `validateImage`/`checkBoard` of
     `software_esp32_revamped/lib/core/src/stm_flasher.cpp` (the code of ESP 2.1.7), built with
     g++ in the container (`software_stm32_rust/image-check/esp_validate.cpp`): accepted without
     `force` for every chip ID the image fits, version, board tag, no marker conflict, the
     erase set, and the acceptance of the image's `gvers` reply after a flash;
   - C4, C5, D9 with `vdm-stm-image-check`: vectors, the ID block as the first version string,
     one board marker, the no-init cells at the C++ 2.1.7 addresses, the 128 KiB budget, and the
     sector-0 proof (every function and flash datum the boot stage and the fault handlers reach
     lies in `.vdm_boot`). It ends with a size table.
3. **renode.sh** runs `software_stm32_rust/renode/boot.robot` (the boot stage, E1-E10 of §5.7)
   and `app.robot` (the application against the C++ goldens, A1-A6) once per image and prints
   one `EVIDENCE` line per scenario. Arguments: image names (`tools/rust/renode.sh
   STM32F401_C2`), and after `--` options of `renode-test`, e.g. `-- --include A3` for one
   scenario (tags E1 ... E10, A1 ... A6). Results in `software_stm32_rust/renode/results/<image>/`
   (not in git). About 12 minutes per image.

   The machine (`vdm.resource`): Renode's STM32F4 platform with the chip sizes and clocks
   (`stm32f401.repl`, `stm32f411.repl`) and the board around it (`vdm_board.repl`): USART6 for
   the debug terminal, `VdmValves.cs` (ADC1 with the twelve valve motors of the C++ valve sim,
   revolution pulses on PA4) and `VdmEeprom.cs` (the 24LC64 on I2C1). Renode compiles the two
   C# models at the start of every test; a change of them while a run is going breaks the tests
   that follow in that run ("assembly already loaded"). `vdm_renode.py` reads the C++ goldens
   for app.robot and checks the no-init cells.

Host tests and the mutation gate of the boot crate:

```
bash tools/rust/docker.sh test software_stm32_rust
bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-boot
```

## C++ goldens of the glue_system suites

`software_stm32_rust/glue/tests/golden/<slug>.txt`, one per C++ glue_system case (§7.4): the
UART bytes of every boot with their times, how each boot ended, the EEPROM rows and the
no-init bytes. The Rust system tests (`glue/src/system/tests_*.rs`) reproduce them byte for
byte and app.robot replays some of them against the images. After a change of the C++ glue
or its suites:

```
bash tools/native/docker.sh run "bash tools/rust/stm/golden/run.sh"
```

It builds `tools/rust/stm/golden/` (the C++ glue_system suites with a recorder linked in by
GNU ld `--wrap`) and writes every golden again; every C++ case must pass.

## ESP32 firmware

```
bash tools/rust/esp/docker.sh build [features]   # release build and app image (default: WiFi)
bash tools/rust/esp/docker.sh size [features]    # the same, then the size; fails above 1,228,800 B
bash tools/rust/esp/docker.sh lint               # clippy -D warnings: default, nowifi, the QEMU test build
bash tools/rust/esp/docker.sh qemu [scenarios]   # the QEMU variants, then the end-to-end harness
```

`build` writes `/target/images/<variant>/vdm-esp-fw.bin` (with `.elf` and `.map`) in the target
volume of the checkout; variants: `default`, `nowifi`, `qemu` (OpenETH of Espressif QEMU instead
of the LAN8720), and the boot guard test images `qemu-fail-boot` and `qemu-hang-setup`. Nothing
is flashed: the first flash of a device is the operator's.

`qemu` runs `esp/qemu/harness.py` with the devices' bootloader and partition table
(`software_esp32/`) and the C++ firmware 2.1.7 in the other OTA slot. The C++ image is the
release asset `VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin` in `esp/qemu/cache/`
(gitignored), used only when its SHA-256 is the one pinned in `esp/docker.sh`; nothing is fetched
unless `VDM_FETCH_CPP=1` is set (then `gh release download`). Scenarios (the harness docstring
has the details and the limits of the C++ image in QEMU):

| Scenario | Proves |
|---|---|
| `boot` | the devices' bootloader starts the Rust image on trial; ESP upload refused during the trial (409); confirmed after 120 s of health (NVS `otaOk`, otadata VALID); confirmed after a power cycle; `POST /api/system/ota/switch-back` restarts with otadata on app0, the C++ firmware starts from it |
| `rollback` | an image that panics right after the boot guard: 3 counted boots, the 4th switches to the C++ firmware; otadata and NVS as the switch left them (read before the C++ firmware runs); the bootloader never touches the trial state |
| `deadline` | an image that never reaches the app thread: the 60 s boot deadline restarts it 3 times, then the switch |
| `badfs` | a LittleFS partition of random bytes: the boot guard decides before the first LittleFS line and the first event; the glue formats it (disk version 2.0) and the firmware comes up |
| `ota` | the C++ image uploaded through `POST /api/ota/esp` (multipart, as the dashboard) into the empty slot: byte-identical and selected at the restart; the C++ firmware starts from it |
| `netwatch` | a NIC without IPv4 and `net.reconnectTimeoutMin` 1: the network watchdog restarts the interface after 60 s, then the ESP after 2 min; the next boot is trial boot 2 (the restart counts) |
| `littlefs` | a LittleFS image of the C++ toolchain's mklittlefs with the C++ layout: the Rust app restores the C++ config backup and appends to the C++ log; mklittlefs reads the result, disk version stays 2.0 |
| `nvs` | an NVS from ESP-IDF's generator with the C++ codec's blobs: the Rust app serves the C++ config; the config it saves decodes with the C++ codec |
| `dashboard` | gzip bytes, ETag, Cache-Control, 304 without Content-Type, files equal `software_esp32_revamped/web` |
| `api` | every GET route against the structure of `software_esp32_revamped/tools/mock_api.py`, the 404/405/410 refusals, config dry run, save and refusal, an STM image upload and delete, the chunked log, the httpd stack and heap after each kind of request |
| `mqtt` | Mosquitto at 10.0.2.2:1883: connect after a config save, status online, the idle heap 60 s after boot, values, HA discovery (every config the device reports is at the broker), a reconnect after a broker restart |
| `soak` | `software_esp32_revamped/tools/loadtest.py` (request timeout raised to 30 s for QEMU) with 3, then 10 workers for 180 s each and a config POST every 15 s: free heap, its minimum and the largest block before, during and after, the 503 counts; no restart, the heap back after the load (QEMU figures are indicative only) |
| `health` | not in the default set (about 17 min): a trial that needs the STM (NVS `otaStm` 1; QEMU has none) with the network up: 15 min without 120 s of health, restart reason 4 into the C++ firmware, no extra counted boot |

The flash files and serial logs stay in `/target/qemu/` of the target volume. The harness prints
the lowest free stack per task seen in `/api/health`.
