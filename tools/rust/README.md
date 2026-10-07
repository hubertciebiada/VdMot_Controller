# Rust tooling

Every build and test of the Rust ports runs in a container; Docker is the only host
requirement (Windows, macOS, Linux). Porting rules and the per-crate commands:
[docs/rust/PORTING.md](../../docs/rust/PORTING.md).

| script | image | used for |
|---|---|---|
| `docker.sh` | `vdmot-rust:<hash of Dockerfile>`: Rust 1.99.0, clippy, rustfmt, cargo-mutants 27.1.0, the `thumbv7em-none-eabihf` target, `llvm-tools`, g++ 12, Python 3 | host tests, mutation gate, STM32 images, image check |
| `renode.sh` | `antmicro/renode:1.16.1` (D11) | boot stage tests of the STM32 images |
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
3. **renode.sh** runs `software_stm32_rust/renode/boot.robot` (scenarios E1-E10 of §5.7) once
   per image and prints one `EVIDENCE` line per scenario. Arguments: image names
   (`tools/rust/renode.sh STM32F401_C2`), and after `--` options of `renode-test`, e.g.
   `-- --include E7` for one scenario (tags E1 ... E10). Results in
   `software_stm32_rust/renode/results/<image>/` (not in git). About 4 minutes per image.

Host tests and the mutation gate of the boot crate:

```
bash tools/rust/docker.sh test software_stm32_rust
bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-boot
```
