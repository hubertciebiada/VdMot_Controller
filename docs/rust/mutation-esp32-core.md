# Mutation testing: esp32 core (Rust)

Scope: `software_esp32_rust/core`, crate `vdm-esp-core`: the port of
`software_esp32_revamped/lib/core`, 37 modules.
Target: >= 95 % overall and >= 95 % for every file, as for the C++ suites
([mutation-esp32.md](../revamped/mutation-esp32.md)).
Result: **100.00 % overall (6433 / 6433 killed), every file 100.00 %, gate passed.**
Measured on `f0342a6` (2026-10-07 and 08; the core is the same up to `d9e032f`),
cargo-mutants 27.1.0, 4 jobs, 8 h 1 min of runs.

Tool: `tools/rust/docker.sh mutate` runs cargo-mutants on the package and then the per-file gate
`tools/rust/mutation_gate.py`. Every module is mutated against its own tests: `--file
core/src/<m>.rs` with the libtest filter `<m>::`, like the C++ per-file test binaries. Modules
with fast tests share a run, and their mutants run the tests of every module of the run (see
Runs). A mutant is built in the test profile (overflow checks on); a failed test, a panic or a
timeout (cargo-mutants' automatic limit, 20 s, 22 s for stm_session) kills it. The timeouts of
stm_session and stm_flasher were run again alone: each failed a test (on the loaded shared host
the run had it past the limit) or hung. The other timeouts (config, poll_planner, ha_discovery)
were not run again; they are mutants in loops and step functions (the JSON cursor, the config
repairs repeated until stable, the re-sync step skip, the discovery iterators).
Not counted: unviable mutants (they do not compile, the C++ "stillborn") and equivalent
mutants, which are listed with a reason in `tools/rust/mutation/equivalents/vdm-esp-core/`.

A mutant that the tests of its module kill also fails the whole suite. A package-wide run, where
every mutant runs all tests of the crate, takes about 20 h on the shared host and holds the
mutation lock of all checkouts that long; it was not made.

## Reproduce

```sh
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-core --file core/src/<m>.rs -- --lib -- <m>::
bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-core --file core/src/a.rs --file core/src/b.rs -- --lib -- a:: b::
```

`--lib` leaves out the doc-test pass.

## Per file

| file | score | killed / counted | timeout | equivalent | unviable | run |
|---|---|---|---|---|---|---|
| `core/src/calib_schedule.rs` | 100.00 % | 367 / 367 | 0 | 0 | 2 | calib_legacy |
| `core/src/common.rs` | 100.00 % | 295 / 295 | 0 | 0 | 9 | f1 |
| `core/src/config.rs` | 100.00 % | 825 / 825 | 9 | 0 | 66 | config |
| `core/src/event_limiter.rs` | 100.00 % | 70 / 70 | 0 | 0 | 1 | f3 |
| `core/src/event_log.rs` | 100.00 % | 317 / 317 | 0 | 0 | 13 | event_log2 |
| `core/src/factory_reset.rs` | 100.00 % | 9 / 9 | 0 | 0 | 2 | f1 |
| `core/src/failsafe.rs` | 100.00 % | 50 / 50 | 0 | 0 | 0 | f1 |
| `core/src/file_manager.rs` | 100.00 % | 42 / 42 | 0 | 0 | 2 | fast |
| `core/src/ha_discovery.rs` | 100.00 % | 344 / 344 | 3 | 0 | 50 | ha2 |
| `core/src/health_monitor.rs` | 100.00 % | 137 / 137 | 0 | 0 | 1 | fast |
| `core/src/image_store.rs` | 100.00 % | 19 / 19 | 0 | 0 | 0 | f1 |
| `core/src/json_api.rs` | 100.00 % | 133 / 133 | 0 | 0 | 2 | fast |
| `core/src/json_writer.rs` | 100.00 % | 118 / 118 | 0 | 0 | 6 | f1 |
| `core/src/lease_client.rs` | 100.00 % | 193 / 193 | 0 | 0 | 1 | fast |
| `core/src/legacy_http.rs` | 100.00 % | 29 / 29 | 0 | 0 | 0 | fast |
| `core/src/legacy_import.rs` | 100.00 % | 231 / 231 | 0 | 0 | 3 | calib_legacy |
| `core/src/line_assembler.rs` | 100.00 % | 31 / 31 | 0 | 0 | 3 | f1 |
| `core/src/link_policy.rs` | 100.00 % | 182 / 182 | 0 | 0 | 4 | f2 |
| `core/src/log_sink.rs` | 100.00 % | 58 / 58 | 0 | 0 | 1 | f3 |
| `core/src/mqtt_policy.rs` | 100.00 % | 140 / 140 | 0 | 0 | 0 | mqtt_policy2 |
| `core/src/mqtt_topics.rs` | 100.00 % | 171 / 171 | 0 | 0 | 14 | f3 |
| `core/src/mqtt_values.rs` | 100.00 % | 143 / 143 | 0 | 0 | 19 | fast |
| `core/src/net_policy.rs` | 100.00 % | 89 / 89 | 0 | 0 | 2 | f1 |
| `core/src/net_trial.rs` | 100.00 % | 92 / 92 | 0 | 0 | 3 | fast |
| `core/src/ota_policy.rs` | 100.00 % | 53 / 53 | 0 | 0 | 2 | f1 |
| `core/src/poll_planner.rs` | 100.00 % | 255 / 255 | 4 | 0 | 7 | f2 |
| `core/src/reset_gate.rs` | 100.00 % | 23 / 23 | 0 | 0 | 0 | f1 |
| `core/src/restart_gate.rs` | 100.00 % | 9 / 9 | 0 | 0 | 1 | smoke |
| `core/src/stm_codec.rs` | 100.00 % | 346 / 346 | 0 | 0 | 26 | f2 |
| `core/src/stm_flasher.rs` | 100.00 % | 655 / 655 | 9 | 2 | 3 | flasher3 |
| `core/src/stm_session.rs` | 100.00 % | 275 / 275 | 13 | 0 | 12 | session2 |
| `core/src/stm_types.rs` | 100.00 % | 2 / 2 | 0 | 0 | 0 | smoke |
| `core/src/sys_health.rs` | 100.00 % | 56 / 56 | 0 | 0 | 0 | f3 |
| `core/src/target_store.rs` | 100.00 % | 47 / 47 | 0 | 0 | 7 | f3 |
| `core/src/valve_model.rs` | 100.00 % | 489 / 489 | 0 | 0 | 5 | fast |
| `core/src/version.rs` | 100.00 % | 62 / 62 | 0 | 0 | 0 | f1 |
| `core/src/web_guard.rs` | 100.00 % | 76 / 76 | 0 | 0 | 6 | fast |
| **total** | **100.00 %** | 6433 / 6433 | 38 | 2 | 273 | |

6708 mutants generated, 2 equivalent, 273 unviable.

## Runs

| run | modules (test filters) | mutants | time |
|---|---|---|---|
| smoke | stm_types, restart_gate | 12 | 1 min |
| fast | legacy_http, file_manager, web_guard, net_trial, json_api, health_monitor, mqtt_values, lease_client, valve_model | 1373 | 84 min |
| calib_legacy | calib_schedule, legacy_import | 603 | 41 min |
| config | config | 891 | 62 min |
| f1 | common, json_writer, line_assembler, version, image_store, failsafe, net_policy, ota_policy, reset_gate, factory_reset | 773 | 42 min |
| f2 | stm_codec, link_policy, poll_planner | 820 | 2 h |
| f3 | mqtt_topics, mqtt_policy, target_store, event_log, event_limiter, log_sink, sys_health | 894 | 36 min |
| session2 | stm_session | 287 | 27 min |
| flasher3 | stm_flasher | 660 | 35 min |
| event_log2 | event_log | 330 | 16 min |
| mqtt_policy2 | mqtt_policy | 140 | 5 min |
| ha2 | ha_discovery | 394 | 12 min |

Each file's numbers come from its last run. The sources and tests of every module are the same
in `f0342a6` as in the tree its run used (the earlier runs are older than the commits of the
safety review, which changed only stm_flasher, stm_session, event_log and mqtt_policy, run again
on `f0342a6`).

## Equivalent mutants

| file | function | mutation | reason |
|---|---|---|---|
| `core/src/stm_flasher.rs` | `check_header` | `replace < with <= in check_header` | pc < FLASH_BASE is evaluated only for an odd reset vector ((pc & 1) == 0 rejects first), and FLASH_BASE is even, so pc <= FLASH_BASE gives the same answer (C++ the same check order); the other < -> <= mutant of the function (size < 8) is killed and falls under this name |

## Survivors found by the module runs

| file | mutant | killed by |
|---|---|---|
| `core/src/ha_discovery.rs` | `delete match arm P::DropList in DiscoveryRun::advance`; `replace match guard p.prune with true in DiscoveryRun::advance` | `a_publish_plan_without_prune_goes_from_the_drop_list_to_publish` |
| `core/src/stm_flasher.rs` | `replace - with +` and `replace - with /` in `StmFlasher::step_handshake` (the five bytes kept of a failed BEEFIT search) | `handshake_keeps_five_bytes_of_a_failed_search` |
| `core/src/stm_flasher.rs` | `replace \|\| with && in StmFlasher::step_waiting_app` | `app_reads_stop_once_the_reply_ends_the_run` |
| `core/src/stm_session.rs` | `delete field force from struct FlashOptions expression in StmSession<P>::begin_flash` | `a_forced_flash_skips_the_image_checks` |
| `core/src/stm_session.rs` | `delete ! in StmSession<P>::apply_reply` (gonec and gowvc) | `a_count_reply_that_changes_the_count_asks_for_the_id_list` |
| `core/src/stm_session.rs` | `delete ! in StmSession<P>::schedule_config` | `an_assembly_push_that_finds_the_queue_full_is_retried_later` |

The first runs of ha_discovery and stm_session found their survivors; the stm_flasher ones were
found by hand before its first run. Before these runs, the porting work had added Rust cases
(marked "Rust addition" in the tests) for the mutants the ported C++ cases left alive.

## Surviving mutants

None.
