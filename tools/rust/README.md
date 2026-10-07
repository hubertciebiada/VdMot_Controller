# Rust tooling

Every build and test of the Rust ports runs in a container; Docker is the only host
requirement (Windows, macOS, Linux). Porting rules and the per-crate commands:
[docs/rust/PORTING.md](../../docs/rust/PORTING.md).

| script | image | used for |
|---|---|---|
| `docker.sh` | `vdmot-rust:<hash of Dockerfile>`: Rust 1.99.0, clippy, rustfmt, cargo-mutants 27.1.0, the `thumbv7em-none-eabihf` target, `llvm-tools`, g++ 12, Python 3 | host tests, mutation gate, STM32 images, image check |
| `renode.sh` | `antmicro/renode:1.16.1` (D11) | the STM32 images in Renode: boot stage and application |
| `stm/golden/run.sh` | the native image (`tools/native/docker.sh`) | the C++ goldens of the glue_system suites |
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
