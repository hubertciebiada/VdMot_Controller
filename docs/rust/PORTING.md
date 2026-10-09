# VdMot Revamped in Rust: porting rules

The Rust firmware is a port of VdMot Revamped 2.1.7, not a redesign. The C++ sources
(`software_esp32_revamped`, `software_stm32`) stayed in the repository as the reference until the
Rust firmware had run on the hardware; since then they are on the branch `revamped` and in the
tag `v2.1.7-revamped`, and the paths of C++ files in this tree name those sources. The binding documents stay binding for both:
`docs/revamped/DESIGN.md`, `docs/revamped/PROTOCOL_V2.md`,
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
tools/rust/esp/  Dockerfile, docker.sh (ESP32 firmware: build, size, lint, QEMU), image_size.py,
                 qemu/harness.py (end-to-end checks with the devices' bootloader)
```

Commands (repo root, Docker needed):

```
bash tools/rust/docker.sh test software_esp32_rust
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-core --file core/src/common.rs
bash tools/rust/docker.sh run "cd software_esp32_rust && cargo clippy --workspace --all-targets"
```

ESP32 firmware (repo root, Docker needed; the first build fetches ESP-IDF into the volume
`vdmot-esp-idf`):

```
bash tools/rust/esp/docker.sh build                 app image of the default build (WiFi)
bash tools/rust/esp/docker.sh size                  size against the 1,228,800 B budget (fails above)
bash tools/rust/esp/docker.sh lint                  clippy -D warnings of every feature set
bash tools/rust/esp/docker.sh qemu                  end-to-end scenarios in QEMU (tools/rust/README.md)
```

The firmware crate is outside the host workspace and outside the mutation gate: it builds only
for `xtensa-esp32-espidf`, and its adapters are calls into ESP-IDF that no host can run. That is
why it holds no decisions: one adapter per port, each method an ESP-IDF call or a fixed sequence
of them with error mapping, and `main` in the order of GLUE-DESIGN-ESP.md 6.4. Anything with
logic (a choice, a format, a state machine) belongs in the glue behind a port method, where the
host tests and the 95 % gate cover it. The firmware is checked by clippy (`docker.sh lint`) and
the QEMU harness instead.

## ESP32 firmware: what an OTA update from the C++ firmware depends on

The devices are updated by OTA only: the bootloader of their first serial flash (Arduino-ESP32,
ESP-IDF 4.4 era, no app rollback) and the partition table stay. Binding for the firmware crate:

- The image header is DIO, 80 MHz, 4 MB, chip revision v0.0 and up (`sdkconfig.defaults`);
  `partitions.csv` is the devices' table and never changes.
- `BOOTLOADER_WDT_DISABLE_IN_USER_CODE` stays unset (the app disables the 9 s RTC watchdog of the
  bootloader) and `ESP_SYSTEM_ESP32_SRAM1_REGION_AS_IRAM` stays off (an older bootloader cannot
  boot such an app).
- The boot guard (`glue/src/boot_guard.rs`) decides in `main` right after the STM release and the
  NVS init, before any other subsystem; it is the only rollback the devices have. An image that
  fails before `main` (ESP-IDF startup) cannot be rolled back.
- NVS is erased only for the two init errors Arduino-ESP32 erased on (`NO_FREE_PAGES`,
  `NEW_VERSION_FOUND`): a partition the firmware cannot initialise would leave the device on its
  defaults with no way to save settings, a serial reflash in the cabinet. Any other init error
  leaves it alone. ESP-IDF 4.4 and 5.5 use the same NVS page version (0xfe), so the C++ settings
  never meet those errors.
- LittleFS keeps on-disk version 2.0 (`LITTLEFS_MULTIVERSION`, `LITTLEFS_DISK_VERSION_2_0`), so the
  C++ firmware can still mount it after a switch back. The adapter's mount never formats
  (`format_if_mount_failed` 0); the glue formats only after a failed mount, as the C++
  `beginFs` did (`LittleFS.begin(false)`, then `format()`).
- The image stays below 1,228,800 B: std is built without its default features (no backtrace
  symbolizer) and the release profile uses `opt-level = "z"` (GLUE-DESIGN-ESP.md 7, item 7).

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

The shared helpers live in `core/src/common.rs` and `core/src/json_writer.rs`; every port uses
them the same way.

Inputs:

- C++ `(const char* s, size_t len)` -> `s: &[u8]`, len = `s.len()`. A NUL inside is data unless
  the C++ contract stops at it (`build_ha_id`, `bounded_length`).
- C++ NUL-terminated `const char*` -> `&[u8]` read up to its end or its first NUL (`c_str(s)`):
  Rust `f(s)` gives what C++ `f(s + NUL)` gives, also for NUL-padded C-layout arrays (NVS, RTC).
- `strlen`/`strnlen` -> `c_str(s).len()`/`bounded_length(s, max)`; `strstr(h, n) != nullptr`
  -> `contains_bytes(h, n)`; `strcmp`/`memcmp` -> slice `==`; prefixes -> `starts_with`.
- A null pointer: a slice is never null. Null with a meaning of its own becomes an `Option`
  (`jw.value(None::<&str>)` writes `null`); a C++ test of a mere null guard has no Rust form and
  is named in a comment (`// C++ f(nullptr, ...): no Rust form`).

Text members and outputs:

- C++ member `char f[N + 1]` -> `Text<N>` (`heapless::Vec<u8, N>`: bytes, no NUL).
  `copyString(f, sizeof f, src)` -> `copy_string(&mut f, src)` (false: truncated; `&mut Text<N>`
  coerces to `&mut TextView`).
- C++ output `char* out, size_t cap` -> `out: &mut [u8]` whose length is the C++ cap, NUL
  included: at most `out.len() - 1` bytes of text fit, the NUL is not written, the text is
  `&out[..n]`. The function returns the length:
  - "length, 0 (and "") when it does not fit" -> `-> usize` (`fmt_fit` or `TextBuf::fit`);
  - truncating `snprintf` (`n < cap ? n : cap - 1`) -> `-> usize` (`fmt_trunc`, `TextBuf::len`);
  - `bool f(..., char* out, size_t cap)` -> `-> Option<usize>` (`TextBuf::fit_opt`).
- Building text: `fmt_fit(out, format_args!("{}.{}", a, b))` for one `snprintf`; for several
  steps `let mut w = TextBuf::new(out);` then `w.push(c)`, `w.push_bytes(s)`,
  `write!(w, "{:02}-{:x}", a, b)` (core::fmt integers are printf `%u %d %02u %x`), and
  `w.fit()`/`w.len()`/`w.fit_opt()`. Floats never go through `{:.N}`: `printf("%.*f")` is
  `format_f64_fixed(v, decimals, out)` (glibc-exact, |v| < 2^128).
- Tests: `char buf[16]; f(x, buf, 7)` -> `let mut buf = [0u8; 16]; f(x, &mut buf[..7])`. A C++
  check of `buf[0] == '\0'` after a failure is the 0/None result; checks past the capacity stay.

Numbers, ids, time:

- C++ `bool parseX(..., T& out)` (out unchanged on failure) -> `-> Option<T>`: `parse_uint(s,
  max)`, `parse_int(s, min, max)`, `parse_ipv4(s)`, `parse_one_wire_id(s)`. Where the C++ resets
  `out` on failure, return the value with its validity (`parse_version(s) -> Version`, `.valid`).
- `format_ipv4(ip, out)` (16 bytes), `format_one_wire_id(&id, out)` (24 bytes), `is_zero(&id)`,
  `crc_valid(&id)`, `build_hostname(station, out)`, `build_ha_id(name, out)`.
- `elapsed_ms(now, since)`, `time_reached(now, deadline)`, `Backoff::new(min, max)`. Wrapping
  on purpose -> `wrapping_*`; saturating C++ counters -> `saturating_add`.

JSON:

```
let mut buf = [0u8; 512];
let mut jw = JsonWriter::new(&mut buf);
jw.begin_object();
jw.kv("valves", 12);           // JsonValue: &str, &[u8], &Text<N>, i8..i64, u8..u32, bool,
jw.kv("name", &cfg.name);      //   Option<T> (None -> null); strings are exact bytes
jw.key("temp");                //   (the C++ value(s, len)); key() and raw() take C strings
jw.fixed(215, 1);              // 21.5; number(v, decimals) for doubles
jw.end_object();
if !jw.ok() { /* overflow or misuse */ }
let doc = jw.as_bytes();       // or into_bytes() for the lifetime of the buffer
```

Types:

- Enums with C++ values: `#[repr(u8)]`, the same discriminants, `from_raw(v) -> Option<Self>`.
  `xName(e)` -> `x_name(e) -> &'static str`; the C++ "unknown" of an out-of-range value cannot
  occur, its test becomes `from_raw(v) == None`.
- Nested C++ types become module-level types named outer + inner (`ResetGate::State` ->
  `ResetGateState`); nested constants become associated constants (`ResetGate::POLL_MS`).
- A class with member initialisers implements `Default` (manually where a value is not zero);
  constructors with arguments are `new(...)`.

Tests and mutation:

- Generators (`test_support`, bit-exact with the C++ tests): `std::mt19937 rng(S)` ->
  `Rng::new(S)` (`next_u32()`, `below(n)` = `rng() % n`); `srand(S)`/`rand()` ->
  `CRand::new(S).rand()` (glibc); `seed = seed * A + C` -> `Lcg::new(S, A, C).next_state()`
  (`Lcg::numerical_recipes`, `Lcg::ansi_c`). `assert_text(got, "want")` prints bytes escaped.
- C++ unsigned wrap in tests (`s + 4999u`) -> `s.wrapping_add(4999)`; std types in tests come
  from `use std::{vec::Vec, string::String, format, vec}` (the crate is `no_std`).
- No doctests in `core` (every mutant would compile them).
- cargo-mutants also mutates constant expressions (`-500`, `24 * 60`): test the constants.
  Avoid code with equivalent mutants: `|` of disjoint bits (use `+`, a table or a decoder),
  `if a > b { a } else { b }` (use `max`), a bound both branches treat alike. Loop over slices
  instead of index arithmetic, and let test helpers that loop until done assert progress: a
  mutant that stops the progress must fail, not hang until the 20 s timeout.

### STM core additions

- `BufWriter<B>` and `LineAssembler<B>` work over `B: Storage`; `StaticBufWriter<N>` and
  `StaticLineAssembler<N>` own their array (`Default`). C++ `(buf, cap)` becomes
  `new(&mut buf[..cap])`; N bytes hold N - 1 characters, the text is `as_bytes()`/`line()`.
- Out-parameters that the C++ always writes stay `&mut T`; C++ default arguments become explicit
  parameters with the doc line "C++ default for x: y".
- A nested type whose name clashes with the prelude gets the outer name (`PresenceTest::Result`
  -> `PresenceResult`); inheritance becomes a field (`ConfigImage.layout`).
- `.noinit` records are `#[repr(C)]` with `offset_of!` asserts, and their CRC runs over an
  explicit little-endian serialization, so C++ and Rust read each other's warm state.
- Cases of `test_fuzz.cpp` go to `<module>/tests_fuzz.rs`.

## Mutation gate

- `cargo-mutants` per package through `tools/rust/docker.sh mutate`, then
  `tools/rust/mutation_gate.py`: at least 95 % killed overall and for every file, unviable
  mutants not counted (the C++ "stillborn").
- A surviving mutant is killed with a test, or, when it cannot change behaviour, listed in
  `tools/rust/mutation/equivalents/<package>.json` or, one file per module, in
  `tools/rust/mutation/equivalents/<package>/<module>.json` with a reason. An entry matches by
  file, function and mutation name, without the line, so it also takes a new survivor of the
  same name: a change of a file with entries needs its entries checked by hand
  ([mutation-stm32.md](mutation-stm32.md#name-matching)). Code that cannot run gets
  `#[cfg_attr(test, mutants::skip)]` with a comment that says why (`mutants` is a
  dev-dependency, the attribute exists only in test builds).
- Reports: `tools/rust/mutation/<package>.report.json` (not committed, like the C++ reports).
- cargo-mutants also mutates `const` expressions: derived contract constants get a test that pins
  their value. `--file` globs with a `/` match from the workspace root (`core/src/x.rs`).
