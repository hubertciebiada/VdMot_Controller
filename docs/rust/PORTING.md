# VdMot Revamped in Rust: porting rules

The Rust firmware is a port of VdMot Revamped 2.1.7, not a redesign. The C++ sources
(`software_esp32_revamped`, `software_stm32`) stay in the repository as the reference until the
Rust firmware has run on the hardware. The binding documents stay binding for both:
`software_esp32_revamped/DESIGN.md`, `software_stm32/PROTOCOL_V2.md`,
`docs/revamped/API.md`, `docs/revamped/MQTT.md`, `docs/revamped/INSTALL.md`.

## Layout

```
software_esp32_rust/
  Cargo.toml     workspace of the host-testable crates (core, glue)
  core/          vdm-esp-core: port of software_esp32_revamped/lib/core; #![no_std], no alloc
  glue/          vdm-esp-glue: the logic of software_esp32_revamped/src/*.cpp over port traits;
                 std, host-tested against fakes (the glue suites)
  firmware/      vdm-esp-fw: esp-idf-svc adapters of the port traits and main; target
                 xtensa-esp32-espidf with its own toolchain, not a workspace member
software_stm32_rust/
  (same shape: core = software_stm32/lib/core, glue = software_stm32/src/*.cpp,
   firmware on embassy-stm32, target thumbv7em-none-eabihf)
tools/rust/      Dockerfile, docker.sh (tests, mutation), mutation_gate.py, mutation/
```

Commands (repo root, Docker needed):

```
bash tools/rust/docker.sh test software_esp32_rust
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-core --file src/common.rs
bash tools/rust/docker.sh run "cd software_esp32_rust && cargo clippy --workspace --all-targets"
```

## Contract parity

- Same inputs give the same outputs, byte for byte: UART request and reply lines, HTTP
  documents, MQTT topics and payloads, HA discovery payloads, NVS blobs, LittleFS files, log
  and syslog lines, EEPROM blocks. The Rust and the C++ firmware must be interchangeable in
  both directions (OTA C++ -> Rust -> C++ keeps every setting and every state record).
- Same limits (lengths, capacities, ranges), same rejection rules, same timing constants, same
  event codes and arguments. A quirk that a document or a test describes is kept.
- A C++ bug found while porting is not fixed silently: keep the behaviour, note it in
  `docs/rust/PORT-NOTES.md` (module, what, why it matters) and report it.

## Modules and tests

- One Rust module per C++ module, same name: `lib/core/src/link_policy.cpp` and
  `include/vdm/link_policy.h` become `core/src/link_policy.rs`.
- The header comments are the contract: they become the doc comments of the Rust items.
- Tests: `test/native/test_<module>.cpp` becomes `core/src/<module>/tests.rs`,
  `test_<module>__<part>.cpp` becomes `core/src/<module>/tests_<part>.rs`, each declared in
  the module as `#[cfg(test)] mod tests;` (`#[path]` not needed: a file module `x.rs` with a
  directory `x/` holds its submodules there).
- Every test case and every assertion is ported. Assertions may be regrouped, none dropped.
  Fuzz and random tests keep their iteration counts; the PRNG is `test_support::Rng`.
- Shared test helpers of `test/native/support` go to `core/src/test_support/`.

## Rust rules

- Names: functions and methods snake_case (`elapsedMs` -> `elapsed_ms`), types and enum
  variants as in C++ (`LinkPolicy`, `LinkState::Up`), constants in SCREAMING_SNAKE_CASE
  without the `k` (`kValveCount` -> `VALVE_COUNT`).
- Enums with numeric meaning keep their values: `#[repr(u8)]` (or the C++ underlying type)
  and explicit discriminants; conversions from raw numbers are `fn from_raw(v) -> Option<Self>`.
- `core`: `#![no_std]`, no alloc, `unsafe` forbidden. Fixed-size storage only: arrays,
  `heapless::String<N>`, `heapless::Vec<T, N>`.
- No panic on any input: no indexing or slicing that can go out of bounds on external data, no
  `unwrap`/`expect` outside tests, no division by an unchecked value. Unsigned C++ arithmetic
  that wraps on purpose (millis) uses `wrapping_*`; everything else must not overflow (tests run
  with overflow checks).
- Bytes from outside (UART, HTTP, MQTT, NVS, files) are `&[u8]`; never assume UTF-8.
- Outputs that the C++ writes into `char* out, size_t cap`: the Rust function writes into
  `&mut [u8]` (the C++ `cap` is the slice length, the C++ NUL is not written) or a
  `heapless::String<N>` when N is fixed by the contract, and keeps the C++ result on overflow
  (what fits, what is returned). See "Idioms" for the shared helpers.
- Numbers are formatted with integer arithmetic exactly as the C++ formatters do; no `{:.N}`
  float formatting.
- Time: `now_ms: u32` wrapping like `millis()`, compared only with `elapsed_ms`/`time_reached`.
- Dependencies: pinned with `=` in the workspace `Cargo.toml`; a new one needs a reason in the
  commit message. `core` uses only `heapless`.

## Idioms

(Filled in by the foundation port: the shared writer and text helpers of `common.rs` and
`json_writer.rs` that every module uses.)

## Mutation gate

- `cargo-mutants` per package through `tools/rust/docker.sh mutate`, then
  `tools/rust/mutation_gate.py`: at least 95 % killed overall and for every file, unviable
  mutants not counted (the C++ "stillborn").
- A surviving mutant is killed with a test, or, when it cannot change behaviour, listed in
  `tools/rust/mutation/equivalents/<package>.json` with a reason. Code that cannot run gets
  `#[cfg_attr(test, mutants::skip)]` with a comment that says why (`mutants` is a
  dev-dependency, the attribute exists only in test builds).
- Reports: `tools/rust/mutation/<package>.report.json`.
