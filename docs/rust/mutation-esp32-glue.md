# Mutation testing: esp32 glue (Rust)

Scope: `software_esp32_rust/glue`, crate `vdm-esp-glue`: the logic of
`software_esp32_revamped/src` and of the Arduino libraries it used, over the port traits; 23 files
with mutants. The firmware crate is outside the gate (docs/rust/PORTING.md).
Target: >= 95 % overall and >= 95 % for every file, as for the C++ glue suite
([mutation-esp32-glue.md](../revamped/mutation-esp32-glue.md): 99.71 %, 3817 / 3828).
Result: **100.00 % overall (3313 / 3313 killed), every file 100.00 %, gate passed.** 3541
mutants, 7 equivalent, 221 unviable.
Measured on 2026-10-08 with cargo-mutants 27.1.0, 4 jobs, in the runs listed under Runs (2 h
41 min); the sources of every file are those of `fb217d2`.

Tool: `tools/rust/docker.sh mutate` runs cargo-mutants on the package and then the per-file gate
`tools/rust/mutation_gate.py`. Every module is mutated against its own tests: `--file
glue/src/<m>.rs` with the libtest filter `<m>::`; a submodule file runs with the filter of its
module (`glue/src/storage/images.rs`: `storage::`, `glue/src/web_server/views.rs`:
`web_server::`), `ota/update.rs` and `app/wiring.rs` with their own (`ota::update::`,
`app::wiring::`). Modules with fast tests share a run, and their mutants run the tests of every
module of the run (see Runs). `port.rs` (the port traits, their types and the forwarding impls
for `&T`) has no tests of its own: its mutants run the whole glue suite. A mutant is built in the
test profile (overflow checks on); a failed test, a panic or a timeout (cargo-mutants' automatic
limit, 20 s in every run) kills it.
Not counted: unviable mutants (they do not compile, the C++ "stillborn") and equivalent
mutants, which are listed with a reason in `tools/rust/mutation/equivalents/vdm-esp-glue/`
(`mqtt_client.json`, `web_server.json`). Files without mutants: `glue/src/board.rs` (constants),
`glue/src/web_server/assets.rs` (the generated dashboard table) and `lib.rs`. `testkit/` and the
`tests*.rs`, `rig.rs` and `support.rs` modules are `#[cfg(test)]` code, which cargo-mutants does
not mutate.

A mutant that the tests of its module kill also fails the whole suite. A package-wide run, where
every mutant runs all 996 glue tests, was not made: by the times of these runs it would take
about 3.5 h and hold the mutation lock of all checkouts that long.

## Reproduce

```sh
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-glue --file glue/src/<m>.rs -- --lib -- <m>::
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-glue --file glue/src/storage.rs --file glue/src/storage/config_files.rs --file glue/src/storage/files.rs --file glue/src/storage/images.rs -- --lib -- storage::
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-glue --file glue/src/port.rs -- --lib
```

`--lib` leaves out the doc-test pass.

## Per file

| file | score | killed / counted | timeout | equivalent | unviable | run |
|---|---|---|---|---|---|---|
| `glue/src/app.rs` | 100.00 % | 48 / 48 | 1 | 0 | 11 | app |
| `glue/src/app/wiring.rs` | 100.00 % | 200 / 200 | 2 | 0 | 52 | wiring |
| `glue/src/boot_guard.rs` | 100.00 % | 119 / 119 | 0 | 0 | 13 | boot_guard |
| `glue/src/heap.rs` | 100.00 % | 7 / 7 | 0 | 0 | 10 | glue_a |
| `glue/src/http_parse.rs` | 100.00 % | 191 / 191 | 9 | 0 | 7 | glue_a |
| `glue/src/json_body.rs` | 100.00 % | 371 / 371 | 8 | 0 | 30 | glue_a |
| `glue/src/logger.rs` | 100.00 % | 60 / 60 | 7 | 0 | 8 | glue_b |
| `glue/src/mqtt_client.rs` | 100.00 % | 517 / 517 | 7 | 1 | 15 | mqtt_client |
| `glue/src/mqtt_conn.rs` | 100.00 % | 225 / 225 | 0 | 0 | 1 | glue_a |
| `glue/src/net.rs` | 100.00 % | 191 / 191 | 0 | 0 | 7 | glue_b |
| `glue/src/ota.rs` | 100.00 % | 99 / 99 | 0 | 0 | 5 | ota |
| `glue/src/ota/update.rs` | 100.00 % | 117 / 117 | 2 | 0 | 0 | glue_b |
| `glue/src/port.rs` | 100.00 % | 56 / 56 | 1 | 0 | 6 | port |
| `glue/src/shared.rs` | 100.00 % | 34 / 34 | 0 | 0 | 4 | glue_a |
| `glue/src/stm_link.rs` | 100.00 % | 42 / 42 | 0 | 0 | 10 | glue_b |
| `glue/src/stm_service.rs` | 100.00 % | 43 / 43 | 0 | 0 | 9 | glue_b |
| `glue/src/storage.rs` | 100.00 % | 155 / 155 | 0 | 0 | 11 | glue_a |
| `glue/src/storage/config_files.rs` | 100.00 % | 43 / 43 | 0 | 0 | 0 | glue_a |
| `glue/src/storage/files.rs` | 100.00 % | 52 / 52 | 3 | 0 | 7 | glue_a |
| `glue/src/storage/images.rs` | 100.00 % | 150 / 150 | 0 | 0 | 11 | glue_a |
| `glue/src/web_server.rs` | 100.00 % | 410 / 410 | 1 | 0 | 4 | web_server |
| `glue/src/web_server/uploads.rs` | 100.00 % | 32 / 32 | 0 | 0 | 0 | glue_b |
| `glue/src/web_server/views.rs` | 100.00 % | 151 / 151 | 0 | 6 | 0 | views2 |
| **total** | **100.00 %** | 3313 / 3313 | 41 | 7 | 221 | |

3541 mutants generated, 7 equivalent, 221 unviable.

## Runs

| run | files (test filters) | tree | date | mutants | time |
|---|---|---|---|---|---|
| boot_guard | boot_guard (`boot_guard::`) | `f0342a6` | 2026-10-08 | 132 | 4 min |
| ota | ota (`ota::`) | `f0342a6` | 2026-10-08 | 104 | 3 min |
| app | app (`app::`, which also selects the wiring cases) | `f0342a6` | 2026-10-08 | 59 | 3 min |
| wiring | app/wiring (`app::wiring::`) | `f0342a6` | 2026-10-08 | 252 | 9 min |
| web_server | web_server (`web_server::`) | `d9e032f` | 2026-10-08 | 414 | 19 min |
| mqtt_client | mqtt_client (`mqtt_client::`) | `9de3760` | 2026-10-08 | 533 | 25 min |
| glue_a | json_body, storage, storage/config_files, storage/files, storage/images, mqtt_conn, http_parse, heap, shared in one run (`json_body:: storage:: mqtt_conn:: http_parse:: heap:: shared::`) | `9de3760` | 2026-10-08 | 1309 | 65 min |
| glue_b | net, ota/update, logger, stm_link, stm_service, web_server/views, web_server/uploads in one run (`net:: ota::update:: logger:: stm_link:: stm_service:: web_server::`) | `9de3760` | 2026-10-08 | 676 | 24 min |
| port | port (no filter: the whole glue suite) | `9de3760` | 2026-10-08 | 62 | 3 min |
| views2 | web_server/views (`web_server::`) | `fb217d2` | 2026-10-08 | 157 | 6 min |

Each file's numbers come from its last run. No source file of the glue changed between
`f0342a6` and `fb217d2`; two commits changed only tests: `d9e032f` (the fake-time driver of the
wiring cases asserts progress, see Timeouts) and `fb217d2` (a status case and
`FakeWall::bare_conversion`, see Survivors). The web_server and views2 runs used those commits
before they were pushed, with the same workspace. The runs on `f0342a6` and `9de3760` lack the
case of `fb217d2`, which only adds a kill (views.rs ran again); app and wiring lack the progress
check of `d9e032f`, which turns their three timeouts into failures.

## Equivalent mutants

| file | function | mutation | matched | reason |
|---|---|---|---|---|
| `glue/src/mqtt_client.rs` | `MqttClient<'a, C, F, N, G, R, W, H>::service_full_publish` | `replace < with <= in MqttClient<'a, C, F, N, G, R, W, H>::service_full_publish` | 1 | full_cursor < SLOT_COUNT is never decided with budget left: a full publish always starts at slot 0, slot 0 and valve slot 1 fill the first pass, each valve slot 2..12 ends its own pass and the 44 slots 13..56 take 11 passes of 4, so the budget is 0 exactly when the cursor reaches 57 and budget > 0 ends the loop first (C++ the same loop) |
| `glue/src/web_server/views.rs` | `(const)` | `replace < with <=` | 1 | const EVENTS_BUFFER = if MAX_EVENTS_PER_RESPONSE < EVENT_CAPACITY { MAX_EVENTS_PER_RESPONSE } else { EVENT_CAPACITY }: < and <= differ only when the two are equal, where both branches give the same number |
| `glue/src/web_server/views.rs` | `Web<'a, P, N, F, G, S, O, H>::document` | `replace \|\| with && in Web<'a, P, N, F, G, S, O, H>::document` | 1 | every build closure ends with JsonWriter::ok() after closing its root (the core writers have no early return, the image list closes its array), and complete() is ok() with every container closed: build is false exactly when the writer is not complete, so !build \|\| !complete and !build && !complete agree |
| `glue/src/web_server/views.rs` | `Web<'a, P, N, F, G, S, O, H>::status_config` | `replace > with >= in Web<'a, P, N, F, G, S, O, H>::status_config` | 1 | write_status_json writes calibration.next only for next_calib_epoch > 0, so the local time converted for an epoch of 0 is never shown |
| `glue/src/web_server/views.rs` | `Web<'a, P, N, F, G, S, O, H>::status_config` | `delete field epoch from struct LocalTime expression in Web<'a, P, N, F, G, S, O, H>::status_config` | 1 | write_status_json reads only valid and the date and time fields of next_calib_local (calibration.next, shown for next_calib_epoch > 0), never its epoch, so the epoch the struct gets is never shown; the field keeps the C++ l.epoch = ci.nextEpoch |
| `glue/src/web_server/views.rs` | `Web<'a, P, N, F, G, S, O, H>::events` | `replace > with >= in Web<'a, P, N, F, G, S, O, H>::events` | 1 | max reaches 0 only after the document of one event did not fit the 12 KB response buffer; one event is far below it (25 events of the longest escaped text fit, test events_a_count_that_does_not_fit_is_halved_until_it_does), so the loop answers before max is 0 |
| `glue/src/web_server/views.rs` | `Web<'a, P, N, F, G, S, O, H>::events_fit` | `replace \|\| with && in Web<'a, P, N, F, G, S, O, H>::events_fit` | 1 | write_events_json ends with JsonWriter::ok() after closing its root, so its result is false exactly when the writer is not complete: both forms of the check agree |

The gate matches an entry by file, function and mutation name, without line and column
([mutation-stm32.md](mutation-stm32.md), Name matching): an entry covers every mutant of that name
in its function, or for a constant (function `""`) in the file's constants. Here each entry
matches one mutant, and each of the seven survives without its entry (they are the MISSED lines
of the mqtt_client and views2 runs). A change of mqtt_client.rs or views.rs needs its entries
checked by hand.

## Survivors found by the runs

| file | mutant | found by | killed by |
|---|---|---|---|
| `glue/src/web_server/views.rs` | `delete field valid from struct LocalTime expression in Web<'a, P, N, F, G, S, O, H>::status_config` (the conversion of the fake wall clock already said valid; C++ set nextCalibLocal.valid itself after localtime_r) | glue_b | `status_the_next_calibration_is_valid_with_a_bare_conversion` with `FakeWall::bare_conversion`, the broken-down time only, as the ScriptedWall of the net cases (`fb217d2`) |
| `glue/src/web_server/views.rs` | `delete field epoch from struct LocalTime expression in Web<'a, P, N, F, G, S, O, H>::status_config` | glue_b | none possible: listed as equivalent (`fb217d2`) |

Before these runs, the porting work had killed the survivors of its own gate runs with cases and
listed the six other equivalents.

## Timeouts

41 counted mutants were killed by a timeout: mutants that stop the progress a loop waits for (the
parsers of the query string and the JSON body, the reader of the discovery list, the inbound
overflow count, the subscriptions, the log file backlog and its rotation, the OTA writer, the
legacy image cleanup, the waits on the fake clock and the fake-time driver of the wiring cases).
Every one ran again alone, with the mutation applied by hand to a copy of the workspace (the same
sources), against the tests of its module (port.rs: the whole suite), stopped after 75 s. "N hung"
means that N cases were still running after 60 s (libtest's warning) when the run stopped; libtest
runs six cases at a time on the six CPUs of the shared Docker host, so 6 means that every thread
hung. None passes when given the time:

| mutant | mutation | run | alone |
|---|---|---|---|
| `glue/src/app.rs:620` | `replace <impl Task<P::Watchdog> for App<'a, P, H>>::pass -> u32 with 0` | app | failed in 1 s with the check of `d9e032f` ("no fake time passed") |
| `glue/src/app/wiring.rs:949` | `replace <impl Task<P::Watchdog> for StmLink<'a, P, H>>::pass -> u32 with 0` | wiring | no end in 60 s before `d9e032f` (run_ms spun without moving the fake time); failed in 0 s with its check |
| `glue/src/app/wiring.rs:967` | `replace <impl Task<W> for MqttClient<'a, C, F, N, G, R, W, H>>::pass -> u32 with 0` | wiring | failed in 0 s with the check of `d9e032f` |
| `glue/src/web_server.rs:150` | `replace += with *= in parse_epoch` | web_server | 1 hung (`a_build_epoch_is_read_from_its_decimal_text`) |
| `glue/src/mqtt_client.rs:777` | `replace <impl DiscoveryPort for ListPort<'_, '_, C, F, N, W, H>>::list_read -> Option<u8> with Some(0)` | mqtt_client | 6 hung |
| `glue/src/mqtt_client.rs:777` | `replace <impl DiscoveryPort for ListPort<'_, '_, C, F, N, W, H>>::list_read -> Option<u8> with Some(1)` | mqtt_client | 6 hung |
| `glue/src/mqtt_client.rs:782` | `replace >= with < in <impl DiscoveryPort for ListPort<'_, '_, C, F, N, W, H>>::list_read` | mqtt_client | 2 hung |
| `glue/src/mqtt_client.rs:786` | `replace += with *= in <impl DiscoveryPort for ListPort<'_, '_, C, F, N, W, H>>::list_read` | mqtt_client | 6 hung |
| `glue/src/mqtt_client.rs:1511` | `replace -= with += in MqttClient<'a, C, F, N, G, R, W, H>::drain_inbound` | mqtt_client | 2 hung |
| `glue/src/mqtt_client.rs:1511` | `replace -= with /= in MqttClient<'a, C, F, N, G, R, W, H>::drain_inbound` | mqtt_client | 2 hung |
| `glue/src/mqtt_client.rs:2290` | `replace += with *= in MqttClient<'a, C, F, N, G, R, W, H>::subscribe` | mqtt_client | 6 hung |
| `glue/src/http_parse.rs:116` | `delete ! in find_param` | glue_a | 1 hung |
| `glue/src/http_parse.rs:129` | `replace split_once -> Option<(&[u8], &[u8])> with Some((Vec::leak(Vec::new()), Vec::leak(vec![0])))` | glue_a | ended after 44 s without a summary (see below) |
| `glue/src/http_parse.rs:129` | `replace split_once -> Option<(&[u8], &[u8])> with Some((Vec::leak(Vec::new()), Vec::leak(vec![1])))` | glue_a | ended after 36 s without a summary |
| `glue/src/http_parse.rs:129` | `replace split_once -> Option<(&[u8], &[u8])> with Some((Vec::leak(vec![0]), Vec::leak(vec![0])))` | glue_a | 5 hung |
| `glue/src/http_parse.rs:129` | `replace split_once -> Option<(&[u8], &[u8])> with Some((Vec::leak(vec![0]), Vec::leak(vec![1])))` | glue_a | ended after 48 s without a summary |
| `glue/src/http_parse.rs:129` | `replace split_once -> Option<(&[u8], &[u8])> with Some((Vec::leak(vec![1]), Vec::leak(vec![0])))` | glue_a | ended after 40 s without a summary |
| `glue/src/http_parse.rs:129` | `replace split_once -> Option<(&[u8], &[u8])> with Some((Vec::leak(vec![1]), Vec::leak(vec![1])))` | glue_a | 5 over 60 s, ended after 74 s without a summary |
| `glue/src/http_parse.rs:141` | `replace <impl Iterator for UrlDecoded<'_>>::next -> Option<u8> with Some(0)` | glue_a | 5 hung |
| `glue/src/http_parse.rs:141` | `replace <impl Iterator for UrlDecoded<'_>>::next -> Option<u8> with Some(1)` | glue_a | 5 hung |
| `glue/src/json_body.rs:690` | `replace += with *= in Parser<'_>::current` | glue_a | 2 hung |
| `glue/src/json_body.rs:702` | `replace Parser<'_>::advance with ()` | glue_a | 2 hung |
| `glue/src/json_body.rs:1042` | `replace can_be_in_non_quoted_string -> bool with true` | glue_a | 6 hung |
| `glue/src/json_body.rs:1097` | `replace NumberText<'_>::digit -> Option<u64> with Some(0)` | glue_a | 6 hung |
| `glue/src/json_body.rs:1117` | `replace += with *= in NumberText<'_>::integer` | glue_a | 6 hung |
| `glue/src/json_body.rs:1133` | `replace += with *= in NumberText<'_>::decimal` | glue_a | 4 hung |
| `glue/src/json_body.rs:1142` | `replace += with *= in NumberText<'_>::decimal` | glue_a | 6 hung |
| `glue/src/json_body.rs:1162` | `replace += with *= in NumberText<'_>::exponent` | glue_a | 2 hung |
| `glue/src/storage/files.rs:92` | `replace LegacyBatch::entries -> impl Iterator<Item =(&[u8], u32)> with ::std::iter::empty()` | glue_a | 3 hung |
| `glue/src/storage/files.rs:236` | `replace Storage<'_, N, F, G, H>::remove_legacy_batch -> bool with true` | glue_a | 6 hung |
| `glue/src/storage/files.rs:238` | `replace == with != in Storage<'_, N, F, G, H>::remove_legacy_batch` | glue_a | 6 hung |
| `glue/src/logger.rs:52` | `replace * with /` (`LOG_FILE_MAX = 64 * 1024`) | glue_b | 6 hung |
| `glue/src/logger.rs:173` | `replace LoggerShared::read_since -> usize with 1` | glue_b | 6 hung |
| `glue/src/logger.rs:174` | `delete field since_seq from struct EventFilter expression in LoggerShared::read_since` | glue_b | 6 hung |
| `glue/src/logger.rs:478` | `delete ! in LogSinks<'a, C, W, K, F, U, H>::append_line` | glue_b | 1 hung |
| `glue/src/logger.rs:522` | `replace != with == in LogSinks<'a, C, W, K, F, U, H>::write_batch` | glue_b | 2 hung |
| `glue/src/logger.rs:531` | `replace != with == in LogSinks<'a, C, W, K, F, U, H>::write_batch` | glue_b | 6 hung |
| `glue/src/logger.rs:553` | `replace && with \|\| in LogSinks<'a, C, W, K, F, U, H>::write_backlog` | glue_b | 2 hung |
| `glue/src/ota/update.rs:196` | `replace Update<O, M, G>::write_buffer -> bool with true` | glue_b | 6 hung |
| `glue/src/ota/update.rs:235` | `replace += with *= in Update<O, M, G>::write` | glue_b | 5 hung |
| `glue/src/port.rs:578` | `replace <impl Clock for &T>::sleep_ms with ()` | port | 6 hung |

A constant `split_once` keeps the loop of find_param going and leaks a new vec in every round:
five of its six mutants ended the run before 75 s without the summary of the test binary. With
the address space of the test binary capped at 2 GB, the first one aborts after 23 s with "memory
allocation of 1 bytes failed". In the gate runs the 20 s limit came first.

The three `pass -> 0` mutants hung the fake-time driver `run_ms` of the wiring cases, which took
the delay of each task without checking it; since `d9e032f` it asserts that every round moves the
fake time, so they fail at once.

## Surviving mutants

None.
