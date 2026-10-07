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
| lease_client | A lease config value the codec refuses (timeout 1..4 or above 1440 min, failsafe percent 101..254) makes `next()` return true with the empty `RequestLine{}` its failed `buildSetLeaseTimeout`/`buildSetFailsafe` left: the link refuses it (Invalid), the request is lost after 10 s and every round fails with reason 1 (`lease_config_failed` after 3). | Not reachable in the firmware: `effectiveLeaseConfig` reads a Config whose `timeoutMin` and `failsafePct` passed their OffOrRange/PctHold checks. Kept: `next()` gives `Some(RequestLine::default())` (test `a_config_value_the_codec_refuses_hands_out_the_empty_request`). |
| lease_client | `onCompletion` of a push answered Ok clears `pushMask_ & ~(1u << req.valve)`; for the empty request above (valve `kNoValve` = 254, only if a caller completed a request it never queued) the shift is undefined behaviour in C++ (x86: no bit of the 12-bit mask). | Rust clears no bit (`checked_shl`), the x86 result. Not reachable through the session. |

## C++ tests without a Rust form

| Module | C++ cases | Rust |
|---|---|---|
| all | Output buffers checked for the NUL the C++ writes on failure (`out[0] == '\0'`) or after the text. | Rust does not write the NUL (PORTING.md); the checks are the 0/None results and the bytes past the capacity. |
| common, json_writer, line_assembler, version, image_store, ota_policy | Null-pointer guards: `copyString(.., nullptr)`, `isPrintableText(nullptr, 1)`, `isHostName(nullptr, ..)`, `parseUint/parseInt/parseIpv4/parseOneWireId(nullptr, ..)`, `buildHaId(nullptr, ..)` and a null output, `key(nullptr)`, `value(nullptr, 3)`, `raw(nullptr)`, `jsonEscape(nullptr, ..)`, `feed(nullptr, 5)`, `parseVersion(nullptr, ..)`, `normalizeImageName(nullptr, ..)` and a null output, `normalizeMd5(nullptr, ..)`. | A slice is never null; each case is named in a comment where it stood. Where null equals the empty input in C++ (`boundedLength`, `utf8SequenceLength`, `isPrintableText(nullptr, 0)`, `JsonWriter(nullptr, 100)`, `LineAssembler(nullptr, 100)`), the empty slice is tested. |
| common | `copyString(buf, 0, ..)` (capacity 0). | A `Text<N>` always has room for N bytes and the NUL. |
| failsafe, version, net_policy | Names of out-of-range enum values ("unknown"). | A Rust enum cannot hold them; `from_raw(v) == None` is tested instead. |
| json_writer | The random call test evaluates `jw.fixed(rng(), rng() % 8)` and `number(...)` with two `rng()` calls in one argument list, an unspecified order in C++ (GCC: right to left). | The Rust test draws left to right; its sequence differs from the C++ one in these two calls. |
| stm_flasher | `checkBoard(nullptr, ..)`, `checkBoard(.., nullptr)`, `boardTagValid(nullptr)`; the names of out-of-range phases, errors and board checks ("unknown", legacy status 8). | The empty slice; `from_raw(v) == None`. |
| stm_flasher | Three cases flash images above 16 KiB: "normal mode end to end on a 1.4.9-sized F411 image", "image sizes around block and word boundaries" (16 KiB + 4) and "full 512 KiB image erases all 8 sectors". | Their erase frames, write, read and phase order are the D9 ones (intended deviations below); the C++ expectations stand in comments next to them. |

## Intended deviations

| Module | What | Why it matters |
|---|---|---|
| stm_flasher | Decision D9 (GLUE-DESIGN-STM.md §8): an image above 16 KiB is flashed in two passes of Erasing, Writing and Verifying, sectors 1..n first (blocks from 0x08004000 upwards), then sector 0 (blocks 1..63, block 0 last). C++ erases all sectors with one frame, writes blocks 1..n-1 and block 0, then reads back blocks 0..n-1. Each pass compares the CRC32 of the image bytes it verified with the CRC the Validating phase found for them (C++: one CRC of the whole image after the verify); a session retry repeats the current pass, and the retries of both passes count against `session_retries`; an erase failure reports the first address of the erased range (0x08004000 for sectors 1..n, C++ always 0x08000000). The phases Erasing, Writing and Verifying (legacy codes 3, 4, 5) appear twice; `bytes_done` counts over both passes, and the percent holds at the value of the first verify during the erase and the writing of sector 0 (88 % for a 53,760-byte image) until the second verify passes it. Images of at most 16 KiB are flashed exactly as in C++. | Sector 0 keeps the old vector table and boot stage until the rest of the new image is written and verified: an interrupted flash (ESP crash, power loss) leaves the STM without a bootable vector table only during the sector-0 pass (about 2 s) instead of during the erase and write of the whole image. An old image whose boot stage lies in sector 0 (the Rust STM images) still answers the handshake after an interruption in the first pass, so the ESP can flash again. Pinned by `stm_flasher/tests_d9.rs`. |
