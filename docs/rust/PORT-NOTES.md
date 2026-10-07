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

| mqtt_topics | `formatVolt` turns the first '.' into ',' with `strchr(out, '.')` also when `putf` wrote nothing: with capacity 0 and `germanComma` it scans, and may change, memory the caller did not hand over. | A C++ bug that cannot happen in the firmware (the MQTT client formats into 24 bytes). Rust changes only the text it wrote. |
| mqtt_topics | Rust differs: `formatVolt` prints `%.3f` through `format_f64_fixed`, which is exact below 2^128 only. For \|v\| >= 2^128 (39 digits and more) Rust returns 0 where the C++ prints the number into an output of 44 bytes or more. | Not reachable: volt values are (vad / 100 + offset) x factor with \|offset\|, \|factor\| <= 1000 (below 2.2e10), formatted into 24 bytes, where both return 0 from 1e19 on. The differential check of the three MQTT/target modules (512,048 cases) found 761 differences, all of this kind, and no other. |
| mqtt_policy | `ButtonGate::confirm()` and `expire()` take the held action in the lowest slot; the header says "the oldest held action of that topic", which a reused slot breaks (hold A, hold B, confirm, hold C: the next confirm gives C before B). | Not observable: the actions held for one topic are the same button. Kept (test `button_gate_confirm_takes_the_lowest_slot_also_after_a_slot_was_reused`). |

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


## stm_codec

| Kind | What | Why it matters |
|---|---|---|
| Kept quirk | gvlvy checks its failsafe position (field 22) only after reading the six v3 fields, glcfg checks each failsafe position as soon as it is read. A gvlvy line with fs_pct 101 and a malformed later field reports `bad_number`; the same mix in glcfg reports `out_of_range`. | Status only: both lines are rejected and counted as parse errors. Kept; the differential check compares the status of 460,206 lines. |
| No Rust form | Builder tests that fill the output with `garbage()` first and check that a build overwrites, or a refusal resets, every member; `buildServiceMove(.., static_cast<MoveDir>(2), ..)`; `cmdName`, `cmdMinProtocol`, `cmdIsV2`, `cmdIsIdempotent` of numbers >= 41, `parseStatusName(99)`, `stopReasonName(8)`; `parseReply(nullptr, ..)`, `cmdFromName(nullptr, ..)`, `resolveTempSlot(id, nullptr, ..)`. | A builder returns a fresh line or None (`unwrap_or_default()` is the C++ reset line); the enums cannot hold the numbers (`from_raw` refuses them); a slice is never null. `resolve_temp_slot` takes a slice for the C++ `(slotIds, uint8_t slotCount)` and looks at its first 255 slots. |

## link_policy

| Kind | What | Why it matters |
|---|---|---|
| No Rust form | `enqueue()` of a request with `len > kRequestMaxLen`, a command number >= 41 or priority 3 (Invalid); `linkStateName(42)`; the C++ stale entry past `count_` that the "nothing stale" case watches. | `Text<63>`, `Cmd` and `Priority` cannot hold the values; the Rust queue is a `heapless::Vec` without stale slots (the case runs and passes). |

## poll_planner

| Kind | What | Why it matters |
|---|---|---|
| No Rust form | The idle `next()` call that resets the caller's `RequestLine` (gstat before) to an empty line. | `next()` returns `Option<RequestLine>`: None carries no line. |


## event_log

| Kind | What | Why it matters |
|---|---|---|
| Intended deviation | Reboot reason 7 `RebootReason::SwitchBack` (`POST /api/system/ota/switch-back`, GLUE-DESIGN-ESP §7 item 6): `reboot_requested` with arg1 7 reads "restart requested (switch back)" and keeps the default severity Info (the Warning reasons stay 2, 4, 5, 6); C++ 2.1.7 reads it "restart requested (unknown)". 8 and above stay "unknown". Event 107 `esp_ota_failed` arg1 -4 (boot guard of the Rust firmware): the previous image failed its trial, arg2 1 boot limit, 2 health; its message stays the generic "ESP update failed (error -4)". | Values appended only (DESIGN.md §13 follows; the C++ firmware gets them in 2.1.8), so the log, `/api/log` and MQTT read the same from either firmware. Pinned by `switch_back_restart_and_trial_failure_are_the_glue_design_contract` and the message table. Differential check: identical with the C++ module plus the reason-7 name; against 2.1.7 only the outputs of reason-7 events differ (150 of 2,189,806 lines, all from the 32 such events). |
| Kept quirk | Messages keep their C++ wording where it reads oddly: "recovered ()" for a ValveRecovered without a previous status or a health bit, "1 events lost", "1 fields", "1 repairs", "1 attempts", and "temp" for every SensorCountChanged kind other than 1. | The messages are external API (log file, `/api/log`, MQTT `msg`); the message table pins every one, and the differential check compares 800,000 lines, messages, JSON and MQTT documents, syslog packets and timestamps of random events (truncating buffers included) byte for byte. |
| No Rust form | `static_cast<EventCode>(0)`/`(999)` and `static_cast<Severity>(5)`/`(7)`/`(9)`: the "unknown" names, "event 999", "UNKNOWN" lines, syslog severity 7 and MQTT class No; a 24-byte text without its NUL (`memset(e.text, ..)`); `EventLog(nullptr, 10)`; `parseSeverity(nullptr, 4, ..)`, `eventMqttNames(nullptr, 5)`, `read(f, nullptr, 5, ..)`, `makeEvent(.., nullptr)` and the null output buffers. | `from_raw(v) == None`; a full 23-byte text (a `Text<23>` holds no more; a NUL inside ends it as in C++, tested); `EventLog<0>`; the empty slice. |

## log_sink

| Kind | What | Why it matters |
|---|---|---|
| Kept quirk | `formatLogGapLine` counts `to - from + 1` in uint32: a gap with `to` below `from` prints the wrapped count ("#5-4 gap: 0 events not written"); `detectLogGap` after cursor 2^32 - 1 starts the gap at seq 0; `logFileStep` adds in size_t. | Not reachable (`detect_log_gap` gives from <= to; 2^32 events); kept with `wrapping_*`, tested and compared. |
| No Rust form | `formatSyslog(e, nullptr, nullptr, ..)`, `formatSyslog(.., nullptr, 10)`, `formatLogGapLine(g, nullptr, 10)`. | The empty C strings and the empty output. |

## sys_health

| Kind | What | Why it matters |
|---|---|---|
| Kept quirk | `onHeap` hands the free heap and the largest block to the event args as int32: figures of 2^31 and more turn negative. | Not reachable (263 KB of heap); kept (`as i32`), tested. |
| No Rust form | `onHeap(.., nullptr, 2)`, `onStack(.., nullptr, 1)`, `httpStatusOk(nullptr, 12)`. | The empty output and the empty line. |

| mqtt_topics, mqtt_policy, target_store | Null pointers: a null topic, segment, HA prefix, payload or record (`parseInboundTopic(.., nullptr, 10, ..)`, `buildTopic`/`buildSegment` with a null segment or name, `buildHaStatusTopic(nullptr, ..)`, `parseTargetPayload(nullptr, 3, ..)`, `parseButtonPayload(nullptr, 5)`, `decideInbound`/`inboundIsClearEcho(.., nullptr, 5)`, `onHaStatus(nullptr, 6)`, `ButtonGate::hold`/`confirm(nullptr, ..)`, `decodeTargets(nullptr, 46, ..)`) and the null output of every builder and formatter. | A slice is never null; each case is named in a comment where it stood. Where the C++ treats null like the empty input (segment, name, HA prefix, payload, `chooseTargets(nullptr, 0, nullptr, 0, ..)`), the empty slice is tested; a null segment table, topic context or echo filter is `None`. |
| mqtt_topics, mqtt_policy | Enum values out of range: `static_cast<Topic>(kTopicCount)` (and 200) in `buildTopic`, `topicRetained`, `topicIsCompat`; `static_cast<HaStatus>(3)` in `RegulatorWatch::restore` and `encodeHaStatusRecord`; `static_cast<RejectReason>(99)`. | `from_raw(v) == None` is tested; the HA status record with status 3 and a matching CRC is built by hand and decodes to Unknown. |
| mqtt_topics | A station array without its NUL (21 x 'x') and segment table entries of 11 'q'. | A `Text<20>`/`Text<10>` holds at most 20/10 bytes; the full texts and texts with a NUL inside are tested instead. |


## config

| Kind | What | Why it matters |
|---|---|---|
| C++ bug, kept | A `cfgx` record with tag 0 is applied to `station`: `findExt(0)` finds the first field without an ext tag instead of no field. | Harmless: no writer emits tag 0, so only a damaged blob with a valid CRC reaches it, and the value still passes the station rule (a text with a NUL inside is refused before that rule, as in C++). Kept for parity (tests `a_cfgx_record_with_tag_0_sets_the_first_base_field`, `a_cfgx_text_with_a_nul_inside_is_bad_also_for_the_station`). |
| Port form | A stored `net.iface` or `mqtt.mode` byte out of range: the C++ enum holds the raw value until `sanitizeConfig` resets it. A Rust enum cannot hold it, so the decoder reports the field and the repair resets it like any other bad field. | Same `DecodeInfo` (mask, count, first path, field value) as the C++ (test `decode_repairs_an_enum_byte_out_of_range`; the differential check flips blob bytes). |
| No Rust form | Null pointers (`setConfigValue(c, nullptr, ..)`, a String value with a null pointer, `applyConfigJson(c, nullptr, ..)`, `encodeConfig(.., nullptr, ..)`, `encodeConfigExt(.., nullptr, ..)`, `decodeConfigExt(nullptr, ..)`, `crc32(nullptr, ..)`, `itemSegment(.., nullptr, 8)`). | Named in comments; where null means "empty" in C++ (`validateConfig(c, nullptr, 0)`, `decodeConfig(nullptr, ..)`), the empty slice is tested. |
| No Rust form | Values a Rust member cannot hold: `net.iface`/`mqtt.mode` 3 set in RAM (validation, sanitize), a bool byte 2 (`net.dhcp`, `persistLog`), text arrays without their NUL (`station`, a valve name and topic: validation, export, sanitize and "an unterminated string is encoded at most cap-1 bytes"), bytes after a text's NUL (`netTrialRequired`, `mqttTopicConfigChanged`), `ItemKind` 3, the names of `SetResult`/`PatchResult` 99. | The enum bytes go through the decoder (above); the full-length texts are tested instead of the unterminated ones; `from_raw(v) == None` for the names. The sanitize fuzz still draws iface `rng() % 4`; a 3 leaves the iface unchanged. |
| Test order | `decode fuzz` writes `b[8 + rng() % n] = rng()`: C++17 evaluates the right operand first. | The Rust test draws the value before the index, so it walks the C++ inputs. |

## net_trial

| Kind | What | Why it matters |
|---|---|---|
| Kept quirk | `decodeNetTrial` copies the stored SSID and password bytes into the char arrays, so a NUL inside a stored text ends it there; the fields CRC of the decoded record then covers the shorter text. | The Rust decoder copies up to the first NUL (`copy_string`). The first port kept the bytes after the NUL; the differential check found it (test `decode_a_nul_inside_a_stored_text_ends_it`). |
| No Rust form | Null pointers (`encodeNetTrial(r, nullptr, ..)`, `decodeNetTrial(nullptr, ..)`, `formatNetAddress(.., nullptr, ..)`) and the damaged struct of "an ssid or password without its terminator ends at the field size". | The full-length SSID and password (32 + 64 bytes, a 130-byte record) are tested instead. |

## legacy_import

| Kind | What | Why it matters |
|---|---|---|
| Port form | `LegacyNvsReader::readString(ns, key, out, cap, truncated)` becomes `read_string(ns, key, out: &mut TextView) -> Option<bool>` (the text in `out`, at most its capacity = the C++ cap - 1; true when truncated); `readInt`/`readBlob` return `Option<i64>`/`Option<usize>` (the stored length). | The importer reads the strings into a `Text<65>` (the C++ 66-byte buffer) and treats them as C strings, so the same keys are rejected; the NVS reads are the same calls in the same order (the differential check compares the read counts). |
| No Rust form | `importLegacyConfig(n, c, nullptr, 2 * kLegacyTempsBlob)` (a null scratch with a capacity). | The empty scratch slice: the temps blob is not read, as with a null pointer. |
| Test order | The fuzz test draws `std::string(rng() % 80, rng() % 256)`, `setValve(.., pool[rng() % 10], rng() % 3)` and `setTemp(..)` with several `rng()` calls in one argument list; GCC 13.3 (the test toolchain, -O0 and -O2) evaluates them right to left. | The Rust test draws in that order, so it walks the C++ inputs. |
