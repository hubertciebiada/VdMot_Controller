# Port notes

C++ behaviour that the Rust port keeps although it looks odd, C++ bugs found while porting, and
the places where a C++ test has no Rust form. One entry per finding: module, what, why it
matters. The rules are in [PORTING.md](PORTING.md).

## Kept quirks

| Module | What | Why it matters |
|---|---|---|
| common | `roundTargetPercent(0.49999999999999994)` returns 1: `v + 0.5` rounds to 1.0 in double precision before the truncation. | A target payload just below x.5 can round up. Kept for parity (`round_target_percent`, test `round_target_percent_cases`). |
| common | `isSafeName`/`isHostName` compute `maxLen + 1` in `size_t`; `maxLen = SIZE_MAX` wraps to 0 and accepts only "" (and only with `allowEmpty`). | None in practice (the limits are constants); kept with `wrapping_add`. |
| json_writer | A failed call rolls the text back but not the state flags its `beforeValue()` set (root written, key consumed, first element). | Not observable: the writer is poisoned until `reset()`, which clears them. Kept. |
| json_writer | `number()` formats with `printf("%.*f")`: glibc in the host tests rounds the exact binary value half to even and prints the sign of -0 ("-0.00"); the device runs newlib. | Rust implements the glibc rule with integers (`format_f64_fixed`), byte-identical with glibc on 255,169 random values (all exponents, near-ties, 0..18 decimals). Not compared with newlib on the device: a difference there would be a C++ host/device difference too. |
| version | `VDM_VERSION`/`VDM_MIN_STM_VERSION` are C++ build flags; Rust reads them with `option_env!` at compile time ("0.0.0-native" and "1.4.0" when unset). | The firmware build must export both variables, or the image reports "0.0.0-native". |
| line_assembler | `StaticLineAssembler<N>` requires N >= 2, the base class takes any capacity (0, 1 and null storage drop every line as overflow). | `LineAssembler<N>` owns its storage and allows N = 0 and 1 with the C++ base-class behaviour (the C++ null-storage case is N = 0). |

## C++ tests without a Rust form

| Module | C++ cases | Rust |
|---|---|---|
| all | Output buffers checked for the NUL the C++ writes on failure (`out[0] == '\0'`) or after the text. | Rust does not write the NUL (PORTING.md); the checks are the 0/None results and the bytes past the capacity. |
| common, json_writer, line_assembler, version, image_store, ota_policy | Null-pointer guards: `copyString(.., nullptr)`, `isPrintableText(nullptr, 1)`, `isHostName(nullptr, ..)`, `parseUint/parseInt/parseIpv4/parseOneWireId(nullptr, ..)`, `buildHaId(nullptr, ..)` and a null output, `key(nullptr)`, `value(nullptr, 3)`, `raw(nullptr)`, `jsonEscape(nullptr, ..)`, `feed(nullptr, 5)`, `parseVersion(nullptr, ..)`, `normalizeImageName(nullptr, ..)` and a null output, `normalizeMd5(nullptr, ..)`. | A slice is never null; each case is named in a comment where it stood. Where null equals the empty input in C++ (`boundedLength`, `utf8SequenceLength`, `isPrintableText(nullptr, 0)`, `JsonWriter(nullptr, 100)`, `LineAssembler(nullptr, 100)`), the empty slice is tested. |
| common | `copyString(buf, 0, ..)` (capacity 0). | A `Text<N>` always has room for N bytes and the NUL. |
| failsafe, version, net_policy | Names of out-of-range enum values ("unknown"). | A Rust enum cannot hold them; `from_raw(v) == None` is tested instead. |
| json_writer | The random call test evaluates `jw.fixed(rng(), rng() % 8)` and `number(...)` with two `rng()` calls in one argument list, an unspecified order in C++ (GCC: right to left). | The Rust test draws left to right; its sequence differs from the C++ one in these two calls. |
