# VdMot Revamped in Rust: parity with C++ 2.1.7

Audit of the claim "the firmware fully rewritten in Rust, with tests, full functionality
preserved" for both firmwares (ESP32 and STM32), at commit `92718f5` of `revamped-rust` plus the
audit's own commits (section 4.1). Reference: the C++ 2.1.7 sources (`software_esp32_revamped`,
`software_stm32`) and their contract documents (`docs/revamped/API.md`, `docs/revamped/MQTT.md`,
`software_esp32_revamped/DESIGN.md`, `software_stm32/PROTOCOL_V2.md`).

## Verdict

- **Not proven on hardware.** Everything below is proven by host tests, the C++ goldens of the
  STM glue, the QEMU harness (ESP) and the Renode suites (STM). What only a device or a bench
  can prove is listed in section 4.3 (H1-H3); the first flash is the operator's.
- **Tests:** every one of the 2442 C++ test cases of the four native suites has its Rust test,
  or a documented reason why it has none: 2386 ported, 26 without a Rust form (each linked to its
  port note), 30 retired by the glue design (each named there), 0 missing. Nine cases were
  missing before this audit (the C++ smoke test and the harness case of the fake STM); they are
  ported now.
- **Features:** no externally visible feature of C++ 2.1.7 is missing in the Rust firmwares.
  Section 3 lists 197 features with the Rust code and the tests that cover them: 156 the same
  as C++, 37 with a documented deviation, 4 that only hardware proves. One undocumented
  deviation with an effect on the device was found and fixed (GET of a legacy alias with a body,
  `3aea221`); the others found are documented now (section 4.1).
- **Open:** 13 gaps, none a missing feature: the ESP version string against decision D8,
  automation (CI jobs, archived emulator runs, the feature-gated dashboard test), the drivers of
  the differential checks, an end-to-end test of the two Rust firmwares together, narrower
  emulator scenarios than the design claims, and the C++ contract documents that do not name
  the Rust-only additions (section 4.2, with estimates).

## 1. Method and evidence

| Evidence | What it proves | Run in this audit |
|---|---|---|
| Host tests: `bash tools/rust/docker.sh test software_esp32_rust` and `software_stm32_rust` | the Rust behaviour per module and per glue suite; clippy `-D warnings` and `cargo fmt --check` with them | yes, on every commit of the audit (section 5) |
| `cargo test -p vdm-esp-glue --features dashboard` | the shipped dashboard table (gzip, ETag, content types); not part of `docker.sh test` (gap O2) | yes, 3 tests passed |
| C++ goldens of the STM `glue_system` suites (`software_stm32_rust/glue/tests/golden/`, 27 cases) | UART bytes, EEPROM rows and no-init bytes per boot, byte for byte against the C++ glue (GLUE-DESIGN-STM.md 7.4) | yes, as part of the STM host tests |
| Differential checks quoted in PORT-NOTES.md (stm_codec, stm_session, config, event_log, the MQTT modules, net_trial, legacy_import) | random inputs through the C++ and the Rust module, outputs compared | no: their drivers are not in the repository (gap O4) |
| Mutation gate (`docker.sh mutate`, 95 % per file) | the tests kill the mutants of the code | no (one global lock, run by `main`); the files changed by the audit are listed in section 5 |
| QEMU harness (`tools/rust/esp/docker.sh qemu`, tools/rust/README.md) | the ESP image with the devices' bootloader and the C++ 2.1.7 image in the other slot: boot chain, boot guard, OTA from Rust to C++ (the C++ image has no network in QEMU), LittleFS/NVS interchange, HTTP, MQTT against Mosquitto, load | no; results of 2026-10-07 are quoted from GLUE-DESIGN-ESP.md 5.5, not archived (gap O7) |
| Renode (`tools/rust/renode.sh`: `boot.robot` E1-E10, `app.robot` A1-A6, `e11.robot` E11) and the image check (C1-C6, D9) | the four STM images: boot window, clocks, faults, watchdog, no-init cells, the glue against the C++ goldens, the Rust ESP flasher against the boot window | no; quoted from GLUE-DESIGN-STM.md 5.7 |

Feature evidence was collected by reading the C++ and the Rust code side by side; the tables of
section 3 name the Rust code and the tests, and every test named in this document exists (checked
by `tools/rust/parity/parity.py`, which also checks the listing). Paths are relative to
`software_esp32_rust` (prefix `esp:`) and `software_stm32_rust` (prefix `stm:`); a test is
`file::fn`.

## 2. Test parity

The cases of the C++ native suites (doctest `TEST_CASE`, their `SUBCASE`s counted with them) and
the Rust `#[test]` functions are paired by `tools/rust/parity/parity.py` along the convention of
the ports (PORTING.md "Modules and tests", GLUE-DESIGN-ESP.md 5.2, GLUE-DESIGN-STM.md 7.1):
`test_<m>.cpp` -> `<crate>/src/<m>/tests.rs`, `test_<m>__<p>.cpp` -> `tests_<p>.rs`, the case
name in snake_case without its module prefix. Pairs that the name alone does not give (a
shortened name, a Rust comment that quotes the C++ case, one Rust test per SUBCASE, the closest
name in the mapped file) were reviewed by hand; the pairs the script cannot find are rows of
`tools/rust/parity/manual.tsv`, each with its Rust tests or the documented reason. The script
fails on an unmatched case and on a manual row that names a test or a case that does not exist.

```
python tools/rust/parity/parity.py              # the counts per file; exit 1 on a gap
python tools/rust/parity/parity.py --unmatched  # the cases without a pair
python tools/rust/parity/parity.py --write      # PARITY-TESTS.md and the block below
python tools/rust/parity/parity.py --check      # both up to date (for CI)
```

Every case with its Rust tests: [PARITY-TESTS.md](PARITY-TESTS.md).

<!-- parity.py summary begin -->

| suite | C++ files | C++ cases | ported | no Rust form | retired | missing |
|---|---|---|---|---|---|---|
| esp-core | 53 | 1039 | 1037 | 2 | 0 | 0 |
| esp-glue | 30 | 662 | 614 | 18 | 30 | 0 |
| stm-core | 45 | 405 | 405 | 0 | 0 | 0 |
| stm-glue | 29 | 336 | 330 | 6 | 0 | 0 |
| all | 157 | 2442 | 2386 | 26 | 30 | 0 |

Ported, by kind: manual 38, hint 12, exact 2098, prefix 16, subcase 11, fuzzy 211. Rust tests: 3185, 733 of them without a C++ case.

| C++ file | cases | ported | no Rust form | retired | missing |
|---|---|---|---|---|---|
| `test_calib_schedule.cpp` (software_esp32_revamped) | 18 | 18 | 0 | 0 | 0 |
| `test_calib_schedule__link.cpp` (software_esp32_revamped) | 16 | 16 | 0 | 0 | 0 |
| `test_common.cpp` (software_esp32_revamped) | 21 | 21 | 0 | 0 | 0 |
| `test_config.cpp` (software_esp32_revamped) | 75 | 74 | 1 | 0 | 0 |
| `test_config__repairs.cpp` (software_esp32_revamped) | 12 | 12 | 0 | 0 | 0 |
| `test_event_limiter.cpp` (software_esp32_revamped) | 12 | 12 | 0 | 0 | 0 |
| `test_event_log.cpp` (software_esp32_revamped) | 19 | 19 | 0 | 0 | 0 |
| `test_factory_reset.cpp` (software_esp32_revamped) | 5 | 5 | 0 | 0 | 0 |
| `test_failsafe.cpp` (software_esp32_revamped) | 6 | 6 | 0 | 0 | 0 |
| `test_file_manager.cpp` (software_esp32_revamped) | 4 | 4 | 0 | 0 | 0 |
| `test_ha_discovery.cpp` (software_esp32_revamped) | 40 | 40 | 0 | 0 | 0 |
| `test_health_monitor.cpp` (software_esp32_revamped) | 11 | 11 | 0 | 0 | 0 |
| `test_health_monitor__link.cpp` (software_esp32_revamped) | 11 | 11 | 0 | 0 | 0 |
| `test_image_store.cpp` (software_esp32_revamped) | 2 | 2 | 0 | 0 | 0 |
| `test_image_store__mut.cpp` (software_esp32_revamped) | 2 | 2 | 0 | 0 | 0 |
| `test_json_api.cpp` (software_esp32_revamped) | 20 | 20 | 0 | 0 | 0 |
| `test_json_api__mut.cpp` (software_esp32_revamped) | 2 | 2 | 0 | 0 | 0 |
| `test_json_writer.cpp` (software_esp32_revamped) | 14 | 14 | 0 | 0 | 0 |
| `test_lease_client.cpp` (software_esp32_revamped) | 36 | 36 | 0 | 0 | 0 |
| `test_lease_client__mut.cpp` (software_esp32_revamped) | 14 | 14 | 0 | 0 | 0 |
| `test_legacy_http.cpp` (software_esp32_revamped) | 6 | 6 | 0 | 0 | 0 |
| `test_legacy_http__mut.cpp` (software_esp32_revamped) | 1 | 1 | 0 | 0 | 0 |
| `test_legacy_import.cpp` (software_esp32_revamped) | 38 | 38 | 0 | 0 | 0 |
| `test_legacy_import__mut.cpp` (software_esp32_revamped) | 2 | 2 | 0 | 0 | 0 |
| `test_line_assembler.cpp` (software_esp32_revamped) | 17 | 17 | 0 | 0 | 0 |
| `test_link_policy.cpp` (software_esp32_revamped) | 67 | 67 | 0 | 0 | 0 |
| `test_log_sink.cpp` (software_esp32_revamped) | 10 | 10 | 0 | 0 | 0 |
| `test_mqtt_policy.cpp` (software_esp32_revamped) | 16 | 16 | 0 | 0 | 0 |
| `test_mqtt_policy__mut.cpp` (software_esp32_revamped) | 5 | 5 | 0 | 0 | 0 |
| `test_mqtt_topics.cpp` (software_esp32_revamped) | 24 | 24 | 0 | 0 | 0 |
| `test_mqtt_topics__mut.cpp` (software_esp32_revamped) | 6 | 6 | 0 | 0 | 0 |
| `test_mqtt_values.cpp` (software_esp32_revamped) | 12 | 12 | 0 | 0 | 0 |
| `test_mqtt_values__mut.cpp` (software_esp32_revamped) | 5 | 5 | 0 | 0 | 0 |
| `test_net_policy.cpp` (software_esp32_revamped) | 14 | 14 | 0 | 0 | 0 |
| `test_net_trial.cpp` (software_esp32_revamped) | 16 | 15 | 1 | 0 | 0 |
| `test_ota_policy.cpp` (software_esp32_revamped) | 15 | 15 | 0 | 0 | 0 |
| `test_poll_planner.cpp` (software_esp32_revamped) | 52 | 52 | 0 | 0 | 0 |
| `test_reset_gate.cpp` (software_esp32_revamped) | 5 | 5 | 0 | 0 | 0 |
| `test_restart_gate.cpp` (software_esp32_revamped) | 3 | 3 | 0 | 0 | 0 |
| `test_restart_gate__mut.cpp` (software_esp32_revamped) | 1 | 1 | 0 | 0 | 0 |
| `test_smoke.cpp` (software_esp32_revamped) | 8 | 8 | 0 | 0 | 0 |
| `test_stm_codec.cpp` (software_esp32_revamped) | 70 | 70 | 0 | 0 | 0 |
| `test_stm_flasher.cpp` (software_esp32_revamped) | 77 | 77 | 0 | 0 | 0 |
| `test_stm_session.cpp` (software_esp32_revamped) | 29 | 29 | 0 | 0 | 0 |
| `test_stm_session__mut.cpp` (software_esp32_revamped) | 63 | 63 | 0 | 0 | 0 |
| `test_sys_health.cpp` (software_esp32_revamped) | 15 | 15 | 0 | 0 | 0 |
| `test_target_store.cpp` (software_esp32_revamped) | 11 | 11 | 0 | 0 | 0 |
| `test_valve_model.cpp` (software_esp32_revamped) | 38 | 38 | 0 | 0 | 0 |
| `test_valve_model__link.cpp` (software_esp32_revamped) | 31 | 31 | 0 | 0 | 0 |
| `test_valve_model__mut.cpp` (software_esp32_revamped) | 9 | 9 | 0 | 0 | 0 |
| `test_version.cpp` (software_esp32_revamped) | 11 | 11 | 0 | 0 | 0 |
| `test_web_guard.cpp` (software_esp32_revamped) | 15 | 15 | 0 | 0 | 0 |
| `test_web_guard__mut.cpp` (software_esp32_revamped) | 7 | 7 | 0 | 0 | 0 |
| `glue/selftest.cpp` (software_esp32_revamped) | 39 | 20 | 14 | 5 | 0 |
| `glue/test_app.cpp` (software_esp32_revamped) | 29 | 29 | 0 | 0 | 0 |
| `glue/test_app__eq.cpp` (software_esp32_revamped) | 1 | 0 | 0 | 1 | 0 |
| `glue/test_app__mut.cpp` (software_esp32_revamped) | 4 | 4 | 0 | 0 | 0 |
| `glue/test_logger.cpp` (software_esp32_revamped) | 20 | 20 | 0 | 0 | 0 |
| `glue/test_logger__mut.cpp` (software_esp32_revamped) | 3 | 3 | 0 | 0 | 0 |
| `glue/test_main.cpp` (software_esp32_revamped) | 4 | 1 | 0 | 3 | 0 |
| `glue/test_mqtt_client.cpp` (software_esp32_revamped) | 39 | 38 | 1 | 0 | 0 |
| `glue/test_mqtt_client__eq.cpp` (software_esp32_revamped) | 5 | 4 | 1 | 0 | 0 |
| `glue/test_mqtt_client__gate.cpp` (software_esp32_revamped) | 10 | 10 | 0 | 0 | 0 |
| `glue/test_mqtt_client__mut.cpp` (software_esp32_revamped) | 59 | 59 | 0 | 0 | 0 |
| `glue/test_net.cpp` (software_esp32_revamped) | 43 | 43 | 0 | 0 | 0 |
| `glue/test_net__eq.cpp` (software_esp32_revamped) | 1 | 1 | 0 | 0 | 0 |
| `glue/test_net_edges.cpp` (software_esp32_revamped) | 37 | 37 | 0 | 0 | 0 |
| `glue/test_ota.cpp` (software_esp32_revamped) | 41 | 40 | 1 | 0 | 0 |
| `glue/test_ota__mut.cpp` (software_esp32_revamped) | 14 | 13 | 1 | 0 | 0 |
| `glue/test_stm_link.cpp` (software_esp32_revamped) | 25 | 25 | 0 | 0 | 0 |
| `glue/test_stm_link__mut.cpp` (software_esp32_revamped) | 7 | 7 | 0 | 0 | 0 |
| `glue/test_stm_service.cpp` (software_esp32_revamped) | 20 | 20 | 0 | 0 | 0 |
| `glue/test_storage.cpp` (software_esp32_revamped) | 16 | 16 | 0 | 0 | 0 |
| `glue/test_storage_config.cpp` (software_esp32_revamped) | 49 | 49 | 0 | 0 | 0 |
| `glue/test_storage_images.cpp` (software_esp32_revamped) | 24 | 24 | 0 | 0 | 0 |
| `glue/test_storage_values.cpp` (software_esp32_revamped) | 30 | 30 | 0 | 0 | 0 |
| `glue/test_web_server.cpp` (software_esp32_revamped) | 43 | 41 | 0 | 2 | 0 |
| `glue/test_web_server__eq.cpp` (software_esp32_revamped) | 7 | 0 | 0 | 7 | 0 |
| `glue/test_web_server__mut_a.cpp` (software_esp32_revamped) | 21 | 16 | 0 | 5 | 0 |
| `glue/test_web_server__mut_a_views.cpp` (software_esp32_revamped) | 15 | 14 | 0 | 1 | 0 |
| `glue/test_web_server__mut_b1.cpp` (software_esp32_revamped) | 20 | 20 | 0 | 0 | 0 |
| `glue/test_web_server__mut_b2.cpp` (software_esp32_revamped) | 22 | 19 | 0 | 3 | 0 |
| `glue/test_web_server__work.cpp` (software_esp32_revamped) | 14 | 11 | 0 | 3 | 0 |
| `test_arg_parser.cpp` (software_stm32) | 10 | 10 | 0 | 0 | 0 |
| `test_buf_writer.cpp` (software_stm32) | 10 | 10 | 0 | 0 | 0 |
| `test_calibration.cpp` (software_stm32) | 14 | 14 | 0 | 0 | 0 |
| `test_config_blocks.cpp` (software_stm32) | 18 | 18 | 0 | 0 | 0 |
| `test_config_store.cpp` (software_stm32) | 18 | 18 | 0 | 0 | 0 |
| `test_eeprom_layout.cpp` (software_stm32) | 12 | 12 | 0 | 0 | 0 |
| `test_end_stop_detector.cpp` (software_stm32) | 19 | 19 | 0 | 0 | 0 |
| `test_end_stop_detector__inrush.cpp` (software_stm32) | 9 | 9 | 0 | 0 | 0 |
| `test_failsafe.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `test_failsafe__drive.cpp` (software_stm32) | 1 | 1 | 0 | 0 | 0 |
| `test_fault_retry.cpp` (software_stm32) | 7 | 7 | 0 | 0 | 0 |
| `test_fuzz.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `test_lease.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `test_lease__class.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `test_legacy_layout.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `test_line_assembler.cpp` (software_stm32) | 16 | 16 | 0 | 0 | 0 |
| `test_manual_enable.cpp` (software_stm32) | 4 | 4 | 0 | 0 | 0 |
| `test_motor_params.cpp` (software_stm32) | 12 | 12 | 0 | 0 | 0 |
| `test_move_classifier.cpp` (software_stm32) | 11 | 11 | 0 | 0 | 0 |
| `test_move_classifier__mut.cpp` (software_stm32) | 2 | 2 | 0 | 0 | 0 |
| `test_move_classifier__partial.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `test_onewire_check.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `test_presence_test.cpp` (software_stm32) | 9 | 9 | 0 | 0 | 0 |
| `test_profile_recorder.cpp` (software_stm32) | 13 | 13 | 0 | 0 | 0 |
| `test_protection_guard.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `test_protection_guard__mut.cpp` (software_stm32) | 1 | 1 | 0 | 0 | 0 |
| `test_replies.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `test_replies_v2.cpp` (software_stm32) | 10 | 10 | 0 | 0 | 0 |
| `test_replies_v3.cpp` (software_stm32) | 12 | 12 | 0 | 0 | 0 |
| `test_reset_guard.cpp` (software_stm32) | 10 | 10 | 0 | 0 | 0 |
| `test_reset_guard__mut.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `test_retry_backoff.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `test_settings.cpp` (software_stm32) | 5 | 5 | 0 | 0 | 0 |
| `test_settings__countdown.cpp` (software_stm32) | 2 | 2 | 0 | 0 | 0 |
| `test_stall_detector.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `test_store_scheduler.cpp` (software_stm32) | 13 | 13 | 0 | 0 | 0 |
| `test_system_stats.cpp` (software_stm32) | 7 | 7 | 0 | 0 | 0 |
| `test_target_rejection.cpp` (software_stm32) | 7 | 7 | 0 | 0 | 0 |
| `test_temp_filter.cpp` (software_stm32) | 11 | 11 | 0 | 0 | 0 |
| `test_temp_refresh.cpp` (software_stm32) | 7 | 7 | 0 | 0 | 0 |
| `test_tokenizer.cpp` (software_stm32) | 13 | 13 | 0 | 0 | 0 |
| `test_uart_errors.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `test_valve_scheduler.cpp` (software_stm32) | 28 | 28 | 0 | 0 | 0 |
| `test_valve_scheduler__mut.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `test_warm_state.cpp` (software_stm32) | 9 | 9 | 0 | 0 | 0 |
| `glue/test_app.cpp` (software_stm32) | 9 | 9 | 0 | 0 | 0 |
| `glue/test_app__mut.cpp` (software_stm32) | 22 | 22 | 0 | 0 | 0 |
| `glue/test_app_v3.cpp` (software_stm32) | 34 | 34 | 0 | 0 | 0 |
| `glue/test_communication.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `glue/test_communication__mut.cpp` (software_stm32) | 13 | 13 | 0 | 0 | 0 |
| `glue/test_communication_v1.cpp` (software_stm32) | 7 | 7 | 0 | 0 | 0 |
| `glue/test_communication_v3.cpp` (software_stm32) | 21 | 21 | 0 | 0 | 0 |
| `glue/test_eeprom.cpp` (software_stm32) | 17 | 17 | 0 | 0 | 0 |
| `glue/test_eeprom__mut.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `glue/test_fakes.cpp` (software_stm32) | 29 | 23 | 6 | 0 | 0 |
| `glue/test_i2c_bus.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `glue/test_main.cpp` (software_stm32) | 10 | 10 | 0 | 0 | 0 |
| `glue/test_main__mut.cpp` (software_stm32) | 2 | 2 | 0 | 0 | 0 |
| `glue/test_motor.cpp` (software_stm32) | 7 | 7 | 0 | 0 | 0 |
| `glue/test_motor__mut.cpp` (software_stm32) | 42 | 42 | 0 | 0 | 0 |
| `glue/test_motor_v3.cpp` (software_stm32) | 22 | 22 | 0 | 0 | 0 |
| `glue/test_otasupport.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `glue/test_owDevices.cpp` (software_stm32) | 5 | 5 | 0 | 0 | 0 |
| `glue/test_owDevices__mut.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `glue/test_owDevices_s7.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `glue/test_sysstat.cpp` (software_stm32) | 11 | 11 | 0 | 0 | 0 |
| `glue/test_system_boot.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `glue/test_system_calib.cpp` (software_stm32) | 3 | 3 | 0 | 0 | 0 |
| `glue/test_system_config.cpp` (software_stm32) | 5 | 5 | 0 | 0 | 0 |
| `glue/test_system_io.cpp` (software_stm32) | 2 | 2 | 0 | 0 | 0 |
| `glue/test_system_motion.cpp` (software_stm32) | 8 | 8 | 0 | 0 | 0 |
| `glue/test_system_proto3.cpp` (software_stm32) | 6 | 6 | 0 | 0 | 0 |
| `glue/test_terminal.cpp` (software_stm32) | 12 | 12 | 0 | 0 | 0 |
| `glue/test_terminal__mut.cpp` (software_stm32) | 12 | 12 | 0 | 0 | 0 |

<!-- parity.py summary end -->

**No Rust form (26).** Each is a row of a port note: the harness subjects of the C++
`selftest.cpp` that have no Rust counterpart (FreeRTOS fakes, sibling fakes, Preferences and the
stdio layer, the bootloader rollback, two xfails: [PORT-NOTES.md#testkit](PORT-NOTES.md#testkit));
an unterminated C string in `config` and `net_trial` (the full-length texts are tested instead:
[PORT-NOTES.md#config](PORT-NOTES.md#config), [#net_trial](PORT-NOTES.md#net_trial)); a failing
`esp_ota_mark_app_valid_cancel_rollback` and the 47-character cut of a library error
([PORT-NOTES.md#ota](PORT-NOTES.md#ota)); the MQTT reload copy and the profile padding
([PORT-NOTES.md#glue](PORT-NOTES.md#glue)); the `readBytes`/`end` of the fake UART, PRIMASK and
the fork-per-case runner hooks of the STM glue suites
([PORT-NOTES-STM.md](PORT-NOTES-STM.md#c-tests-without-a-rust-form)).

**Retired (30).** The cases of the AsyncWebServer and Arduino mechanisms that the Rust server and
boot order replace, as GLUE-DESIGN-ESP.md 5.2 lists them (approved, decision 7.4): the two
response slots, the marks of concurrent bodies, 503 `retry`, the library's multipart and
pipelining quirks, the Arduino hooks of `main.cpp`, `xTaskGetHandle(nullptr)`. Where the subject
remains, the replacement case is named in PARITY-TESTS.md.

**Assertions.** PORTING.md requires every assertion to be ported; the pairing above checks
cases, not assertions. The assertions known to have no Rust form are listed in PORT-NOTES.md
("C++ tests without a Rust form" and the module sections) and PORT-NOTES-STM.md (the set-up the
firmware owns: pins, baud rates, watchdog and timer start).

**Rust-only tests.** The Rust tests that pair with no C++ case (counted above) cover the edges
the mutation gate found, the new modules (`boot_guard`, `http_parse`, `json_body`, `mqtt_conn`,
`ota/update`, the STM `serial`, `i2c_master`, `onewire`, `dallas`, `ds2438`, `eeprom24`,
`hw_timer`, the boot crate), the D9 flasher order and the C++ goldens.

## 3. Feature parity

Status: **same** (the C++ behaviour, covered by the tests named), **deviation** (a documented,
intended difference; the link names the document), **hardware** (an adapter of the firmware
crate, which no host test runs by design; the proof named is the emulator's or the operator's).

### 3.1 HTTP API (ESP)

All 43 routes of the C++ router are in the Rust router in the same order, plus one Rust-only
route (`esp:core/src/json_api/tests.rs::api_every_route_and_method`); every JSON document writes
the C++ keys in the C++ order (compared writer by writer). `mock_api.py` has no route beyond the
C++ table, and the dashboard calls only the routes below. Handlers are functions of
`esp:glue/src/web_server.rs`, documents of `esp:glue/src/web_server/views.rs`, uploads of
`esp:glue/src/web_server/uploads.rs`.

| Route | Rust | Tests | Status |
|---|---|---|---|
| GET /api/health | `health`; core `write_health_json` | `esp:glue/src/web_server/tests.rs::wg17_health_answers_from_the_app_document`; `esp:glue/src/web_server/tests_mut_a_views.rs::health_the_document_may_use_its_whole_1024_byte_buffer`; `esp:core/src/json_api/tests.rs::health_document`; QEMU `api` | deviation: the task names `httpd` and the IDF tasks instead of `async_tcp`, `arduino_events` ([PORT-NOTES.md#app](PORT-NOTES.md#app)) |
| GET /api/status | `status`; core `write_status_json` | `esp:glue/src/web_server/tests.rs::wg15_status_members_of_2_1`; `esp:glue/src/web_server/tests_rust.rs::status_shows_the_clock_the_last_sync_and_the_board_figures`; `esp:core/src/json_api/tests.rs::api_status_of_a_populated_snapshot`; QEMU `api` | same |
| GET /api/valves | `valves`, `valve_views` | `esp:glue/src/web_server/tests.rs::valves_carry_the_sensor_position_and_the_calibration_end`; `esp:glue/src/web_server/tests_mut_a_views.rs::valves_the_sensors_of_every_valve_with_name_offset_and_validity`; `esp:core/src/json_api/tests.rs::api_valves_document`; QEMU `api` | deviation: 503 when the list gets no memory ([PORT-NOTES.md#glue](PORT-NOTES.md#glue), web_server 4.3) |
| GET /api/valves/{n}/profile | `profile` | `esp:glue/src/web_server/tests_mut_a_views.rs::a_valve_profile_404_without_one`; `esp:core/src/json_api/tests.rs::api_profile_document` | same |
| GET /api/sensors | `sensors`, `sensor_views` | `esp:glue/src/web_server/tests_mut_a_views.rs::sensors_temperature_slots_bus_sensors_without_a_slot_staleness_and_age`; `esp:glue/src/web_server/tests_mut_a_views.rs::sensors_voltage_slots_conversion_clamping_and_bus_sensors_without_a_slot`; `esp:core/src/json_api/tests.rs::api_sensors_document`; QEMU `api` | deviation: 503 for the list as above |
| GET /api/events | `events`, `events_fit` | `esp:glue/src/web_server/tests_mut_a_views.rs::events_default_and_maximum_count_parameters_and_their_errors`; `esp:glue/src/web_server/tests_mut_a_views.rs::events_a_count_that_does_not_fit_is_halved_until_it_does`; `esp:glue/src/web_server/tests_mut_a_views.rs::events_without_memory_for_the_list_answer_503`; QEMU `api` | deviation: 503 for the list as above |
| GET /api/stm/motor | `motor` | `esp:glue/src/web_server/tests_mut_a_views.rs::the_motor_parameters_with_and_without_breakaway`; `esp:core/src/json_api/tests.rs::api_motor_document`; QEMU `api` | same |
| GET /api/stm/flash | `flash_status` | `esp:glue/src/web_server/tests_mut_a_views.rs::the_flash_status_names_its_image`; `esp:glue/src/web_server/tests.rs::images_and_the_flash_status_show_the_board_revision`; `esp:core/src/json_api/tests.rs::api_flash_status_document`; QEMU `api` | same |
| GET /api/import-report | `import_report` | `esp:glue/src/web_server/tests.rs::wg14_the_import_report_is_sent_and_dismissed`; `esp:glue/src/web_server/tests_mut_b2.rs::import_report_an_empty_and_an_oversized_file`; QEMU `api` (404) | same |
| GET /api/log | `log_download` | `esp:glue/src/web_server/tests_mut_b2.rs::log_the_previous_file_first_then_the_current_one`; `esp:glue/src/web_server/tests_mut_b2.rs::log_a_client_gone_ends_the_download_none_without_a_file_system`; `esp:glue/src/web_server/tests.rs::wg18_every_served_request_reports_its_client_the_log_is_flushed_first`; QEMU `api`, `littlefs` | deviation: one request at a time, no 409 for a second download (GLUE-DESIGN-ESP.md 4.6, 4.7 row 4) |
| POST /api/valves/{n}/target | `target`, `submit_target` | `esp:glue/src/web_server/tests.rs::a_valve_target_is_submitted_and_answered_202`; `esp:glue/src/web_server/tests.rs::a_target_out_of_range_an_inactive_valve_a_full_queue`; `esp:glue/src/web_server/tests.rs::wg9_fractional_targets_are_rounded_other_values_refused` | same |
| POST /api/valves/{n}/stop, POST /api/valves/stop | `stop` | `esp:glue/src/web_server/tests.rs::stop_and_safe_mode_leave_need_protocol_3`; `esp:glue/src/web_server/tests.rs::stm_actions_are_refused_while_the_stm_firmware_is_too_old` | same |
| POST /api/valves/{n}/calibrate, /assembly; POST /api/valves/calibrate, /assembly, /detect; POST /api/sensors/scan | `simple` | `esp:glue/src/web_server/tests_rust.rs::every_simple_stm_route_queues_its_command`; `esp:glue/src/web_server/tests.rs::stm_actions_are_refused_while_the_stm_firmware_is_too_old` | same |
| POST /api/valves/{n}/service-move | `service_move`, `move_fields` | `esp:glue/src/web_server/tests_mut_b1.rs::service_move_protocol_2_is_required_the_command_carries_dir_counts_and_max_ma`; `esp:glue/src/web_server/tests_mut_b1.rs::service_move_every_member_is_required_and_checked` | same |
| POST /api/valves/{n}/sensors | `valve_sensors`, `sensor_slots` | `esp:glue/src/web_server/tests_mut_b1.rs::valve_sensors_the_slots_name_the_configured_ids`; `esp:glue/src/web_server/tests_mut_b1.rs::valve_sensors_ranges_distinct_slots_and_valid_ids` | same |
| POST /api/valves/{n}/profile | `profile_refresh` | `esp:glue/src/web_server/tests_mut_b1.rs::profile_refresh_protocol_2_is_required` | same |
| POST /api/stm/safe-mode/leave | `safe_mode_leave` | `esp:glue/src/web_server/tests.rs::stop_and_safe_mode_leave_need_protocol_3` | same |
| GET /api/config, GET /api/config/export | `config_get`; core `write_config_json` | `esp:glue/src/web_server/tests_mut_b2.rs::config_get_is_inline_no_download`; `esp:glue/src/web_server/tests.rs::wg11_the_export_never_carries_a_secret_secrets_1_included`; `esp:core/src/config/tests.rs::json_export_of_the_defaults_golden`; QEMU `api`, `nvs`, `littlefs` | same |
| POST /api/config | `config_patch`, `patch`, `save_config` | `esp:glue/src/web_server/tests.rs::wg10_the_save_answer_tells_whether_a_restart_and_a_trial_follow`; `esp:glue/src/web_server/tests.rs::wg10_a_failed_save_answers_its_path_and_reloads_the_copy`; `esp:glue/src/web_server/tests.rs::wg10_a_save_without_memory_for_its_answer_applies_nothing`; QEMU `api`, `nvs`, `mqtt` | deviation: no 503 "response buffers in use" (GLUE-DESIGN-ESP.md 4.3, 4.7 row 1) |
| POST /api/config?dryRun=1 | `config_patch`, `dry_run_answer` | `esp:glue/src/web_server/tests.rs::wg10_a_dry_run_validates_and_answers_without_storing`; `esp:glue/src/web_server/tests.rs::wg10_a_dry_run_needs_no_response_buffer`; `esp:glue/src/web_server/tests_mut_b2.rs::config_a_failed_dry_run_leaves_the_copy_as_the_active_config`; QEMU `api` | same |
| POST /api/stm/motor | `motor_set`, `motor_fields` | `esp:glue/src/web_server/tests_mut_b1.rs::motor_all_five_motor_values_are_sent_as_one_command`; `esp:glue/src/web_server/tests_mut_b1.rs::motor_breakaway_needs_protocol_2_and_a_complete_set_until_it_was_read`; `esp:glue/src/web_server/tests_mut_b1.rs::motor_learn_movements_is_0_or_50_to_65534` | same |
| POST /api/system/network/confirm, /revert | `net_trial` | `esp:glue/src/web_server/tests.rs::wg12_confirm_and_revert_of_a_network_trial` | same |
| GET /api/stm/images | `images`, `write_image` | `esp:glue/src/web_server/tests_mut_a_views.rs::the_image_list_shows_crc_version_check_and_board_only_when_known`; `esp:glue/src/web_server/tests_mut_b1.rs::images_the_list_answers_every_image_once` | same |
| POST /api/stm/images | `upload`, `first_data`, `finish_upload` | `esp:glue/src/web_server/tests.rs::upload_an_stm_image_is_stored_and_described`; `esp:glue/src/web_server/tests_mut_b2.rs::upload_the_status_of_every_storage_result`; `esp:glue/src/web_server/tests_rust.rs::upload_a_body_without_the_boundary_has_no_file`; QEMU `api` | deviation: a malformed multipart body is answered 400, a stalled body gets no answer (GLUE-DESIGN-ESP.md 4.7 rows 7, 9) |
| DELETE /api/stm/images/{name} | `image_delete` | `esp:glue/src/web_server/tests_mut_b1.rs::image_delete_each_storage_result_has_its_answer`; QEMU `api` | same |
| POST /api/stm/flash | `flash`, `flash_refused`, `board_refused` | `esp:glue/src/web_server/tests.rs::a_flash_checks_the_board_revision_of_the_image`; `esp:glue/src/web_server/tests_mut_b1.rs::flash_the_request_members`; `esp:glue/src/web_server/tests_mut_b1.rs::flash_busy_restarting_unknown_and_unchecked_images_are_refused` | same |
| POST /api/stm/flash/abort | `flash_abort` | `esp:glue/src/web_server/tests_mut_b1.rs::flash_abort_only_a_running_flash_is_aborted` | same |
| POST /api/stm/reset | `stm_reset` | `esp:glue/src/web_server/tests_mut_b1.rs::stm_reset_only_confirm_true_resets` | same |
| POST /api/ota/esp | `upload`, `upload_md5` | `esp:glue/src/web_server/tests.rs::upload_an_esp_image_goes_to_ota_and_asks_for_the_restart`; `esp:glue/src/web_server/tests_mut_b2.rs::upload_the_md5_from_a_form_field_or_a_header`; `esp:glue/src/web_server/tests_mut_b2.rs::upload_the_md5_query_in_capitals_after_the_small_ones_before_the_capital_field`; `esp:glue/src/web_server/tests_rust.rs::an_esp_upload_is_refused_while_the_image_is_on_trial`; QEMU `ota`, `boot` | deviation: 409 `upload_failed` "image on trial" (decision 7.5); malformed multipart answered 400 |
| POST /api/system/reboot, POST /api/mqtt/reconnect | `api_system` | `esp:glue/src/web_server/tests.rs::reboot_and_mqtt_reconnect_are_handed_to_their_modules` | same |
| POST /api/system/factory-reset | `factory_reset` | `esp:glue/src/web_server/tests_mut_b1.rs::factory_reset_confirmed_erased_restart_requested` | deviation: the Rust keeps NVS `otaOk`, `otaTrial` ([PORT-NOTES.md#storage](PORT-NOTES.md#storage)) |
| GET /api/files, DELETE /api/files?path= | `files`, `file_delete` | `esp:glue/src/web_server/tests.rs::wg13_the_file_list_and_file_removal`; `esp:glue/src/web_server/tests_mut_a_views.rs::files_at_most_32_entries_more_are_reported_as_truncated`; QEMU `api` | deviation: 503 for the list as above |
| DELETE /api/import-report | `import_report_dismiss` | `esp:glue/src/web_server/tests.rs::wg14_the_import_report_is_sent_and_dismissed` | same |
| POST /api/mqtt/discovery | `discovery` | `esp:glue/src/web_server/tests.rs::mqtt_discovery_rules_per_mode`; `esp:glue/src/web_server/tests_mut_b1.rs::mqtt_discovery_an_unknown_action_is_refused` | same |
| POST /api/system/ota/switch-back (Rust only) | `switch_back`; `esp:glue/src/boot_guard.rs` | `esp:glue/src/web_server/tests_rust.rs::switch_back_restarts_into_the_other_image`; `esp:glue/src/web_server/tests_rust.rs::switch_back_needs_the_confirmation`; `esp:glue/src/web_server/tests_rust.rs::switch_back_without_an_image_in_the_other_slot`; `esp:core/src/json_api/tests.rs::route_of_the_ota_switch_back`; QEMU `boot` | deviation: new route ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations), decision 7.6) |
| Unknown path (404), known path with another method (405), HEAD/PUT/OPTIONS (405) | `api_system` | `esp:glue/src/web_server/tests.rs::unknown_paths_and_methods`; `esp:core/src/json_api/tests.rs::api_malformed_and_unknown_paths`; QEMU `api` | same |
| Non-API path with a body (GET 404, else 405, never read) | `legacy_refused` | `esp:glue/src/web_server/tests_mut_a.rs::a_body_to_a_path_outside_api_is_refused_without_being_read` | same |
| Guard: Host (403 `host_not_allowed` with the detail) | `guard`; core `check_request`, `guard_detail` | `esp:glue/src/web_server/tests.rs::wg3_the_host_header_must_name_this_device_assets_are_not_checked`; `esp:core/src/web_guard/tests.rs::guard_detail_texts` | same |
| Guard: Origin (403 `origin_not_allowed`) | `guard_request`; core `origin_allowed` | `esp:glue/src/web_server/tests.rs::wg4_a_foreign_origin_is_refused_the_own_one_accepted`; `esp:core/src/web_guard/tests.rs::origin_allowed_rules` | same |
| Guard: X-VdMot on API writes and uploads (403 `header_required`) | `guard_request` | `esp:glue/src/web_server/tests.rs::wg1_an_api_write_without_x_vdmot_is_refused_nothing_submitted`; `esp:glue/src/web_server/tests.rs::wg5_an_upload_without_x_vdmot_never_reaches_storage`; `esp:core/src/web_guard/tests.rs::check_request_the_x_vdmot_marker_for_api_writes` | same |
| Guard: JSON bodies (415), multipart where JSON is expected (415), event 213 once per verdict and minute | `guard_request`, `body_refused`, `refuse` | `esp:glue/src/web_server/tests.rs::wg2_a_body_must_be_json_parameters_of_the_media_type_are_fine`; `esp:glue/src/web_server/tests_mut_a.rs::guard_a_multipart_body_outside_the_upload_routes_is_refused`; `esp:glue/src/web_server/tests.rs::wg16_request_refused_is_logged_once_per_verdict_and_minute` | same |
| Limits: upload not multipart (415), without Content-Length (411), too large (413 "file"), upload or flash running (409); body over 8 KB (413, never read) | `upload_refused`, `upload_limit`, `body_refused` | `esp:glue/src/web_server/tests.rs::guard_uploads_need_multipart_and_a_length`; `esp:glue/src/web_server/tests_mut_a.rs::guard_an_esp_image_may_fill_the_update_slot_plus_8_kib_of_framing`; `esp:glue/src/web_server/tests_mut_a.rs::guard_an_upload_waits_for_every_other_upload_and_the_flash`; `esp:glue/src/web_server/tests.rs::guard_a_json_body_over_8192_bytes_is_refused_without_being_read` | same |
| 503 `busy` "out of memory" (working set, body buffer) | `take_work`, `read_body` | `esp:glue/src/web_server/tests_work.rs::working_set_without_memory_the_first_request_is_answered_503_the_next_allocates`; `esp:glue/src/web_server/tests_work.rs::working_set_each_of_the_parts_that_cannot_be_allocated_answers_503`; `esp:glue/src/web_server/tests_work.rs::body_a_body_without_memory_for_its_buffer_is_refused_503_nothing_runs` | same |
| 503 "response buffers in use", 409 `busy` "body", 503 `retry` | none (one request at a time) | `esp:glue/src/web_server/tests.rs::requests_one_after_another_each_get_the_response_buffer` | deviation: never sent (GLUE-DESIGN-ESP.md 4.7 rows 1-3) |
| Error body `{"error","detail"}`, `{"error":"internal"}` above 200 bytes | `send_error` | `esp:glue/src/web_server/tests_mut_a.rs::an_error_answer_longer_than_its_200_byte_buffer_becomes_internal`; `esp:core/src/json_api/tests.rs::api_error_document` | same |
| Connections: keep-alive, serial handling, 431/414/400 for oversized heads, header trimming, stalled clients | esp_http_server adapter; `read_body`, `receive` | `esp:glue/src/web_server/tests_rust.rs::body_a_client_that_stalls_three_times_in_a_row_gets_no_answer`; `esp:glue/src/web_server/tests_rust.rs::body_reads_that_deliver_nothing_count_as_stalls`; `esp:glue/src/web_server/tests_rust.rs::upload_a_client_that_stalls_three_times_in_a_row_aborts_it`; QEMU `soak` | deviation (GLUE-DESIGN-ESP.md 4.7 rows 4-6, 8, 9) |
| GET /valves | `legacy_valves`; core `write_legacy_valves_json` | `esp:glue/src/web_server/tests.rs::wg8_get_valves_answers_the_legacy_document`; `esp:core/src/legacy_http/tests.rs::legacy_valves_document`; `esp:glue/src/web_server/tests_mut_a.rs::guard_a_get_alias_ignores_its_body_and_takes_at_most_8192_bytes_for_it`; QEMU `api` | same (the body of a GET alias since `3aea221`) |
| GET /temps, GET /volts | `legacy_sensors` | `esp:glue/src/web_server/tests.rs::wg8_temps_and_volts_answer_the_legacy_documents`; `esp:core/src/legacy_http/tests.rs::legacy_temps_document`; `esp:core/src/legacy_http/tests.rs::legacy_volts_document` | same (as above) |
| POST /setvalve (no X-VdMot needed) | `set_valve` | `esp:glue/src/web_server/tests.rs::wg8_post_setvalve_rounds_the_value_and_answers_the_legacy_body`; `esp:glue/src/web_server/tests_mut_a.rs::guard_setvalve_reads_a_body_of_8192_bytes`; `esp:glue/src/web_server/tests.rs::no_request_needs_credentials_an_authorization_header_is_ignored` | same |
| An alias with another method (405) | `legacy_refused` | `esp:core/src/legacy_http/tests.rs::legacy_routes_aliases_per_method` | same |
| The 410 table: 27 paths and their replacements (/netinfo, /sysinfo ... /auth), any method, not guarded, body never read | `legacy_refused`; core `legacy_http` | `esp:core/src/legacy_http/tests.rs::legacy_routes_the_410_table`; `esp:glue/src/web_server/tests.rs::wg8_legacy_paths_answer_410_without_reading_others_404_405_never_413`; QEMU `api` | same |
| Dashboard /, /index.html, /app.js, /app.css: gzip level 9 mtime 0, ETag = CRC32 of the gzip bytes, Cache-Control no-cache, 304 without body and Content-Type, 404 for others | `static_asset`, `send_asset`; `esp:glue/build.rs` (feature `dashboard`, the functions of `gen_web_assets.py`) | `esp:glue/src/web_server/tests.rs::the_dashboard_is_served_gzip_with_its_etag_304_when_unchanged`; `esp:glue/src/web_server/tests_mut_b2.rs::static_every_asset_is_served`; `esp:glue/src/web_server/tests_rust.rs::answers_without_content_carry_no_content_type`; `esp:glue/src/web_server/tests_rust.rs::the_dashboard_is_every_file_of_web_gzipped_with_its_etag` (feature-gated, gap O2); QEMU `dashboard` | same |

### 3.2 MQTT and Home Assistant discovery (ESP)

The topic table (55 topics: path, `/value` suffix, retain rule: 42 follow the `retained`
setting, 4 always, 9 never) and the HA definition tables (20 valve kinds, 4 common, 23 device
entities, the DROP lists, the option lists) are the C++ ones entry by entry
(`esp:core/src/mqtt_topics/tests.rs::compat_and_new_topics_byte_exact`,
`esp:core/src/mqtt_topics/tests.rs::retain_flags`; HA payloads byte for byte in
`esp:core/src/ha_discovery/tests.rs`). The client is `esp:glue/src/mqtt_client.rs`, the
PubSubClient 2.8 replacement `esp:glue/src/mqtt_conn.rs`.

| Topic or behaviour | Rust | Tests | Status |
|---|---|---|---|
| in: `<main>valves/<V>/target/set` (separate), payload 0..100, one fraction digit, OPEN/CLOSE, QoS 1, cleared with "" retained | `handle_inbound`; core `decide_inbound`, `parse_target_payload` | `esp:glue/src/mqtt_client/tests.rs::a_target_is_submitted_and_its_retained_topic_cleared`; `esp:glue/src/mqtt_client/tests.rs::fractions_round_half_up_the_number_form_of_a_named_valve`; `esp:core/src/mqtt_topics/tests.rs::parse_target_payload_cases` | same |
| in: `<main>valves/<V>/target` without `separate` (the own value is an echo, never cleared) | core `EchoFilter` | `esp:core/src/mqtt_policy/tests.rs::decide_inbound_the_state_form_without_separate`; `esp:glue/src/mqtt_client/tests.rs::without_separate_the_esps_own_target_is_not_a_command`; `esp:glue/src/mqtt_client/tests_mut.rs::values_without_separate_a_valve_without_a_published_target_has_no_echo` | same |
| in: STOP on a target topic (STM protocol 3) | `act` | `esp:glue/src/mqtt_client/tests.rs::stop_and_safe_exit_need_protocol_3` | same |
| in: `<main>cmd/...` (valve calibrate, calibrate, detect, stmReset, restart, stop, stmSafeExit; PRESS, after the broker's echo, 5 s), unknown commands rejected | `act`; core `ButtonGate`, `parse_inbound_topic` | `esp:glue/src/mqtt_client/tests.rs::every_cmd_topic_submits_its_command_after_the_echo`; `esp:glue/src/mqtt_client/tests.rs::a_button_acts_only_after_the_broker_echoed_its_clear`; `esp:glue/src/mqtt_client/tests.rs::a_button_without_the_echo_is_rejected_after_5_s`; `esp:core/src/mqtt_policy/tests.rs::decide_inbound_cmd_topics` | same |
| in: `homeassistant/status` or `<prefix>/status` (online/offline, mode 2) | `on_ha_status`; core `RegulatorWatch` | `esp:glue/src/mqtt_client/tests.rs::ha_status_offline_online_commands_discovery_on_the_way_back`; `esp:glue/src/mqtt_client/tests.rs::ha_status_is_ignored_outside_ha_mode`; `esp:core/src/mqtt_policy/tests.rs::regulator_watch_ha_status_transitions` | same |
| out: `common/ip`, `common/state`, `common/uptime`, `common/message` | `publish_common` | `esp:glue/src/mqtt_client/tests_mut.rs::full_publish_common_ip_once_per_connection`; `esp:glue/src/mqtt_client/tests.rs::failsafe_lease_valve_and_common_state`; `esp:glue/src/mqtt_client/tests_mut.rs::full_publish_no_uptime_topic_without_up_time`; `esp:glue/src/mqtt_client/tests_mut.rs::events_only_warning_and_above_set_common_message` | same |
| out: `valves/<V>/target`, `requested`, `sync`, `failsafe`, `problem`, `state`, `actual` | `publish_valve`; core `published_target`, `valve_problem` | `esp:glue/src/mqtt_client/tests.rs::requested_sync_and_the_read_back_target`; `esp:glue/src/mqtt_client/tests.rs::a_blocked_valve_at_its_failsafe_position`; `esp:glue/src/mqtt_client/tests.rs::a_failed_temperature_and_link_loss_show_as_problems`; `esp:core/src/mqtt_values/tests.rs::published_target_w5` | same |
| out: `valves/<V>/calibration/date`, `calibration/repetitions` | `publish_valve`; core `CalibEndTracker` | `esp:glue/src/mqtt_client/tests.rs::calibration_end_the_end_of_the_last_calibration_since_boot`; `esp:glue/src/mqtt_client/tests_mut.rs::values_the_topic_and_payload_options_of_the_config_reach_the_broker`; `esp:core/src/mqtt_topics/tests.rs::calibration_date_in_the_legacy_strftime_format` | same |
| out: `valves/<V>/diag/meanCurrrent`, `openCount`, `closeCount`, `deadZoneCount`, `moves` (option `diag`) | `publish_valve` | `esp:glue/src/mqtt_client/tests_mut.rs::values_valve_state_diag_counters_no_calibration_date_inactive_valves`; `esp:glue/src/mqtt_client/tests_mut.rs::values_the_topic_and_payload_options_of_the_config_reach_the_broker` | same |
| out: `valves/<V>/temp1`, `temp2` (with the slot offset), `temps/<T>/id`, `temps/<T>/value` | `publish_valve_temp`, `publish_temp`; core `sensor_topic_segment` | `esp:glue/src/mqtt_client/tests_mut.rs::values_valve_temperatures_with_the_offset_of_their_1_based_slot`; `esp:glue/src/mqtt_client/tests_mut.rs::values_a_sensor_seen_60_001_s_ago_is_stale`; `esp:core/src/mqtt_values/tests.rs::bus_index_and_sensor_topic_segments_e22` | same |
| out: `sensors/<S>/id`, `value` (`%.3f`), `unit` | `publish_volt`; core `format_volt` | `esp:glue/src/mqtt_client/tests.rs::unnamed_sensors_use_the_bus_index_inactive_volts_are_published`; `esp:glue/src/mqtt_client/tests_mut.rs::on_change_a_volt_change_of_10_mv_goes_out`; `esp:core/src/mqtt_topics/tests.rs::temperature_and_volt_payloads` | deviation: out of reach (values from 2^128 on; the on-change value saturates) ([PORT-NOTES.md#kept-quirks](PORT-NOTES.md#kept-quirks), [#glue](PORT-NOTES.md#glue)) |
| Options `retained`, `pathAsRoot`, `plainText`, `germanDecimal`, `diag` as the task applies them | `apply_topics`, `publish_valve`, `publish_common` | `esp:glue/src/mqtt_client/tests_mut.rs::values_the_topic_and_payload_options_of_the_config_reach_the_broker` | same |
| out: `status` (will `offline` QoS 0 retained, `online` after CONNACK), `stm/status`, `failsafe` | `connect`, `publish_system` | `esp:glue/src/mqtt_client/tests.rs::connects_with_the_mac_client_id_lwt_online_first_wildcard_subscriptions`; `esp:glue/src/mqtt_client/tests.rs::stm_status_per_link_state_once_per_change`; `esp:glue/tests/mqtt_interop.rs::a_dropped_connection_publishes_the_will` (Mosquitto, `docker.sh interop`) | same |
| out: `diag/valves/<V>/lastMove`, `earlyStops`, `cmdRejected`, `calState`; `diag/valves/<V>/profile` (only a new one) | `service_diag`, `check_profile`, `profile_crc` | `esp:glue/src/mqtt_client/tests_mut.rs::diag_valve_last_move_counters_inactive_and_unextended_valves`; `esp:glue/src/mqtt_client/tests_mut.rs::diag_four_valve_messages_per_pass_profiles_only_when_new`; `esp:glue/src/mqtt_client/tests_eq.rs::the_profile_crc_takes_the_fields_only` | deviation: the profile CRC over the fields with zero padding ([PORT-NOTES.md#glue](PORT-NOTES.md#glue)) |
| out: `diag/stm/*`, `diag/calibration/*`, `diag/mqtt/*` | `service_stm_diag`, `publish_paced` | `esp:glue/src/mqtt_client/tests_mut.rs::diag_stm_counters_link_protocol_lease_safe_mode_once_per_change`; `esp:glue/src/mqtt_client/tests.rs::diag_version_started_calibration_next_counters`; `esp:glue/src/mqtt_client/tests_mut.rs::diag_paced_counters_go_out_10_s_after_their_last_publish` | same |
| out: `events` (merge of one code on several valves) | `service_events`, `publish_event`; core `event_limiter` | `esp:glue/src/mqtt_client/tests.rs::events_12_valves_of_one_code_become_one_message`; `esp:glue/src/mqtt_client/tests_mut.rs::events_an_event_json_of_511_characters_goes_out_512_do_not_fit`; `esp:core/src/event_limiter/tests.rs::event_rate_limiter_hourly_token_bucket` | same |
| Client id (host name + eFuse MAC, or the configured one), credentials as a pair, clean session only after a topic change, a station rename | `apply_names`, `open_session`; core `build_mqtt_client_id` | `esp:glue/src/mqtt_client/tests.rs::a_configured_client_id_is_used_verbatim_ha_mode_subscriptions`; `esp:glue/src/mqtt_client/tests.rs::credentials_go_out_only_as_a_pair`; `esp:glue/src/mqtt_client/tests.rs::clean_session_only_after_a_topic_config_change`; `esp:glue/src/mqtt_client/tests.rs::a_station_renamed_at_run_time_reconnects_with_its_client_id_will_and_topics` | same |
| Subscriptions (targets QoS 1, `cmd/#` QoS 0, HA status), keepalive and -4, back-off 2..60 s | `subscribe`; `mqtt_conn.rs` `poll`; core `ReconnectPacer` | `esp:core/src/mqtt_topics/tests.rs::subscriptions_per_mode_separate_and_prefix`; `esp:glue/src/mqtt_conn/tests.rs::keepalive_pings_after_the_period_and_times_out_without_answer`; `esp:glue/src/mqtt_client/tests.rs::a_broker_that_drops_at_once_is_retried_at_2_4_8_s`; `esp:glue/tests/mqtt_interop.rs::a_broker_restart_is_noticed_and_reconnected` | same |
| PubSubClient 2.8 packets: CONNECT, CONNACK wait 5 s, 2304 B buffer, QoS 1 PUBACK, one packet per loop, oversized packets dropped | `esp:glue/src/mqtt_conn.rs` | `esp:glue/src/mqtt_conn/tests.rs::connect_sends_the_connect_packet_of_the_library`; `esp:glue/src/mqtt_conn/tests.rs::poll_delivers_one_message_per_call_and_acknowledges_qos_1`; `esp:glue/src/mqtt_conn/tests.rs::an_oversized_packet_is_read_and_dropped`; `esp:glue/tests/mqtt_interop.rs::qos1_inbound_is_delivered_and_acknowledged` | deviation: waits poll every 1 ms; two library bugs not reproduced ([PORT-NOTES.md#glue](PORT-NOTES.md#glue)) |
| Budgets per pass, inbound queue of 4 (33 B payloads), target latch, rejects counted and logged once per 10 s | `pass`, `drain_inbound`, `reject_command` | `esp:glue/src/mqtt_client/tests_mut.rs::full_publish_common_and_a_valve_one_valve_per_pass_four_other_slots_per_pass`; `esp:glue/src/mqtt_client/tests_mut.rs::inbound_payloads_are_cut_at_33_bytes`; `esp:glue/src/mqtt_client/tests.rs::a_refused_target_is_submitted_again_the_newest_wins`; `esp:glue/src/mqtt_client/tests.rs::rejections_are_counted_and_logged_once_per_10_s` | same |
| A pending restart: `offline` and DISCONNECT | `pass`, `disconnect_clean` | `esp:glue/src/mqtt_client/tests.rs::a_pending_restart_sends_offline_and_disconnects_cleanly` | same |
| HA: valve entities (valve, state, calibration texts, legacy diag texts, temp sensors, position, problem, failsafe, sync, calibrate button, newDiag counters) | core `ha_discovery` `describe_*`, `write_entity` | `esp:core/src/ha_discovery/tests.rs::discovery_keep_entities_carry_no_availability_k3_1_k4_1_w3_8`; `esp:core/src/ha_discovery/tests.rs::discovery_new_per_valve_entities_e29_1_w15_1`; `esp:core/src/ha_discovery/tests.rs::discovery_expire_after_is_three_publish_intervals_at_least_60_s` | same (four payloads pinned by topic only, gap O10) |
| HA: device entities, buttons, the event entity, the device block, availability, ids, the 2047 B limit | core `ha_discovery` | `esp:core/src/ha_discovery/tests.rs::discovery_device_entities_k3_2_e29_1_w15_2`; `esp:core/src/ha_discovery/tests.rs::discovery_device_block_with_hw_version_and_variants_w15_4`; `esp:core/src/ha_discovery/tests.rs::discovery_ha_safe_ids_raw_names_root_and_prefix_e20_2_h5`; `esp:core/src/ha_discovery/tests_mut.rs::a_payload_of_2047_bytes_goes_out_and_one_of_2048_does_not` | same |
| HA: the run (list file `/HADiscovery.cfg`, stale configs deleted, the 2.0.0 migration, the legacy DROP list, the triggers) | core `DiscoveryRun`; `ListPort` | `esp:core/src/ha_discovery/tests.rs::discovery_run_prune_before_publish_rewrite_the_list_w4_2_w4_3`; `esp:glue/src/mqtt_client/tests.rs::discovery_a_renamed_valve_loses_its_old_config_first`; `esp:glue/src/mqtt_client/tests.rs::discovery_the_2_0_0_migration_runs_once_in_ha_mode`; `esp:glue/src/mqtt_client/tests_gate.rs::gate_a_connect_before_the_stm_settled_waits_then_one_run`; QEMU `mqtt` | deviation: list written unbuffered, two heap blocks per run ([PORT-NOTES.md#glue](PORT-NOTES.md#glue)) |

### 3.3 UART protocol v1, v2, v3 (ESP <-> STM)

Every command of the ESP command table (40) has its STM handler in C++ and in Rust; the STM
also accepts `stsnx`, `stsny`, `gmotx` and `ESPalive`, which no ESP sends (in both); `gactp` and
`gmenc` are declared but implemented by neither. STM: `stm:glue/src/communication.rs`
(`dispatch`, `dispatch_v2`); ESP: `esp:core/src/stm_codec.rs` (`build_*`, `parse_*`). The v1
replies are compared as whole lines with CR LF against firmware 2.0.0.

| Command (protocol) | STM tests | ESP tests | Status |
|---|---|---|---|
| stgtp (1) | `stm:glue/src/communication/tests.rs::v1_replies_gvers_gproto_gtgtp_stgtp`; `stm:glue/src/communication/tests_mut.rs::stgtp_gtgtp_valve_11_positions_0_and_100_valve_12_is_refused` | `esp:core/src/stm_codec/tests.rs::build_set_target_cases`; `esp:core/src/stm_codec/tests.rs::ack_replies_golden` | same |
| gtgtp (1) | `stm:glue/src/communication/tests_mut.rs::stgtp_gtgtp_valve_11_positions_0_and_100_valve_12_is_refused` | `esp:core/src/stm_codec/tests.rs::single_valve_builders`; `esp:core/src/stm_codec/tests.rs::gtgtp` | same |
| gvlvd (1; renews the lease) | `stm:glue/src/communication/tests.rs::v1_replies_gvlvd_with_the_valve_and_its_sensors`; `stm:glue/src/communication/tests_mut.rs::gvlvd_and_gvlvx_report_temperatures_counts_and_clamps` | `esp:core/src/stm_codec/tests.rs::gvlvd_golden_and_calibrating_bit`; `esp:core/src/stm_codec/tests.rs::gvlvd_rejects_every_bad_field` | same |
| gvlst (1) | `stm:glue/src/communication/tests_v1.rs::v1_ghwin_masns_eepst_gvlst_reset`; `stm:glue/src/communication/tests_mut.rs::gvlst_lists_the_status_of_every_valve` | `esp:core/src/stm_codec/tests.rs::gvlst_with_and_without_the_v1_trailing_comma` | same |
| gonec, gowvc (1) | `stm:glue/src/communication/tests_v1.rs::v1_gonec_and_goned`; `stm:glue/src/communication/tests_v1.rs::v1_gowvc_and_gowvd`; `stm:glue/src/communication/tests_mut.rs::gonec_255_one_space_before_the_list_254_and_256_are_no_list` | `esp:core/src/stm_codec/tests.rs::gonec_gowvc_count_and_list_forms`; `esp:core/src/stm_codec/tests.rs::list_elements_beyond_the_count_are_rejected` | same |
| goned, gowvd (1) | `stm:glue/src/communication/tests_mut.rs::goned_and_gowvd_the_first_and_the_last_sensor_index` | `esp:core/src/stm_codec/tests.rs::bus_index_builders`; `esp:core/src/stm_codec/tests.rs::goned_gowvd_data_and_error_forms` | same |
| gvlon (1) | `stm:glue/src/communication/tests_v1.rs::v1_gvlon_for_one_valve_all_valves_and_an_invalid_index`; `stm:glue/src/communication/tests_mut.rs::gvlon_of_every_valve_lists_both_slots_with_commas` | `esp:core/src/stm_codec/tests.rs::gvlon_single_and_list`; `esp:core/src/stm_codec/tests.rs::v1_gvlon_error_arrives_with_the_goned_prefix` | same |
| stons, masns (1) | `stm:glue/src/communication/tests_v1.rs::v1_stons_stlnt_stlnm_gtlnm_staop_staln`; `stm:glue/src/communication/tests_v1.rs::v1_ghwin_masns_eepst_gvlst_reset` | `esp:core/src/stm_codec/tests.rs::builders_without_arguments_golden_lines`; `esp:core/src/stm_codec/tests.rs::ack_replies_golden` | same |
| stvls (1) | `stm:glue/src/communication/tests_v3.rs::stvls_both_slots_of_the_valve_each_marked_only_when_it_changes` | `esp:core/src/stm_codec/tests.rs::build_set_valve_sensors_cases`; `esp:core/src/stm_codec/tests.rs::stvls_ack_carries_the_valve` | same |
| staop, staln (1) | `stm:glue/src/communication/tests_mut.rs::staop_staln_stdet_0_and_65535_reach_the_handler` | `esp:core/src/stm_codec/tests.rs::valve_or_all_builders` | same |
| stdet (1) | `stm:glue/src/communication/tests_v3.rs::stdet_255_tests_every_valve_another_number_is_an_error_no_number_no_reply` | `esp:core/src/stm_codec/tests.rs::builders_without_arguments_golden_lines` | same |
| stlnm, gtlnm (1) | `stm:glue/src/communication/tests_v3.rs::stlnm_every_request_resets_the_counters_the_eeprom_is_marked_only_for_a_new_value`; `stm:glue/src/communication/tests_mut.rs::stlnt_0_and_stlnm_0_are_taken` | `esp:core/src/stm_codec/tests.rs::build_set_learn_movements_cases`; `esp:core/src/stm_codec/tests.rs::gtlnm_ghwin_eepst` | same |
| stlnt (1) | `stm:glue/src/communication/tests_v3.rs::stlnt_the_learn_time_goes_to_the_app_and_is_stored_only_when_it_changes` | `esp:core/src/stm_codec/tests.rs::build_set_learn_time_cases`; `esp:core/src/stm_codec/tests.rs::gtlnt_and_stlnt` | same |
| smotc, gmotc (1) | `stm:glue/src/communication/tests_v1.rs::v1_smotc_and_gmotc`; `stm:glue/src/communication/tests_mut.rs::smotc_and_scalx_store_only_a_change_slcfg_keeps_an_equal_timeout_unmarked` | `esp:core/src/stm_codec/tests.rs::build_set_motor_chars_cases`; `esp:core/src/stm_codec/tests.rs::gmotc_with_3_to_5_fields` | same |
| gvers (1) | `stm:glue/src/communication/tests_v3.rs::gproto_answers_3_gvers_carries_the_board_revision`; `stm:glue/src/system/tests_env.rs::gvers_and_the_banner_report_the_identity_of_the_image` | `esp:core/src/stm_codec/tests.rs::gvers_with_suffix_hw_and_build` | deviation: `2.2.0-revamped` ([GLUE-DESIGN-STM.md](GLUE-DESIGN-STM.md#8-open-decisions-and-risks) D8) |
| ghwin, eepst (1) | `stm:glue/src/communication/tests_mut.rs::ghwin_reports_the_device_id_of_the_chip`; `stm:glue/src/communication/tests_v1.rs::v1_ghwin_masns_eepst_gvlst_reset` | `esp:core/src/stm_codec/tests.rs::gtlnm_ghwin_eepst` | same |
| reset (1; 200 ms, then the reset) | `stm:glue/src/communication/tests_mut.rs::reset_the_reply_200_ms_for_it_to_leave_then_the_reset_request`; `stm:glue/src/system/tests_boot.rs::reset_answers_waits_for_the_eeprom_and_restarts_the_controller_software_reset` | `esp:core/src/stm_codec/tests.rs::builders_without_arguments_golden_lines` | same |
| gproto (2) | `stm:glue/src/communication/tests_v3.rs::gproto_answers_3_gvers_carries_the_board_revision` | `esp:core/src/stm_codec/tests.rs::gproto`; `esp:core/src/link_policy/tests.rs::only_the_gproto_builder_marks_its_request_as_a_probe` | same |
| gvlvx (2; renews the lease) | `stm:glue/src/communication/tests_mut.rs::gvlvd_gvlvx_gprof_gvlvy_valves_0_and_11_answer_12_does_not` | `esp:core/src/stm_codec/tests.rs::gvlvx_golden`; `esp:core/src/stm_codec/tests.rs::gvlvx_field_ranges` | same |
| gprof (2) | `stm:glue/src/communication/tests_mut.rs::gprof_sends_the_profile_of_the_valve`; `stm:glue/src/communication/tests_mut.rs::the_reply_buffer_holds_the_longest_gprof` | `esp:core/src/stm_codec/tests.rs::gprof` | same |
| svmov (2) | `stm:glue/src/communication/tests_mut.rs::svmov_checks_every_argument_range`; `stm:glue/src/communication/tests_mut.rs::svmov_valves_0_and_11_one_argument_reports_its_valve` | `esp:core/src/stm_codec/tests.rs::build_service_move_cases`; `esp:core/src/stm_codec/tests.rs::svmov_signed_index` | same |
| scalx, gcalx (2) | `stm:glue/src/communication/tests_v3.rs::scalx_the_escalation_is_stored_only_when_it_changes`; `stm:glue/src/communication/tests_v1.rs::v2_gprof_svmov_scalx_gcalx_gmotx` | `esp:core/src/stm_codec/tests.rs::build_set_breakaway_cases`; `esp:core/src/stm_codec/tests.rs::gcalx` | same |
| gstat (2) | `stm:glue/src/communication/tests.rs::v1_replies_gstat_counts_the_dropped_and_malformed_lines` | `esp:core/src/stm_codec/tests.rs::gstat` | same |
| gvlvy (3) | `stm:glue/src/communication/tests_v3.rs::gvlvy_the_golden_of_contracts_1_2_gvlvx_the_same_first_values_none_for_a_bad_index` | `esp:core/src/stm_codec/tests.rs::gvlvy_golden`; `esp:core/src/stm_codec/tests.rs::failsafe_position_check_order_of_gvlvy_and_glcfg` | same |
| gstax (3) | `stm:glue/src/communication/tests_v3.rs::gstax_the_golden_of_contracts_1_3_gstat_the_same_first_values`; `stm:glue/src/communication/tests_v3.rs::gstax_every_field_from_its_source` | `esp:core/src/stm_codec/tests.rs::gstax_golden_with_23_values` | deviation: the receive error counters are read one after the other ([PORT-NOTES-STM.md](PORT-NOTES-STM.md#glue-communication-srccommunicationcpp)) |
| slhbt (3) | `stm:glue/src/communication/tests_v3.rs::slhbt_0_and_1_reach_the_lease_the_reply_carries_its_state_and_remaining_time` | `esp:core/src/stm_codec/tests.rs::build_heartbeat_and_build_set_lease_timeout`; `esp:core/src/stm_codec/tests.rs::slhbt_ok_and_error_forms` | same |
| slcfg, glcfg (3) | `stm:glue/src/communication/tests_v3.rs::slcfg_0_and_5_to_1440_are_stored_and_configure_the_lease_the_rest_is_an_error`; `stm:glue/src/communication/tests_v3.rs::glcfg_the_lease_timeout_and_the_failsafe_positions_of_the_valves_extra_arguments_ignored` | `esp:core/src/stm_codec/tests.rs::slcfg_and_ssafe_ok_err_forms`; `esp:core/src/stm_codec/tests.rs::glcfg` | same |
| sfspo (3) | `stm:glue/src/communication/tests_v3.rs::sfspo_one_valve_or_all_0_to_100_or_255_stored_only_when_it_changes` | `esp:core/src/stm_codec/tests.rs::build_set_failsafe_cases`; `esp:core/src/stm_codec/tests.rs::sfspo_and_sstop_indexed_results` | same |
| sstop (3) | `stm:glue/src/communication/tests_v3.rs::sstop_one_valve_or_all_through_app_stop_an_invalid_index_or_a_refusal_is_an_error` | `esp:core/src/stm_codec/tests.rs::sfspo_and_sstop_indexed_results` | same |
| gtlnt, ssafe (3) | `stm:glue/src/communication/tests_v3.rs::gtlnt_the_stored_learn_time_extra_arguments_ignored`; `stm:glue/src/communication/tests_v3.rs::ssafe_only_0_leaves_safe_mode` | `esp:core/src/stm_codec/tests.rs::gtlnt_and_stlnt`; `esp:core/src/stm_codec/tests.rs::slcfg_and_ssafe_ok_err_forms` | same |
| stsnx, stsny, gmotx, ESPalive (STM only, in both) | `stm:glue/src/communication/tests_v3.rs::stsnx_stsny_the_slot_of_the_valve_is_marked_only_when_the_address_changes`; `stm:glue/src/communication/tests_v1.rs::v2_gprof_svmov_scalx_gcalx_gmotx`; `stm:glue/src/communication/tests_mut.rs::the_debug_lines_of_the_v1_commands` | — | same |
| An unknown command or bad arguments: no reply | `stm:glue/src/communication/tests.rs::communication_loop_an_unknown_command_and_bad_arguments_get_no_reply` | `esp:core/src/link_policy/tests.rs::error_forms_complete_as_rejected` | same |

| Behaviour | Rust | Tests | Status |
|---|---|---|---|
| STM framing: CR/LF, 127 characters (128 is an overflow), an idle partial line dropped after 100 ms, 4 requests and 512 bytes per call, more than 5 arguments counted | `communication.rs`; core `line_assembler`, `tokenizer` | `stm:glue/src/communication/tests_mut.rs::a_request_of_127_characters_is_answered_one_of_128_is_an_overflow`; `stm:glue/src/communication/tests.rs::communication_loop_an_idle_partial_line_is_dropped_after_more_than_100_ms`; `stm:glue/src/communication/tests.rs::communication_loop_at_most_4_requests_per_call_the_rest_stays_queued`; `stm:glue/src/communication/tests.rs::communication_loop_reads_at_most_512_bytes_per_call`; `stm:glue/src/communication/tests_mut.rs::a_line_of_too_many_arguments_is_counted_and_the_loop_goes_on` | same |
| STM: USART1 receive errors counted, the byte stored; boot-window bytes dropped; the lease poll | `stm:glue/src/serial.rs` `Port::on_irq`; `communication.rs` `setup` | `stm:glue/src/serial/tests.rs::rx_an_error_of_the_interrupt_is_counted_and_its_byte_stored`; `stm:glue/src/system/tests_env.rs::receive_errors_of_usart1_reach_gstax`; `stm:glue/src/communication/tests.rs::communication_setup_bytes_of_the_boot_window_dropped`; `stm:glue/src/communication/tests_v3.rs::gvlvd_and_gvlvx_renew_the_lease_through_app_lease_poll_gvlvy_and_the_rest_do_not` | same |
| ESP link: states, queue of 24 (merging, eviction), priorities, timeouts 400/1500/3000 ms, retries of idempotent commands only | `esp:core/src/link_policy.rs` | `esp:core/src/link_policy/tests.rs::degraded_after_one_timeout_down_after_five_up_after_a_reply`; `esp:core/src/link_policy/tests.rs::full_queue_eviction_of_the_newest_poll_entry`; `esp:core/src/link_policy/tests.rs::per_command_timeouts`; `esp:core/src/link_policy/tests.rs::actions_are_never_retried` | same |
| ESP: the R6 reset rule, the hold after an ESP boot, the gproto probe | `link_policy.rs`; `esp:core/src/poll_planner.rs`; `esp:glue/src/stm_link.rs` `pulse` | `esp:core/src/link_policy/tests.rs::r6_reset_at_most_once_per_10_minutes`; `esp:core/src/link_policy/tests.rs::hold_after_esp_boot_clears_the_failure_counter_and_blocks_r6`; `esp:core/src/link_policy/tests.rs::gproto_probe_timeouts_never_count_toward_the_failure_counter`; `esp:glue/src/stm_link/tests.rs::task_a_silent_stm_is_reset_by_the_link_policy` | same |
| ESP: replies up to 1023 characters and 40 tokens, unparsable lines counted | `stm_codec.rs` `parse_reply` | `esp:core/src/stm_codec/tests.rs::empty_blank_null_and_too_long_lines`; `esp:core/src/link_policy/tests.rs::parse_errors_are_counted_only` | same |
| ESP UART task: 4 commands and 512 bytes per pass, 2 ms pause, NRST pulses of 100 ms | `esp:glue/src/stm_link.rs` | `esp:glue/src/stm_link/tests.rs::task_one_pass_reads_at_most_512_bytes_from_the_uart`; `esp:glue/src/stm_link/tests_mut.rs::task_one_pass_takes_at_most_4_commands`; `esp:glue/src/stm_link/tests.rs::pulse_reset_nrst_high_for_100_ms` | same |

### 3.4 STM debug terminal (USART6)

`stm:glue/src/terminal.rs` (`execute`, `serve`, `supervise`; feature `terminal`, on in every
release image).

| Command | Tests | Status |
|---|---|---|
| help | `stm:glue/src/terminal/tests.rs::terminal_serve_no_complete_line_too_many_arguments_unknown_command` | same |
| learn | `stm:glue/src/terminal/tests.rs::terminal_serve_learn_open_and_close_hand_the_command_to_the_valve_machine`; `stm:glue/src/terminal/tests_mut.rs::learn_the_full_16_bit_range_a_refused_command_bad_arguments` | same |
| open, close | `stm:glue/src/terminal/tests.rs::terminal_serve_learn_open_and_close_hand_the_command_to_the_valve_machine`; `stm:glue/src/terminal/tests_mut.rs::open_and_close_100_percent_is_the_limit_a_refused_command_bad_arguments` | same |
| settar | `stm:glue/src/terminal/tests.rs::terminal_serve_settar_gvers_and_stdet_255`; `stm:glue/src/terminal/tests_mut.rs::settar_the_last_valve_and_100_percent_nothing_beyond_bad_arguments` | same |
| smux, sdir | `stm:glue/src/terminal/tests.rs::smux_and_sdir_only_while_the_valve_machine_is_idle`; `stm:glue/src/terminal/tests_mut.rs::smux_on_a_c1_board_drives_the_pin_high` | same |
| sena (2 s or 60 mA, the valve machine held) | `stm:glue/src/terminal/tests.rs::sena_the_output_and_the_valve_psu_on_off_after_2_s_the_valve_machine_gets_no_command`; `stm:glue/src/terminal/tests.rs::sena_off_at_once_above_60_ma_filtered_current_either_sign`; `stm:glue/src/terminal/tests.rs::sena_refused_while_the_valve_machine_works_or_in_safe_mode_0_switches_off_bad_arguments`; `stm:glue/src/system/tests_env.rs::sena_holds_the_valve_machine_for_2_s` | same |
| getone | `stm:glue/src/terminal/tests.rs::terminal_serve_getone_prints_the_sensor_data_stm_is_gone`; `stm:glue/src/system/tests_env.rs::one_wire_sensors_are_found_matched_to_a_valve_and_read` | same |
| seteep, saveep | `stm:glue/src/terminal/tests.rs::terminal_serve_seteep_sets_the_base_fields_and_marks_everything_saveep_marks_everything`; `stm:glue/src/terminal/tests.rs::terminal_serve_seteep_is_refused_while_the_eeprom_could_not_be_read` | same |
| stsnx, stsny | `stm:glue/src/terminal/tests_mut.rs::stsnx_and_stsny_slot_1_and_2_a_refused_index_bad_arguments` | same |
| stvls | `stm:glue/src/terminal/tests_mut.rs::stvls_both_addresses_to_the_valve_then_the_sensors_matched_errors` | same |
| gvlon | `stm:glue/src/terminal/tests_mut.rs::gvlon_the_addresses_of_the_last_valve_errors` | same |
| gvers, the banner | `stm:glue/src/terminal/tests.rs::terminal_serve_settar_gvers_and_stdet_255`; `stm:glue/src/terminal/tests.rs::terminal_init_banner_with_version_and_revision` | deviation: the version string (D8) |
| stons | `stm:glue/src/terminal/tests_mut.rs::gmotc_stons_and_one_line_per_call` | same |
| stlnt | `stm:glue/src/terminal/tests.rs::terminal_stlnt_stores_the_learn_time_through_comm_set_learntime_smotc_marks_only_a_change`; `stm:glue/src/terminal/tests_mut.rs::stlnt_0_s_is_a_valid_learn_time` | same |
| staop, staln | `stm:glue/src/terminal/tests_mut.rs::staop_and_staln_the_valve_echoed_a_refused_valve_bad_arguments` | same |
| smotc, gmotc | `stm:glue/src/terminal/tests_mut.rs::smotc_exactly_two_numbers`; `stm:glue/src/terminal/tests_mut.rs::gmotc_stons_and_one_line_per_call`; `stm:glue/src/system/tests_env.rs::the_terminal_reaches_the_modules` | same |
| stdet | `stm:glue/src/terminal/tests.rs::terminal_serve_settar_gvers_and_stdet_255`; `stm:glue/src/terminal/tests_mut.rs::smux_sdir_and_stdet_take_exactly_one_number` | same |
| An unknown command, too many arguments, lines of 127 characters, 256 bytes per call | `stm:glue/src/terminal/tests.rs::terminal_serve_no_complete_line_too_many_arguments_unknown_command`; `stm:glue/src/terminal/tests_mut.rs::terminal_serve_a_line_of_127_characters_is_a_command_one_of_128_is_dropped`; `stm:glue/src/terminal/tests_mut.rs::terminal_serve_at_most_256_bytes_are_taken_per_call` | same |
| The fault record of the start before (Rust only) | `stm:glue/src/terminal/tests_fault.rs::the_fault_record_is_one_line_with_the_registers_in_hex`; `stm:glue/src/system/tests_env.rs::a_fault_record_of_the_start_before_follows_the_banner`; Renode A6 | deviation ([GLUE-DESIGN-STM.md](GLUE-DESIGN-STM.md#55-faults) 5.5) |

### 3.5 Stored data, both directions

Every stored format is the C++ one (layouts compared field by field). The exchange itself is
proven by the QEMU scenarios `nvs` and `littlefs` (ESP; the C++ side read through its codec and
tools, gap O6) and by the C++ goldens and Renode A4/E9 (STM).

| Item | Rust | Tests | Status |
|---|---|---|---|
| NVS `cfg` (≤4096 B, the 2.0.0 layout, schema 1) | `esp:glue/src/storage.rs` `save_blobs`, `load_nvs`; core `encode_config` | `esp:core/src/config/tests.rs::the_cfg_blob_keeps_the_2_0_0_layout_with_a_neutral_web_login`; `esp:core/src/config/tests.rs::a_stored_web_login_loads_without_a_repair_and_is_dropped`; `esp:glue/src/storage/tests_config.rs::load_cfg_and_cfgx_are_used_no_event_the_backup_follows_once`; QEMU `nvs` (both ways) | same |
| NVS `cfgx` (≤1536 B, tags 1..9, unknown records kept) | core `encode_config_ext`, `decode_config_ext` | `esp:core/src/config/tests.rs::cfgx_layout_and_round_trip`; `esp:core/src/config/tests.rs::cfgx_unknown_records_are_kept_and_written_back`; `esp:glue/src/storage/tests_values.rs::load_a_single_unknown_cfgx_record_is_logged_as_a_newer_firmware`; QEMU `nvs` | same |
| NVS `boots` (u32), `calSlot` (u32), `lastCal` (i64), `haDrop`, `haLayout`, `imported`, `frLatch`, `otaStm` (u8) | `storage.rs` | `esp:glue/src/storage/tests_values.rs::values_keep_the_nvs_types_of_the_cpp_firmware_both_ways`; `esp:glue/src/storage/tests.rs::values_boot_count_calibration_slot_and_time_flags`; QEMU `nvs` (boots, imported, frLatch) | same |
| NVS `targets` (46 B "VDTG") | core `target_store` | `esp:core/src/target_store/tests.rs::targets_golden_encoding_and_round_trip`; `esp:core/src/target_store/tests.rs::targets_every_single_bit_flip_is_rejected_and_clears_the_output` | same |
| NVS `netTrial` (≤130 B "VDNT") | core `net_trial` | `esp:core/src/net_trial/tests.rs::encode_the_exact_layout_of_a_static_record`; `esp:core/src/net_trial/tests.rs::decode_rejects` | same |
| NVS `otaOk` (16 B), `otaTrial` (32 B), Rust only; kept by a factory reset | `esp:glue/src/boot_guard.rs` | `esp:glue/src/boot_guard/tests.rs::records_round_trip_and_damaged_ones_count_as_absent`; `esp:glue/src/boot_guard/tests.rs::a_rust_factory_reset_keeps_the_trial_a_cpp_one_restarts_validation`; `esp:glue/src/storage/tests.rs::factory_reset_keeps_the_boot_guard_records_the_next_boot_stays_confirmed` | deviation (GLUE-DESIGN-ESP.md 6.1, 6.5) |
| The legacy namespaces of 1.x (58 keys, integers of any width, strings, blobs), read once | `storage.rs` `LegacyReader`; core `legacy_import` | `esp:glue/src/storage/tests_values.rs::import_legacy_integers_of_every_width_are_read`; `esp:glue/src/storage/tests_values.rs::reader_strings_up_to_the_buffer_whole_a_longer_one_refused_empty_ones_absent`; `esp:core/src/legacy_import/tests.rs::the_import_report_document` | same |
| `/sys/cfg.bak`, `/sys/cfgx.bak` and the `.tmp` rename protocol | `esp:glue/src/storage/config_files.rs` | `esp:glue/src/storage/tests_config.rs::save_both_blobs_saved_the_backup_written_by_service`; `esp:glue/src/storage/tests_config.rs::load_a_backup_cut_between_its_renames_uses_cfgx_bak_tmp_one_cut_before_them_the_old_pair`; `esp:glue/src/storage/tests_config.rs::load_an_unusable_cfg_is_replaced_by_the_backup_nvs_rewritten`; QEMU `littlefs` (C++ to Rust) | same |
| `/sys/import.json` | `config_files.rs` `write_report` | `esp:glue/src/storage/tests_config.rs::load_the_legacy_import_writes_the_report_logs_dropped_features_removes_images` | same |
| `/log/events.log`, `/log/events.1.log` (64 KiB, 8 KiB slack) | `esp:glue/src/logger.rs` | `esp:glue/src/logger/tests.rs::file_info_events_wait_in_ram_for_the_5_min_flush_then_one_append`; `esp:glue/src/logger/tests.rs::file_the_file_rotates_to_events_1_log_at_64_kib`; `esp:glue/src/logger/tests.rs::file_a_blocked_rotation_grows_the_file_up_to_72_kib_then_stops`; QEMU `littlefs` (appends to the C++ log) | same |
| `/stm/<name>.bin`, `.bin.part`, `last_good.bin` | `esp:glue/src/storage/images.rs` | `esp:glue/src/storage/tests.rs::image_upload_the_part_file_becomes_the_image_and_is_indexed`; `esp:glue/src/storage/tests.rs::images_delete_and_the_last_good_copy_after_a_flash`; QEMU `littlefs` (a C++ image listed) | same |
| `/HADiscovery.cfg` (+ `.tmp`) | `esp:glue/src/mqtt_client.rs` `ListPort` | `esp:glue/src/mqtt_client/tests_mut.rs::discovery_the_list_is_read_through_the_512_b_buffer_of_the_run`; `esp:glue/src/mqtt_client/tests_mut.rs::discovery_a_list_that_cannot_be_committed_leaves_no_temporary_file` | deviation: unbuffered ([PORT-NOTES.md#glue](PORT-NOTES.md#glue)) |
| Legacy `/*.bin` images of 1.x, removed once | `esp:glue/src/storage/files.rs` `remove_legacy_images` | `esp:glue/src/storage/tests_values.rs::remove_legacy_images_a_legacy_image_with_the_longest_path_is_removed` | deviation: in batches of 8 paths on the heap ([PORT-NOTES.md#storage](PORT-NOTES.md#storage)) |
| LittleFS mount: disk version 2.0, formatted only after a failed mount | `storage.rs` `begin_fs`; `esp:firmware/sdkconfig.defaults` | `esp:glue/src/storage/tests_values.rs::begin_fs_a_partition_that_mounts_is_not_reported_as_formatted`; QEMU `badfs`, `littlefs` | same |
| RTC records: targets, lease emulation, VNWD, HA status; the guard's mirror (Rust only) | `esp:glue/src/app.rs` `RTC_*` | `esp:glue/src/app/tests.rs::rtc_layout_one_record_after_the_other`; `esp:glue/src/stm_service/tests.rs::record_sizes_are_the_cpp_formats`; `esp:glue/src/stm_service/tests.rs::the_rtc_copies_survive_a_software_restart_and_win_over_nvs` | deviation: other offsets, lost on a switch between the firmwares ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations)) |
| Config schema: 66 key paths with kind, range, rule, cfgx tag and restart flag; the JSON export without secrets; the restart reasons of a change | core `config` | `esp:core/src/config/tests.rs::string_keys_after_2_0_0_and_their_rules`; `esp:core/src/config/tests.rs::failsafe_keys_accept_exactly_their_range`; `esp:core/src/config/tests.rs::json_export_without_secrets_and_the_apply_members`; `esp:core/src/config/tests.rs::restart_reasons` | same |
| STM EEPROM: the 1.x layout (309 B) | `stm:core/src/legacy_layout.rs` | `stm:core/src/legacy_layout/tests.rs::encode_legacy_layout_byte_order_of_the_58632d6_writer`; `stm:core/src/legacy_layout/tests.rs::decode_legacy_layout_the_golden_image_gives_back_every_field` | same |
| STM EEPROM: block A v3, block B, the calibration records | `stm:core/src/eeprom_layout.rs`, `config_blocks.rs` | `stm:core/src/eeprom_layout/tests.rs::encode_extension_version_3_block`; `stm:core/src/config_blocks/tests.rs::encode_safety_byte_layout`; `stm:core/src/config_blocks/tests.rs::encode_calib_byte_layout` | same |
| STM EEPROM: the write order, the load and the repairs | `stm:glue/src/eeprom.rs`; core `config_store` | `stm:core/src/config_store/tests.rs::resolve_config_a_consistent_blocks_load_unchanged_nothing_to_write`; `stm:glue/src/system/tests_io.rs::after_the_first_start_and_changes_of_every_block_the_next_start_loads_without_flags`; the 27 C++ goldens | same |
| STM no-init cells at the C++ 2.1.7 addresses: warm state (180 B), reset guard (20 B), reset counter (12 B) | `stm:boot/src/capture.rs`; core `warm_state`, `reset_guard`, `system_stats` | `stm:boot/src/capture/tests.rs::layout_constants_are_the_cpp_2_1_7_addresses`; `stm:boot/src/capture/tests.rs::a_cpp_written_cell_is_read_and_continued`; `stm:glue/src/app/tests_warm.rs::the_bytes_follow_the_cpp_layout_and_carry_the_crc_of_the_core`; image check C4; Renode E9, A4 | same |
| STM fault record (Rust only) | `stm:boot/src/fault_record.rs` | `stm:boot/src/fault_record/tests.rs::the_words_are_magic_fields_count_and_the_inverted_xor`; Renode A6 | deviation (GLUE-DESIGN-STM.md 5.5, D4) |

### 3.6 Events and restart reasons

The 87 event codes (number, name, severity, MQTT class, message template) are the C++ table
entry by entry; restart reasons 0..6 have the C++ names and severities.

| Item | Rust | Tests | Status |
|---|---|---|---|
| Event registry, messages, JSON, syslog lines | `esp:core/src/event_log.rs` `CODES` | `esp:core/src/event_log/tests.rs::mqtt_event_json_single_and_aggregate`; the message table of `esp:core/src/event_log/tests.rs` | same |
| Restart reasons 0..6 (user, ota, net watchdog, factory reset, rollback, network revert, heap guard) and their severity | `event_log.rs` `REBOOT_REASONS`; `esp:glue/src/ota.rs` `reboot_event` | `esp:glue/src/ota/tests.rs::request_restart_the_first_request_wins_severity_per_reason` | same |
| Reason 7 (switch back) and event 107 arg1 -4 (the previous image failed its trial) | `event_log.rs`; `boot_guard.rs` | `esp:core/src/event_log/tests.rs::switch_back_restart_and_trial_failure_are_the_glue_design_contract`; `esp:glue/src/boot_guard/tests.rs::constants_and_events` | deviation ([PORT-NOTES.md#event_log](PORT-NOTES.md#event_log)) |

### 3.7 ESP OTA and the boot guard

| Feature | Rust | Tests | Status |
|---|---|---|---|
| Upload checks: busy, STM flash or image upload, partition, size + 16 KiB, MD5 form | `esp:glue/src/ota.rs` `upload_begin`; core `ota_policy` | `esp:glue/src/ota/tests.rs::upload_begin_refused_while_an_stm_flash_or_image_upload_runs_busy_while_an_upload_runs`; `esp:glue/src/ota/tests.rs::upload_begin_one_byte_more_is_refused_before_the_update_starts`; `esp:core/src/ota_policy/tests.rs::normalize_md5_cases` | same |
| The `Update` contract: error codes and texts, MD5 compare, image selection | `esp:glue/src/ota/update.rs` | `esp:glue/src/ota/update/tests.rs::error_texts_are_the_ones_of_updater_cpp`; `esp:glue/src/ota/update/tests.rs::md5_over_the_written_bytes_is_compared_as_given`; `esp:glue/src/ota/update/tests.rs::a_failed_last_sector_never_selects_what_an_earlier_update_left_in_the_slot`; QEMU `ota` | deviation: a failed update never selects a slot ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations), ota/update) |
| Restart path: no restart during an STM flash, STM EEPROM gate, targets flushed, event 323, `otaStm`, log flush | `ota.rs` `service_restart`; core `restart_gate` | `esp:glue/src/ota/tests.rs::service_restart_stm_save_target_flush_log_flush_then_esp_restart`; `esp:glue/src/ota/tests.rs::service_restart_never_in_the_middle_of_an_stm_flash`; `esp:glue/src/ota/tests.rs::service_restart_a_silent_stm_task_restarts_at_12000_ms_with_event_323` | same |
| Validation of a new image (net, HTTP self-check, STM): 120 s confirms, 15 min gives up | `ota.rs` `service`; core `OtaValidator` | `esp:glue/src/ota/tests.rs::service_self_check_200_net_and_link_confirm_120_s_after_the_first_healthy_second`; `esp:glue/src/ota/tests.rs::service_a_503_self_check_switches_back_at_900_s_through_the_restart_path`; QEMU `boot`, `health` | deviation: the rollback is the boot guard's switch; `otaStm` applies for the whole trial ([PORT-NOTES.md#ota](PORT-NOTES.md#ota)) |
| Boot guard: trial, 3 counted boots, 60 s boot deadline, switch back, no ping-pong, no fallback confirms | `esp:glue/src/boot_guard.rs`; `esp:glue/src/app.rs` `boot_deadline` | `esp:glue/src/boot_guard/tests.rs::a_crash_loop_switches_back_after_three_boots`; `esp:glue/src/boot_guard/tests.rs::the_image_that_failed_the_last_trial_is_never_switched_to_again`; `esp:glue/src/app/tests.rs::boot_deadline_restarts_a_boot_that_never_reached_the_app_thread`; QEMU `boot`, `rollback`, `deadline` | deviation: the devices' bootloader has no rollback (GLUE-DESIGN-ESP.md 6) |
| An ESP upload while the image is on trial | `ota.rs`, `uploads.rs` | `esp:glue/src/ota/tests.rs::upload_begin_is_refused_while_the_image_is_on_trial`; QEMU `boot` | deviation (decision 7.5) |
| Failure before `main` (bootloader hand-off, IDF start-up) | none (cannot be caught by the image) | QEMU boot chain with the devices' bootloader | hardware (H1) |

### 3.8 STM images and flashing

| Feature | Rust | Tests | Status |
|---|---|---|---|
| Image upload, name rules, 3 + 1 slots, 512 KiB, CRC, scan of the markers and the board tag | `esp:glue/src/storage/images.rs`; core `stm_flasher` `validate_image` | `esp:glue/src/storage/tests.rs::image_upload_bad_names_last_good_busy_too_many_images`; `esp:glue/src/storage/tests_images.rs::upload_exactly_512_kib_is_accepted_one_byte_more_removes_the_part`; `esp:core/src/stm_flasher/tests.rs::validate_handshake_strings_also_across_chunk_boundaries`; image check C1-C3 | same |
| Board check of the running STM, `force` | core `check_board`; `web_server.rs` `board_refused` | `esp:core/src/stm_flasher/tests.rs::a_c2_image_on_a_c1_board_fails_in_validating_before_any_reset`; `esp:glue/src/stm_link/tests.rs::task_a_flash_of_an_image_for_another_board_is_refused_before_any_reset` | same |
| last_good copy after a flash | `images.rs` `service_copy` | `esp:glue/src/storage/tests_images.rs::last_good_copy_one_byte_short_of_the_space_logs_and_copies_nothing` | same |
| Flash start waits for the STM EEPROM; refused while a restart is pending | core `stm_session` `request_flash` | `esp:core/src/stm_session/tests.rs::flash_requests_busy_restart_pending_unreadable_image_abort_while_waiting`; `esp:glue/src/stm_link/tests.rs::task_a_missing_image_and_a_pending_restart_refuse_the_flash` | same |
| AN3155 flasher: reset and handshake (DEADBEEF/BEEFIT), sync, GET, GET ID, erase, write, verify, waiting for the application, blank mode, abort | `esp:core/src/stm_flasher.rs` | `esp:core/src/stm_flasher/tests.rs::handshake_resyncs_the_v1_stms_fixed_8_byte_matcher`; `esp:core/src/stm_flasher/tests.rs::a_bootloader_that_only_syncs_at_57600_gets_a_second_session`; `esp:core/src/stm_flasher/tests.rs::the_new_application_may_take_up_to_60_s`; `esp:glue/src/stm_link/tests_mut.rs::task_a_blank_flash_runs_through_the_uart_to_the_new_firmware`; Renode E11 | same |
| Erase and write order: sector 0 last (images above 16 KiB) | `stm_flasher.rs` `pass_blocks`, `pass_base` | `esp:core/src/stm_flasher/tests_d9.rs::sector_0_is_erased_written_and_verified_last`; `esp:core/src/stm_flasher/tests_d9.rs::an_image_of_16_kib_is_flashed_in_one_pass_as_in_cpp`; Renode E11 | deviation: D9 ([PORT-NOTES.md](PORT-NOTES.md#intended-deviations)) |
| STM boot window: setup order, 3001 calls, fixed 8-byte blocks, BEEFIT, the jump, LED, 8E1 | `stm:boot/src/stage.rs`, `window.rs`; `stm:firmware/src/boot_hw.rs` | `stm:boot/src/stage/tests.rs::the_steps_run_in_the_order_of_the_cpp_setup`; `stm:boot/src/window/tests.rs::without_deadbeef_the_window_ends_at_call_3001_and_call_3002_starts_the_application`; `stm:boot/src/window/tests.rs::deadbeef_answers_beefit_and_jumps_into_the_bootloader`; `stm:boot/src/stage/tests_esp.rs::a_stray_byte_after_the_drop_realigns_on_the_8th_send`; Renode E1-E5, E10 | same |
| HSE within 5 ms else HSI; application PLL from HSI | `stage.rs` `probe_hse`; `stm:firmware/src/clocks.rs` | `stm:boot/src/stage/tests.rs::a_dead_hse_is_switched_off_after_5_ms_and_the_window_runs_on_hsi`; Renode E6 | deviation: D1, D5 (GLUE-DESIGN-STM.md 5.3) |
| Faults: outputs off, fault record, IWDG reset | `stm:firmware/src/fault.rs`; `stm:boot/src/fault_record.rs` | `stm:boot/src/fault_record/tests.rs::the_count_continues_a_valid_record_and_starts_at_1_otherwise`; Renode E7, E8 | deviation: D4 (GLUE-DESIGN-STM.md 5.5) |
| Reset capture, safe mode (3 watchdog resets in 10 min, leave after 30 min or `ssafe 0`) | `capture.rs`; core `reset_guard`; `stm:glue/src/sysstat.rs` | `stm:boot/src/capture/tests.rs::s9_three_watchdog_resets_within_10_min_enter_safe_mode`; `stm:glue/src/sysstat/tests.rs::s9_ssafe_0_leaves_safe_mode_and_clears_the_window_a_power_on_clears_it_too`; Renode E9, A5 | same |
| Entry into the real ROM bootloader, real erase and program timing, the HSE start-up | `boot_hw.rs` `jump_to_bootloader` | Renode E1/E11 with a model of the ROM | hardware (H3) |

### 3.9 Network (ESP)

| Feature | Rust | Tests | Status |
|---|---|---|---|
| Interface choice: Ethernet, WiFi, Auto with WiFi after 30 s and back | `esp:glue/src/net.rs` `service` | `esp:glue/src/net/tests.rs::service_wifi_as_fallback_after_30_s_without_ethernet`; `esp:glue/src/net/tests_edges.rs::auto_fallback_to_wifi_and_back_to_ethernet`; `esp:glue/src/net/tests_edges.rs::auto_with_an_ethernet_driver_that_failed_starts_wifi_at_once` | same |
| WiFi back-off 5 s doubling to 60 s, one retry after the first unrequested disconnect | `net.rs` `retry_wifi_once` | `esp:glue/src/net/tests_edges.rs::wifi_retries_back_off_from_5_s_doubling_up_to_60_s`; `esp:glue/src/net/tests_edges.rs::wifi_one_retry_after_the_first_unrequested_disconnect_since_boot` | deviation: the retry runs in the next pass ([PORT-NOTES.md#net](PORT-NOTES.md#net)) |
| Static IP and DNS (0 means the gateway), host name | `net.rs` `ip_setup` | `esp:glue/src/net/tests.rs::begin_a_static_address_is_configured_on_ethernet`; `esp:glue/src/net/tests.rs::begin_a_static_address_without_dns_uses_the_gateway_as_dns`; `esp:glue/src/net/tests.rs::eth_start_sets_the_host_name_of_the_ethernet_interface` | same |
| NetUp/NetDown events, reconnect count; reachability evidence and the gateway ping | `net.rs` `refresh_info`, `probe_gateway`; core `net_policy` | `esp:glue/src/net/tests.rs::service_ethernet_with_an_address_is_up_net_up_names_the_address`; `esp:glue/src/net/tests.rs::reachability_dhcp_lease_mqtt_sntp_and_lan_http_prove_the_network`; `esp:core/src/net_policy/tests.rs::reachability_ip_down_then_up_probe_ping_reply_staleness_lost_regained` | same |
| SNTP with one server and the POSIX TZ | `net.rs` `apply_time` | `esp:glue/src/net/tests.rs::begin_sntp_with_the_configured_server_and_the_posix_tz_string`; `esp:glue/src/net/tests.rs::begin_without_an_ntp_server_sntp_is_stopped_and_only_tz_is_set` | deviation: the last-sync epoch reaches /api/status one pass later ([PORT-NOTES.md#net](PORT-NOTES.md#net)) |
| Network watchdog: interface restart (212), ESP restart (reason 2), VNWD count | `net.rs` `restart_interface` | `esp:glue/src/net/tests.rs::watchdog_gateway_silent_unreachable_interface_restart_esp_restart`; `esp:glue/src/net/tests_boots.rs::the_watchdog_wait_grows_per_restart_of_one_outage_and_a_power_cycle_starts_over`; QEMU `netwatch` | same |
| Network trial: start, confirm, revert, revert at boot | `net.rs` `begin_trial`, `revert_trial`; core `net_trial` | `esp:glue/src/net/tests.rs::trial_boot_with_an_armed_record_runs_it_no_confirm_reverts_120_s_after_the_ip`; `esp:glue/src/net/tests_boots.rs::a_reset_during_the_trial_reverts_at_the_next_boot_before_the_interfaces_start` | same |
| Inbound filter (loopback, own address) | `NetShared::note_inbound_http` | `esp:glue/src/net/tests_edges.rs::shared_requests_and_the_inbound_filter` | same |
| mDNS | none: C++ 2.1.7 has none either (its test asserts none) | — | same |
| LAN8720, WiFi radio, SNTP and ping on a LAN | `esp:firmware/src/adapters/` `eth.rs`, `wifi.rs`, `sntp.rs`, `ping.rs` | QEMU (OpenETH only, no WiFi model) | hardware (H2) |

### 3.10 Lease, failsafe, calibration, targets

| Feature | Rust | Tests | Status |
|---|---|---|---|
| ESP lease client: heartbeat, glcfg compare, slcfg/sfspo push, emulation | `esp:core/src/lease_client.rs` | `esp:core/src/lease_client/tests.rs::heartbeat_every_60_s_after_the_last_one_completed_at_once_on_an_alive_change`; `esp:core/src/lease_client/tests.rs::push_order_slcfg_sfspo_per_valve_then_verify`; `esp:core/src/lease_client/tests.rs::emulation_after_timeout_min_without_the_regulator_valves_with_a_position` | same |
| STM lease and failsafe positions | `stm:core/src/lease.rs`, `failsafe.rs`; `stm:glue/src/app.rs` | `stm:core/src/lease/tests_class.rs::runs_down_to_the_second_expires_and_is_renewed_by_slhbt_1_only`; `stm:core/src/failsafe/tests_drive.rs::drive_target_truth_table_over_status_failsafe_lease_and_assembly_hold`; `stm:glue/src/app/tests_v3.rs::failsafe_k1_4_an_expired_lease_drives_the_valves_to_their_failsafe_positions`; Renode A3 | same |
| Calibration schedule and learn time | `esp:core/src/calib_schedule.rs`; `esp:glue/src/stm_service.rs` | `esp:glue/src/stm_service/tests.rs::the_stms_confirmation_books_the_slot_and_logs_it`; `esp:glue/src/stm_service/tests.rs::no_reply_is_reported_and_the_slot_fires_again_10_min_later`; `esp:core/src/calib_schedule/tests_link.rs::learn_time_0_while_the_esp_schedule_is_on_else_one_week` | same |
| Desired targets across ESP restarts (RTC wins over NVS, flushed before a restart) | core `target_store`; `stm_service.rs` | `esp:glue/src/stm_service/tests.rs::the_rtc_copies_survive_a_software_restart_and_win_over_nvs`; `esp:glue/src/stm_service/tests.rs::flush_for_restart_writes_dirty_targets_at_once_nothing_when_clean`; `esp:glue/src/app/wiring/tests.rs::the_heap_guard_restart_keeps_the_desired_targets_across_the_reboot` | same |
| STM warm restore across STM resets | `stm:glue/src/app.rs` `app_warm_save`, `app_restore` | `stm:glue/src/app/tests_v3.rs::app_restore_w2_k1_8_a_warm_reset_restores_positions_holds_retries_and_the_expired_lease`; Renode A4, E9 | same |

### 3.11 Board I/O

| Feature | Rust | Tests | Status |
|---|---|---|---|
| ESP factory pin GPIO2 (5 s, latch `frLatch`, event 113, latch cleared when released) | `esp:glue/src/app.rs` `check_factory_pin`, `check_factory_latch`; core `factory_reset` | `esp:glue/src/app/tests.rs::setup_gpio2_held_low_for_5_s_resets_the_configuration_and_sets_the_latch`; `esp:glue/src/app/tests.rs::setup_gpio2_low_for_3_s_then_high_does_not_reset`; `esp:glue/src/app/tests.rs::task_the_latch_is_cleared_once_when_the_pin_goes_high_at_run_time` | same |
| HTTP factory reset | `web_server.rs` `factory_reset`; `storage.rs` `factory_reset` | `esp:glue/src/storage/tests.rs::factory_reset_vdmrev_is_erased_the_latch_is_kept_imported_is_set` | deviation: keeps `otaOk`/`otaTrial` ([PORT-NOTES.md#storage](PORT-NOTES.md#storage)) |
| NRST (IO15) and BOOT0 (IO14): released first at boot, 100 ms pulses | `esp:glue/src/stm_link.rs` `release_stm_reset`, `pulse` | `esp:glue/src/stm_link/tests.rs::release_reset_boot0_low_before_nrst_is_released_both_driven`; `esp:glue/src/app/wiring/tests.rs::boot_releases_the_stm_first_then_runs_the_boot_order_over_the_real_modules` | same |
| STM LED PC13 (off at tick 30, on at 31), button PB2 report | `stm:glue/src/main_loop.rs` | `stm:glue/src/main_loop/tests.rs::loop_system_the_led_goes_off_at_the_30th_and_on_at_the_31st_100_ms_tick`; `stm:glue/src/main_loop/tests.rs::loop_system_the_button_is_reported_every_other_100_ms_tick_while_it_reads_high` | same |
| STM valve PSU and enables safe at reset | `stm:glue/src/motor.rs` `valve_pins_safe`; `boot_hw.rs` | `stm:glue/src/motor/tests.rs::valve_pins_safe_presets_the_psu_latch_off_and_the_enables_low`; `stm:boot/src/stage/tests.rs::the_capture_is_written_back_before_the_outputs_and_the_window`; Renode E7, E8 | same |
| Watchdogs: ESP task watchdog fed by the threads; STM IWDG fed only while the valve timer runs | `app.rs`, `stm_link.rs`; `main_loop.rs` | `esp:glue/src/stm_link/tests_mut.rs::task_a_pass_returns_the_2_ms_delay_and_feeds_the_watchdog`; `stm:glue/src/main_loop/tests.rs::loop_system_the_watchdog_is_fed_only_while_the_valve_timer_makes_progress`; Renode A5 | same |
| ESP pin, UART and RTC adapters; the factory pin on the board | `esp:firmware/src/adapters/gpio.rs`, `system.rs` | — | hardware (H2) |

## 4. Gaps

### 4.1 Closed in this audit

| Commit | Gap | Kind |
|---|---|---|
| `e6f8347` | `test_smoke.cpp` (8 cases) had no port and no note | missing tests |
| `0c478ce` | the harness case of the fake STM had no Rust case | missing test |
| `3aea221` | GET /valves, /temps, /volts with a body read the whole body into a block of its length (C++: at most 8 KB, then 200): a LAN client could take a block as large as the free heap, and a refused block answered 503 | undocumented deviation, fixed in the glue |
| `9775d45` | no case of the 127/128-character edge of the STM line buffers (ESP UART and terminal; the C++ suites neither) | missing tests |
| `9d8d8af` | gstax reads the receive error counters one after the other (C++ with interrupts off); two comments said otherwise | undocumented deviation, documented |
| `13ede86`, `2557330`, `7560a0e` | no case of the query `MD5` of an ESP upload, of NVS values in the C++ types both ways, of the MQTT options at task level and of a station rename at run time (the C++ suites neither) | missing tests |
| `bff0191` | PORT-NOTES.md: rows that a blank line cut off from their tables, the testkit section, the RTC layout as an intended deviation, the meaning of `otaStm`; PORT-NOTES-STM.md: the C++ tests without a Rust form; GLUE-DESIGN-ESP.md 4.3, 4.6, 6.1 and GLUE-DESIGN-STM.md 4.2, 6.1 where the implementation differs from the design text | documentation |

### 4.2 Open

| # | Gap | Evidence | Estimate |
|---|---|---|---|
| O1 | The ESP reports `2.1.7-revamped-rust` (`esp:firmware/.cargo/config.toml` `VDM_VERSION`, workspace version `2.1.7-revamped`); decision D8 says `2.2.0-revamped` for the first Rust release of ESP and STM, and the STM follows it. | GLUE-DESIGN-STM.md 8 D8; `software_stm32_rust/Cargo.toml` | 0.25 h (firmware config; operator's decision which string) |
| O2 | `the_dashboard_is_every_file_of_web_gzipped_with_its_etag` is behind `#[cfg(feature = "dashboard")]` and no script runs it (`docker.sh test` has no features); it passed when run by hand. | `esp:glue/src/web_server/tests_rust.rs` | 0.5 h (one more `cargo test` in `docker.sh test` or CI) |
| O3 | No Rust job in `.github/workflows/build.yml` at `92718f5` (host tests, `parity.py --check`, interop, QEMU, Renode, image check). | `.github/workflows/build.yml` | 2-4 h (none if a later CI commit covers it) |
| O4 | The drivers of the differential checks quoted in PORT-NOTES.md (stm_codec 460,206 lines, stm_session 3,804,631 trace lines, config, event_log, the MQTT modules, net_trial, legacy_import, json_body) are not in the repository; the figures cannot be run again. | PORT-NOTES.md | 6-10 h |
| O5 | No test runs the Rust ESP session against the Rust STM glue; each side is proven against the C++ (goldens, fake STM). | `esp:core/src/test_support/stm_golden.rs`; `stm:glue/tests/golden/` | 6-10 h (a host harness over both workspaces) |
| O6 | QEMU scenarios narrower than GLUE-DESIGN-ESP.md 7 item 2 and 5.5 state: `nvs` crosses 5 of the 12 C++ keys (`otaStm` only in the optional `health`) and does not check the integer types; `littlefs` has no `cfgx.bak` and its Rust-to-C++ backup check reads the unchanged C++ file; `api` compares document keys only and sends no guard refusal; no SNTP sync, ping reply, static IP, factory pin or RTC record checked; `health` not in the default set. | `tools/rust/esp/qemu/harness.py` | 6-10 h |
| O7 | The QEMU, Renode and interop results are not archived; the runs of 2026-10-07 are quoted from the design documents. | GLUE-DESIGN-ESP.md 5.5, GLUE-DESIGN-STM.md 5.7 | 2-3 h (run and archive) |
| O8 | The C++ contract documents do not name the Rust-only additions: the switch-back route, 409 "image on trial", reason 7, event 107 arg1 -4, NVS `otaOk`/`otaTrial` (API.md, DESIGN.md 9 and 13); API.md still lists the 503 causes the Rust never sends. Decision 7.6 leaves the C++ side to the operator. | docs/revamped/API.md; DESIGN.md | 1-3 h after the decision |
| O9 | A consistent copy of the four gstax receive error counters (documented now). | PORT-NOTES-STM.md | 0.5-1 h |
| O10 | The HA payloads of `message`, `uptime`, `valves_calibration_repetitions` and `valves_temp2` are pinned by topic and order, not byte for byte (in C++ neither). | `esp:core/src/ha_discovery/tests.rs` | 0.5 h |
| O11 | Renode checks no STM LED or button; E11 checks the flash and `gvers` only, not the no-init cells of the real C++ images across C++ <-> Rust flashes. | `software_stm32_rust/renode/` | 3-4 h |
| O12 | The TCP adapter of the MQTT client has no host test (QEMU `mqtt` covers it). | `esp:firmware/src/adapters/tcp.rs` | 1-2 h |
| O13 | The mutation gate of the files the audit changed (section 5). | — | the gate's run time |

### 4.3 Hardware only

- **H1 ESP boot and OTA on a device:** an image that fails before `main` (bootloader hand-off,
  IDF start-up), the first OTA from C++ to Rust, a software restart after more than 60 s of
  uptime (a QEMU limit), the C++ firmware reading the NVS and LittleFS the Rust wrote
  (GLUE-DESIGN-ESP.md 5.5 "Not provable in QEMU", 6.6).
- **H2 ESP adapters:** LAN8720 (RMII clock on GPIO0, PHY reset on GPIO16), WiFi, UART2 to the STM
  and IO15/IO14, the factory pin IO2, RTC retention, SNTP and ping on a LAN, stack and heap
  figures of the device.
- **H3 STM on a chip:** entry into the real ROM bootloader (VTOR, peripheral state, risk R2), real
  erase and program timing, HSE start-up and HSI baud error (R11), IWDG LSI spread, 1-Wire slot
  timing (R5), the first C++ -> Rust STM flash with a working HSE (R12), an interrupted flash
  during the sector-0 pass (GLUE-DESIGN-STM.md 5.9, 5.10, D10).

## 5. Verification of the audit's commits

`bash tools/rust/docker.sh test software_esp32_rust` and `software_stm32_rust` with
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` after every
commit, all green: ESP core 1229 passed, glue 971 passed and 1 ignored (+ 6 interop tests that need
Mosquitto); STM boot 67, core 412, glue 494, image check 4.

Files for the mutation gate (code that cargo-mutants mutates): `esp:glue/src/web_server.rs`
(`serve_legacy`). Every other change of the audit is a test file, a comment or a document.
