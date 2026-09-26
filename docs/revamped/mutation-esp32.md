# Mutation testing: esp32

Scope: `software_esp32_revamped/lib/core`, ESP32 core (pure logic).
Target: >= 95 % overall and >= 95 % for every file.
Result: **97.68 % overall (13421 / 13740 killed), lowest file `lib/core/src/file_manager.cpp` 95.00 %, gate passed.**
Measured on commit `c79fecd` (2026-09-26), 6 workers, 44 min.

Tool: `tools/mutation/mutate.py`, config `tools/mutation/esp32.json`. Every mutant is built
with the sanitizer build of the native tests (ASan + UBSan, `-Werror`) and runs the tests of its
module first, then the whole suite. A failed test, a crash or a timeout kills the mutant.
Not counted: stillborn mutants (they do not compile), mutants in code the native build does not
compile, and equivalent mutants. The 351 equivalent mutants are listed with a reason in
`tools/mutation/equivalents/esp32/`; lines marked `// NOMUTATE` get no mutants.

## Reproduce

```sh
bash tools/native/docker.sh mutate esp32            # this suite
bash tools/native/docker.sh mutate esp32 --files <file>
bash tools/native/docker.sh mutate-all                 # all four suites and the gate table
```

## Per file

| file | score | killed | survived | timeout | equivalent | stillborn | not compiled |
|---|---|---|---|---|---|---|---|
| `lib/core/src/auth.cpp` | 98.15 % | 319 | 6 | 0 | 7 | 5 | 0 |
| `lib/core/src/calib_schedule.cpp` | 98.22 % | 553 | 10 | 0 | 0 | 14 | 0 |
| `lib/core/src/common.cpp` | 96.57 % | 725 | 26 | 6 | 0 | 15 | 0 |
| `lib/core/src/config.cpp` | 95.11 % | 2045 | 106 | 18 | 0 | 195 | 0 |
| `lib/core/src/event_limiter.cpp` | 96.84 % | 92 | 3 | 0 | 0 | 0 | 0 |
| `lib/core/src/event_log.cpp` | 98.33 % | 527 | 9 | 3 | 0 | 4 | 0 |
| `lib/core/src/factory_reset.cpp` | 100.00 % | 10 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/failsafe.cpp` | 100.00 % | 24 | 0 | 0 | 0 | 24 | 0 |
| `lib/core/src/file_manager.cpp` | 95.00 % | 113 | 6 | 1 | 0 | 20 | 0 |
| `lib/core/src/ha_discovery.cpp` | 98.70 % | 680 | 9 | 3 | 92 | 195 | 0 |
| `lib/core/src/health_monitor.cpp` | 98.11 % | 208 | 4 | 0 | 0 | 0 | 0 |
| `lib/core/src/image_store.cpp` | 100.00 % | 76 | 0 | 0 | 2 | 0 | 0 |
| `lib/core/src/json_api.cpp` | 100.00 % | 336 | 0 | 0 | 22 | 13 | 0 |
| `lib/core/src/json_writer.cpp` | 97.09 % | 267 | 8 | 0 | 8 | 2 | 0 |
| `lib/core/src/lease_client.cpp` | 99.58 % | 479 | 2 | 0 | 12 | 8 | 0 |
| `lib/core/src/legacy_http.cpp` | 100.00 % | 78 | 0 | 0 | 2 | 6 | 0 |
| `lib/core/src/legacy_import.cpp` | 98.93 % | 462 | 5 | 0 | 43 | 26 | 0 |
| `lib/core/src/line_assembler.cpp` | 100.00 % | 54 | 0 | 9 | 0 | 0 | 0 |
| `lib/core/src/link_policy.cpp` | 98.36 % | 299 | 5 | 1 | 0 | 19 | 0 |
| `lib/core/src/log_sink.cpp` | 97.41 % | 113 | 3 | 0 | 0 | 8 | 0 |
| `lib/core/src/mqtt_policy.cpp` | 98.33 % | 236 | 4 | 0 | 8 | 26 | 0 |
| `lib/core/src/mqtt_topics.cpp` | 99.84 % | 635 | 1 | 1 | 36 | 34 | 0 |
| `lib/core/src/mqtt_values.cpp` | 96.51 % | 249 | 9 | 0 | 0 | 1 | 0 |
| `lib/core/src/net_policy.cpp` | 97.89 % | 92 | 2 | 1 | 0 | 13 | 0 |
| `lib/core/src/net_trial.cpp` | 97.11 % | 302 | 9 | 0 | 0 | 10 | 0 |
| `lib/core/src/ota_policy.cpp` | 95.28 % | 101 | 5 | 0 | 0 | 4 | 0 |
| `lib/core/src/poll_planner.cpp` | 95.43 % | 298 | 15 | 15 | 0 | 0 | 0 |
| `lib/core/src/reset_gate.cpp` | 100.00 % | 26 | 0 | 0 | 0 | 1 | 0 |
| `lib/core/src/restart_gate.cpp` | 100.00 % | 17 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/stm_codec.cpp` | 100.00 % | 983 | 0 | 0 | 0 | 200 | 0 |
| `lib/core/src/stm_flasher.cpp` | 97.90 % | 1017 | 22 | 9 | 98 | 84 | 0 |
| `lib/core/src/stm_session.cpp` | 98.69 % | 449 | 6 | 2 | 9 | 10 | 0 |
| `lib/core/src/sys_health.cpp` | 96.12 % | 124 | 5 | 0 | 0 | 6 | 0 |
| `lib/core/src/target_store.cpp` | 98.94 % | 187 | 2 | 0 | 0 | 6 | 0 |
| `lib/core/src/valve_model.cpp` | 96.08 % | 710 | 29 | 0 | 0 | 21 | 0 |
| `lib/core/src/version.cpp` | 96.65 % | 202 | 7 | 0 | 0 | 9 | 0 |
| `lib/core/src/web_guard.cpp` | 99.62 % | 260 | 1 | 4 | 12 | 12 | 0 |
| **total** | **97.68 %** | 13348 | 319 | 73 | 351 | 1001 | 0 |

## NOMUTATE lines

| file:line | reason |
|---|---|
| `lib/core/src/config.cpp:102` | any size above the longest field is equivalent |
| `lib/core/src/config.cpp:243` | compile-time check |
| `lib/core/src/config.cpp:244` | compile-time check |
| `lib/core/src/config.cpp:245` | compile-time check |
| `lib/core/src/config.cpp:246` | compile-time check |
| `lib/core/src/config.cpp:250` | compile-time check |
| `lib/core/src/config.cpp:251` | compile-time check |
| `lib/core/src/config.cpp:253` | compile-time check |
| `lib/core/src/config.cpp:255` | compile-time check |
| `lib/core/src/config.cpp:256` | compile-time check |
| `lib/core/src/config.cpp:257` | compile-time check |
| `lib/core/src/config.cpp:258` | compile-time check |
| `lib/core/src/config.cpp:259` | compile-time check |
| `lib/core/src/config.cpp:260` | compile-time check |
| `lib/core/src/config.cpp:279` | root count 0 and 1 are equal |
| `lib/core/src/config.cpp:297` | root count 0 and 1 are equal |
| `lib/core/src/config.cpp:571` | i = 0 has no earlier item to compare |
| `lib/core/src/config.cpp:929` | any tiny epsilon is equivalent |
| `lib/core/src/config.cpp:1779` | payload length placeholder, patched below |
| `lib/core/src/config.cpp:1829` | the root tail (persistLog, a bool) never fails its rule |
| `lib/core/src/config.cpp:2095` | payload length placeholder, patched below |
| `lib/core/src/event_limiter.cpp:25` | any value below 1000 is equivalent |
| `lib/core/src/event_limiter.cpp:36` | start time irrelevant (full bucket) |
| `lib/core/src/event_limiter.cpp:37` | start time irrelevant (full bucket) |
| `lib/core/src/event_log.cpp:232` | attribute |
| `lib/core/src/event_log.cpp:632` | wraps after 2^32 events, not reachable in tests |
| `lib/core/src/mqtt_topics.cpp:59` | attribute |
| `lib/core/src/stm_codec.cpp:40` | UINT32_MAX has 10 digits; no builder sends that many |
| `lib/core/src/stm_codec.cpp:50` | exact size; larger is equivalent |
| `lib/core/src/stm_codec.cpp:58` | unreachable guard, longest request is 59 chars |
| `lib/core/src/stm_codec.cpp:66` | unreachable guard (see finish()) |
| `lib/core/src/stm_codec.cpp:67` | unreachable guard (see finish()) |
| `lib/core/src/stm_codec.cpp:90` | unreachable guard (see finish()) |
| `lib/core/src/stm_codec.cpp:811` | unreachable guard (see finish()) |
| `lib/core/src/version.cpp:22` | compile-time check |

## Surviving mutants (319)

<details><summary>list</summary>

- `lib/core/src/auth.cpp:29:28` bool `false` -> `true`
- `lib/core/src/auth.cpp:32:39` bool `false` -> `true`
- `lib/core/src/auth.cpp:35:22` const `0` -> `1`
- `lib/core/src/auth.cpp:54:24` rel `<` -> `<=`
- `lib/core/src/auth.cpp:72:11` rel `<=` -> `<`
- `lib/core/src/auth.cpp:75:23` rel `<=` -> `<`
- `lib/core/src/calib_schedule.cpp:39:36` const `1460` -> `1459`
- `lib/core/src/calib_schedule.cpp:39:49` const `36524` -> `36523`
- `lib/core/src/calib_schedule.cpp:39:63` const `146096` -> `146095`
- `lib/core/src/calib_schedule.cpp:123:20` const `0` -> `1`
- `lib/core/src/calib_schedule.cpp:128:35` const `0` -> `1`
- `lib/core/src/calib_schedule.cpp:143:17` rel `<` -> `<=`
- `lib/core/src/calib_schedule.cpp:244:18` log `||` -> `&&`
- `lib/core/src/calib_schedule.cpp:244:49` log `||` -> `&&`
- `lib/core/src/calib_schedule.cpp:244:80` log `||` -> `&&`
- `lib/core/src/calib_schedule.cpp:249:24` log `||` -> `&&`
- `lib/core/src/common.cpp:15:18` rel `>` -> `>=`
- `lib/core/src/common.cpp:15:20` const `0` -> `1`
- `lib/core/src/common.cpp:15:32` const `1` -> `0`
- `lib/core/src/common.cpp:15:32` const `1` -> `2`
- `lib/core/src/common.cpp:15:47` rel `>` -> `>=`
- `lib/core/src/common.cpp:56:20` log `||` -> `&&`
- `lib/core/src/common.cpp:56:32` const `0` -> `1`
- `lib/core/src/common.cpp:101:28` bool `false` -> `true`
- `lib/core/src/common.cpp:102:48` const `1` -> `2`
- `lib/core/src/common.cpp:105:24` rel `<` -> `<=`
- `lib/core/src/common.cpp:114:22` log `||` -> `&&`
- `lib/core/src/common.cpp:114:32` const `0` -> `1`
- `lib/core/src/common.cpp:114:42` const `0` -> `1`
- `lib/core/src/common.cpp:136:14` const `0` -> `1`
- `lib/core/src/common.cpp:139:13` rel `>` -> `>=`
- `lib/core/src/common.cpp:153:48` const `1` -> `2`
- `lib/core/src/common.cpp:199:13` const `3` -> `4`
- `lib/core/src/common.cpp:201:11` rel `<` -> `<=`
- `lib/core/src/common.cpp:207:85` const `0` -> `1`
- `lib/core/src/common.cpp:247:16` const `0` -> `1`
- `lib/core/src/common.cpp:256:18` const `0` -> `1`
- `lib/core/src/common.cpp:271:18` const `0` -> `1`
- `lib/core/src/common.cpp:275:15` rel `>=` -> `>`
- `lib/core/src/common.cpp:290:9` rel `<` -> `<=`
- `lib/core/src/common.cpp:290:11` const `0` -> `1`
- `lib/core/src/common.cpp:333:11` const `1` -> `2`
- `lib/core/src/config.cpp:455:18` rel `<` -> `<=`
- `lib/core/src/config.cpp:475:10` bool `false` -> `true`
- `lib/core/src/config.cpp:483:35` const `0` -> `1`
- `lib/core/src/config.cpp:508:11` incdec `++` -> `--`
- `lib/core/src/config.cpp:508:16` incdec `++` -> `--`
- `lib/core/src/config.cpp:509:24` rel `==` -> `!=`
- `lib/core/src/config.cpp:510:24` rel `==` -> `!=`
- `lib/core/src/config.cpp:512:28` bool `true` -> `false`
- `lib/core/src/config.cpp:538:13` const `0` -> `1`
- `lib/core/src/config.cpp:632:22` const `0` -> `1`
- `lib/core/src/config.cpp:632:32` incdec `++` -> `--`
- `lib/core/src/config.cpp:637:16` const `0` -> `1`
- `lib/core/src/config.cpp:642:25` rel `<` -> `<=`
- `lib/core/src/config.cpp:833:11` rel `<` -> `<=`
- `lib/core/src/config.cpp:851:29` const `1` -> `2`
- `lib/core/src/config.cpp:876:25` rel `<` -> `<=`
- `lib/core/src/config.cpp:876:41` log `||` -> `&&`
- `lib/core/src/config.cpp:876:46` rel `>` -> `>=`
- `lib/core/src/config.cpp:930:22` rel `>=` -> `>`
- `lib/core/src/config.cpp:1027:11` rel `>` -> `>=`
- `lib/core/src/config.cpp:1027:25` bool `false` -> `true`
- `lib/core/src/config.cpp:1034:17` rel `>` -> `>=`
- `lib/core/src/config.cpp:1045:33` const `3` -> `4`
- `lib/core/src/config.cpp:1064:51` arith `+` -> `-`
- `lib/core/src/config.cpp:1064:53` const `1` -> `0`
- `lib/core/src/config.cpp:1064:53` const `1` -> `2`
- `lib/core/src/config.cpp:1065:14` const `0` -> `1`
- `lib/core/src/config.cpp:1065:16` log `||` -> `&&`
- `lib/core/src/config.cpp:1065:23` rel `>` -> `>=`
- `lib/core/src/config.cpp:1088:20` const `0` -> `1`
- `lib/core/src/config.cpp:1096:59` log `||` -> `&&`
- `lib/core/src/config.cpp:1122:12` const `64` -> `63`
- `lib/core/src/config.cpp:1122:12` const `64` -> `65`
- `lib/core/src/config.cpp:1132:15` const `32` -> `31`
- `lib/core/src/config.cpp:1132:15` const `32` -> `33`
- `lib/core/src/config.cpp:1159:16` const `16` -> `17`
- `lib/core/src/config.cpp:1173:36` const `1` -> `2`
- `lib/core/src/config.cpp:1261:14` rel `>=` -> `>`
- `lib/core/src/config.cpp:1296:68` bool `false` -> `true`
- `lib/core/src/config.cpp:1305:13` const `1` -> `2`
- `lib/core/src/config.cpp:1328:47` log `||` -> `&&`
- `lib/core/src/config.cpp:1331:28` arith `-` -> `+`
- `lib/core/src/config.cpp:1340:15` const `4` -> `5`
- `lib/core/src/config.cpp:1342:12` rel `<` -> `<=`
- `lib/core/src/config.cpp:1361:35` rel `<` -> `<=`
- `lib/core/src/config.cpp:1380:34` bool `false` -> `true`
- `lib/core/src/config.cpp:1401:20` rel `<` -> `<=`
- `lib/core/src/config.cpp:1405:12` bool `false` -> `true`
- `lib/core/src/config.cpp:1422:14` bool `false` -> `true`
- `lib/core/src/config.cpp:1475:38` log `&&` -> `||`
- `lib/core/src/config.cpp:1512:18` const `12` -> `11`
- `lib/core/src/config.cpp:1512:18` const `12` -> `13`
- `lib/core/src/config.cpp:1538:23` arith `+` -> `-`
- `lib/core/src/config.cpp:1538:25` const `1` -> `0`
- `lib/core/src/config.cpp:1538:25` const `1` -> `2`
- `lib/core/src/config.cpp:1541:21` arith `+` -> `-`
- `lib/core/src/config.cpp:1541:23` const `1` -> `0`
- `lib/core/src/config.cpp:1541:23` const `1` -> `2`
- `lib/core/src/config.cpp:1541:29` const `0` -> `1`
- `lib/core/src/config.cpp:1542:21` arith `+` -> `-`
- `lib/core/src/config.cpp:1542:23` const `1` -> `0`
- `lib/core/src/config.cpp:1542:23` const `1` -> `2`
- `lib/core/src/config.cpp:1542:29` const `0` -> `1`
- `lib/core/src/config.cpp:1553:41` bool `false` -> `true`
- `lib/core/src/config.cpp:1556:15` const `24` -> `23`
- `lib/core/src/config.cpp:1556:15` const `24` -> `25`
- `lib/core/src/config.cpp:1586:67` const `0` -> `1`
- `lib/core/src/config.cpp:1597:21` const `2` -> `3`
- `lib/core/src/config.cpp:1601:21` const `4` -> `5`
- `lib/core/src/config.cpp:1621:14` bool `false` -> `true`
- `lib/core/src/config.cpp:1628:17` const `0` -> `1`
- `lib/core/src/config.cpp:1633:15` const `2` -> `3`
- `lib/core/src/config.cpp:1633:21` const `0` -> `1`
- `lib/core/src/config.cpp:1633:24` const `0` -> `1`
- `lib/core/src/config.cpp:1638:15` const `4` -> `5`
- `lib/core/src/config.cpp:1638:21` const `0` -> `1`
- `lib/core/src/config.cpp:1638:24` const `0` -> `1`
- `lib/core/src/config.cpp:1638:27` const `0` -> `1`
- `lib/core/src/config.cpp:1638:30` const `0` -> `1`
- `lib/core/src/config.cpp:1709:13` rel `>=` -> `>`
- `lib/core/src/config.cpp:1803:32` arith `+` -> `-`
- `lib/core/src/config.cpp:1803:34` const `1` -> `0`
- `lib/core/src/config.cpp:1803:34` const `1` -> `2`
- `lib/core/src/config.cpp:1826:20` retval `encodeDefault<CalibScheduleConfig>(f, out, cap)` -> `0`
- `lib/core/src/config.cpp:1874:18` const `0` -> `1`
- `lib/core/src/config.cpp:1881:38` const `0` -> `1`
- `lib/core/src/config.cpp:1899:35` const `0` -> `1`
- `lib/core/src/config.cpp:1905:39` const `0` -> `1`
- `lib/core/src/config.cpp:1909:36` const `0` -> `1`
- `lib/core/src/config.cpp:1913:36` const `0` -> `1`
- `lib/core/src/config.cpp:1922:40` const `0` -> `1`
- `lib/core/src/config.cpp:1924:36` const `0` -> `1`
- `lib/core/src/config.cpp:1934:36` const `0` -> `1`
- `lib/core/src/config.cpp:1938:36` const `0` -> `1`
- `lib/core/src/config.cpp:1942:38` const `0` -> `1`
- `lib/core/src/config.cpp:1946:37` const `0` -> `1`
- `lib/core/src/config.cpp:1982:22` const `0` -> `1`
- `lib/core/src/config.cpp:1985:24` const `0` -> `1`
- `lib/core/src/config.cpp:1985:34` incdec `++` -> `--`
- `lib/core/src/config.cpp:1992:18` const `0` -> `1`
- `lib/core/src/config.cpp:1992:20` log `&&` -> `||`
- `lib/core/src/config.cpp:1992:37` arith `-` -> `+`
- `lib/core/src/config.cpp:1992:39` const `1` -> `0`
- `lib/core/src/config.cpp:1992:47` const `0` -> `1`
- `lib/core/src/config.cpp:2013:12` bool `true` -> `false`
- `lib/core/src/config.cpp:2170:32` const `16` -> `17`
- `lib/core/src/event_limiter.cpp:106:24` rel `>` -> `>=`
- `lib/core/src/event_limiter.cpp:110:59` rel `>` -> `>=`
- `lib/core/src/event_limiter.cpp:132:59` rel `>` -> `>=`
- `lib/core/src/event_log.cpp:238:11` rel `>` -> `>=`
- `lib/core/src/event_log.cpp:450:37` const `1460` -> `1459`
- `lib/core/src/event_log.cpp:450:50` const `36524` -> `36523`
- `lib/core/src/event_log.cpp:450:64` const `146096` -> `146095`
- `lib/core/src/event_log.cpp:489:27` rel `<` -> `<=`
- `lib/core/src/event_log.cpp:513:12` const `160` -> `159`
- `lib/core/src/event_log.cpp:513:12` const `160` -> `161`
- `lib/core/src/event_log.cpp:537:27` rel `<` -> `<=`
- `lib/core/src/event_log.cpp:697:15` rel `<` -> `<=`
- `lib/core/src/file_manager.cpp:19:14` rel `>=` -> `>`
- `lib/core/src/file_manager.cpp:22:31` rel `>=` -> `>`
- `lib/core/src/file_manager.cpp:22:43` rel `<=` -> `<`
- `lib/core/src/file_manager.cpp:33:29` const `2` -> `0`
- `lib/core/src/file_manager.cpp:33:29` const `2` -> `1`
- `lib/core/src/file_manager.cpp:95:24` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:255:50` rel `>=` -> `>`
- `lib/core/src/ha_discovery.cpp:320:14` const `60` -> `59`
- `lib/core/src/ha_discovery.cpp:320:14` const `60` -> `61`
- `lib/core/src/ha_discovery.cpp:723:39` rel `>` -> `>=`
- `lib/core/src/ha_discovery.cpp:789:15` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:888:25` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:1181:24` const `0` -> `1`
- `lib/core/src/ha_discovery.cpp:1181:29` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:1181:46` incdec `++` -> `--`
- `lib/core/src/health_monitor.cpp:28:54` rel `<` -> `<=`
- `lib/core/src/health_monitor.cpp:135:45` rel `<` -> `<=`
- `lib/core/src/health_monitor.cpp:187:29` rel `<` -> `<=`
- `lib/core/src/health_monitor.cpp:202:14` const `1` -> `2`
- `lib/core/src/json_writer.cpp:74:25` bool `false` -> `true`
- `lib/core/src/json_writer.cpp:99:39` bool `false` -> `true`
- `lib/core/src/json_writer.cpp:160:41` bool `false` -> `true`
- `lib/core/src/json_writer.cpp:193:14` rel `>` -> `>=`
- `lib/core/src/json_writer.cpp:217:14` rel `>` -> `>=`
- `lib/core/src/json_writer.cpp:217:44` rel `<` -> `<=`
- `lib/core/src/json_writer.cpp:231:14` rel `>` -> `>=`
- `lib/core/src/json_writer.cpp:231:44` rel `<` -> `<=`
- `lib/core/src/lease_client.cpp:258:17` rel `<` -> `<=`
- `lib/core/src/lease_client.cpp:304:62` log `||` -> `&&`
- `lib/core/src/legacy_import.cpp:148:40` bool `true` -> `false`
- `lib/core/src/legacy_import.cpp:211:11` const `0` -> `1`
- `lib/core/src/legacy_import.cpp:371:40` bool `false` -> `true`
- `lib/core/src/legacy_import.cpp:603:52` bool `true` -> `false`
- `lib/core/src/legacy_import.cpp:672:24` const `0` -> `1`
- `lib/core/src/link_policy.cpp:70:30` rel `<` -> `<=`
- `lib/core/src/link_policy.cpp:163:25` rel `<` -> `<=`
- `lib/core/src/link_policy.cpp:285:9` rel `>` -> `>=`
- `lib/core/src/link_policy.cpp:300:71` const `1000` -> `999`
- `lib/core/src/link_policy.cpp:312:15` rel `>=` -> `>`
- `lib/core/src/log_sink.cpp:55:19` const `0` -> `1`
- `lib/core/src/log_sink.cpp:66:14` rel `<` -> `<=`
- `lib/core/src/log_sink.cpp:81:11` const `26` -> `27`
- `lib/core/src/mqtt_policy.cpp:122:11` const `16` -> `15`
- `lib/core/src/mqtt_policy.cpp:123:12` rel `>` -> `>=`
- `lib/core/src/mqtt_policy.cpp:123:14` const `0` -> `1`
- `lib/core/src/mqtt_policy.cpp:326:13` rel `>=` -> `>`
- `lib/core/src/mqtt_topics.cpp:344:30` const `1` -> `2`
- `lib/core/src/mqtt_values.cpp:28:25` rel `<` -> `<=`
- `lib/core/src/mqtt_values.cpp:36:18` log `||` -> `&&`
- `lib/core/src/mqtt_values.cpp:67:9` rel `>=` -> `>`
- `lib/core/src/mqtt_values.cpp:108:45` rel `<` -> `<=`
- `lib/core/src/mqtt_values.cpp:139:32` const `0` -> `1`
- `lib/core/src/mqtt_values.cpp:160:25` rel `<` -> `<=`
- `lib/core/src/mqtt_values.cpp:171:16` rel `<` -> `<=`
- `lib/core/src/mqtt_values.cpp:179:16` rel `<` -> `<=`
- `lib/core/src/mqtt_values.cpp:183:13` rel `<` -> `<=`
- `lib/core/src/net_policy.cpp:29:18` bool `false` -> `true`
- `lib/core/src/net_policy.cpp:81:15` bool `false` -> `true`
- `lib/core/src/net_trial.cpp:11:26` const `4` -> `5`
- `lib/core/src/net_trial.cpp:34:12` rel `<` -> `<=`
- `lib/core/src/net_trial.cpp:91:26` const `25` -> `24`
- `lib/core/src/net_trial.cpp:91:26` const `25` -> `26`
- `lib/core/src/net_trial.cpp:91:36` rel `>=` -> `>`
- `lib/core/src/net_trial.cpp:126:11` const `16` -> `15`
- `lib/core/src/net_trial.cpp:126:11` const `16` -> `17`
- `lib/core/src/net_trial.cpp:165:12` rel `>=` -> `>`
- `lib/core/src/net_trial.cpp:165:27` const `0` -> `1`
- `lib/core/src/ota_policy.cpp:17:14` bool `false` -> `true`
- `lib/core/src/ota_policy.cpp:70:13` rel `>=` -> `>`
- `lib/core/src/ota_policy.cpp:70:28` const `0` -> `1`
- `lib/core/src/ota_policy.cpp:75:12` const `33` -> `34`
- `lib/core/src/ota_policy.cpp:90:26` const `33` -> `32`
- `lib/core/src/poll_planner.cpp:37:13` rel `>=` -> `>`
- `lib/core/src/poll_planner.cpp:53:20` bool `false` -> `true`
- `lib/core/src/poll_planner.cpp:62:46` rel `>=` -> `>`
- `lib/core/src/poll_planner.cpp:300:30` const `0` -> `1`
- `lib/core/src/poll_planner.cpp:300:48` const `0` -> `1`
- `lib/core/src/poll_planner.cpp:301:11` rel `>` -> `>=`
- `lib/core/src/poll_planner.cpp:301:13` const `0` -> `1`
- `lib/core/src/poll_planner.cpp:301:49` const `1` -> `0`
- `lib/core/src/poll_planner.cpp:301:49` const `1` -> `2`
- `lib/core/src/poll_planner.cpp:306:21` bool `false` -> `true`
- `lib/core/src/poll_planner.cpp:307:22` bool `false` -> `true`
- `lib/core/src/poll_planner.cpp:309:17` const `0` -> `1`
- `lib/core/src/poll_planner.cpp:312:17` rel `==` -> `!=`
- `lib/core/src/poll_planner.cpp:350:35` rel `>=` -> `>`
- `lib/core/src/poll_planner.cpp:350:38` const `2` -> `3`
- `lib/core/src/stm_flasher.cpp:31:36` const `256` -> `255`
- `lib/core/src/stm_flasher.cpp:31:36` const `256` -> `257`
- `lib/core/src/stm_flasher.cpp:126:37` arith `-` -> `+`
- `lib/core/src/stm_flasher.cpp:181:60` arith `-` -> `+`
- `lib/core/src/stm_flasher.cpp:333:34` arith `-` -> `+`
- `lib/core/src/stm_flasher.cpp:333:36` const `1` -> `0`
- `lib/core/src/stm_flasher.cpp:472:33` arith `-` -> `+`
- `lib/core/src/stm_flasher.cpp:485:37` arith `-` -> `+`
- `lib/core/src/stm_flasher.cpp:486:12` asgn `-=` -> `+=`
- `lib/core/src/stm_flasher.cpp:495:47` const `1` -> `2`
- `lib/core/src/stm_flasher.cpp:530:67` rel `<` -> `<=`
- `lib/core/src/stm_flasher.cpp:626:6` negcond `(` -> `(!`
- `lib/core/src/stm_flasher.cpp:626:14` rel `>` -> `<=`
- `lib/core/src/stm_flasher.cpp:690:7` incdec `++` -> `--`
- `lib/core/src/stm_flasher.cpp:691:14` const `2` -> `0`
- `lib/core/src/stm_flasher.cpp:691:14` const `2` -> `1`
- `lib/core/src/stm_flasher.cpp:737:21` const `0` -> `1`
- `lib/core/src/stm_flasher.cpp:878:19` const `0` -> `1`
- `lib/core/src/stm_flasher.cpp:878:21` log `&&` -> `||`
- `lib/core/src/stm_flasher.cpp:883:12` asgn `-=` -> `+=`
- `lib/core/src/stm_flasher.cpp:890:51` rel `<` -> `<=`
- `lib/core/src/stm_flasher.cpp:926:12` rel `<` -> `<=`
- `lib/core/src/stm_session.cpp:157:52` arith `+` -> `-`
- `lib/core/src/stm_session.cpp:632:10` negcond `(` -> `(!`
- `lib/core/src/stm_session.cpp:751:34` log `||` -> `&&`
- `lib/core/src/stm_session.cpp:751:66` log `||` -> `&&`
- `lib/core/src/stm_session.cpp:753:50` log `||` -> `&&`
- `lib/core/src/stm_session.cpp:781:14` bool `true` -> `false`
- `lib/core/src/sys_health.cpp:11:17` rel `>` -> `>=`
- `lib/core/src/sys_health.cpp:11:19` const `512` -> `0`
- `lib/core/src/sys_health.cpp:11:19` const `512` -> `511`
- `lib/core/src/sys_health.cpp:16:33` rel `<` -> `<=`
- `lib/core/src/sys_health.cpp:52:31` const `7` -> `6`
- `lib/core/src/target_store.cpp:9:26` const `4` -> `5`
- `lib/core/src/target_store.cpp:67:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:81:37` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:150:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:164:13` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:189:68` rel `==` -> `!=`
- `lib/core/src/valve_model.cpp:210:15` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:230:15` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:293:15` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:357:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:359:17` const `2` -> `3`
- `lib/core/src/valve_model.cpp:382:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:404:24` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:430:13` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:435:22` rel `>` -> `>=`
- `lib/core/src/valve_model.cpp:441:13` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:450:13` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:460:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:481:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:507:13` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:522:13` rel `>=` -> `>`
- `lib/core/src/valve_model.cpp:549:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:572:21` bool `false` -> `true`
- `lib/core/src/valve_model.cpp:580:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:591:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:607:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:674:29` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:742:47` const `1` -> `2`
- `lib/core/src/valve_model.cpp:749:47` const `1` -> `2`
- `lib/core/src/valve_model.cpp:788:25` rel `<` -> `<=`
- `lib/core/src/valve_model.cpp:795:16` rel `>=` -> `>`
- `lib/core/src/version.cpp:56:51` rel `>=` -> `>`
- `lib/core/src/version.cpp:57:51` rel `>=` -> `>`
- `lib/core/src/version.cpp:78:23` const `1` -> `0`
- `lib/core/src/version.cpp:91:42` rel `<` -> `<=`
- `lib/core/src/version.cpp:92:42` rel `<` -> `<=`
- `lib/core/src/version.cpp:93:42` rel `<` -> `<=`
- `lib/core/src/version.cpp:105:31` const `0` -> `1`
- `lib/core/src/web_guard.cpp:154:34` const `0` -> `1`

</details>
