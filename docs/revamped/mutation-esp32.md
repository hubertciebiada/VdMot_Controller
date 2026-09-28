# Mutation testing: esp32

Scope: `software_esp32_revamped/lib/core`, ESP32 core (pure logic).
Target: >= 95 % overall and >= 95 % for every file.
Result: **97.75 % overall (13053 / 13354 killed), lowest file `lib/core/src/file_manager.cpp` 95.00 %, gate passed.**
Measured on commit `3ad136a` (2026-09-28), 6 workers, 44 min.

Tool: `tools/mutation/mutate.py`, config `tools/mutation/esp32.json`. Every mutant is built
with the sanitizer build of the native tests (ASan + UBSan, `-Werror`) and runs the tests of its
module first, then the whole suite. A failed test, a crash or a timeout kills the mutant.
Not counted: stillborn mutants (they do not compile), mutants in code the native build does not
compile, and equivalent mutants. The 345 equivalent mutants are listed with a reason in
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
| `lib/core/src/calib_schedule.cpp` | 98.22 % | 553 | 10 | 0 | 0 | 14 | 0 |
| `lib/core/src/common.cpp` | 96.57 % | 725 | 26 | 6 | 0 | 15 | 0 |
| `lib/core/src/config.cpp` | 95.32 % | 1978 | 98 | 17 | 0 | 198 | 0 |
| `lib/core/src/event_limiter.cpp` | 96.84 % | 92 | 3 | 0 | 0 | 0 | 0 |
| `lib/core/src/event_log.cpp` | 98.34 % | 530 | 9 | 3 | 0 | 4 | 0 |
| `lib/core/src/factory_reset.cpp` | 100.00 % | 10 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/failsafe.cpp` | 100.00 % | 24 | 0 | 0 | 0 | 24 | 0 |
| `lib/core/src/file_manager.cpp` | 95.00 % | 113 | 6 | 1 | 0 | 20 | 0 |
| `lib/core/src/ha_discovery.cpp` | 98.74 % | 700 | 9 | 3 | 93 | 195 | 0 |
| `lib/core/src/health_monitor.cpp` | 98.11 % | 208 | 4 | 0 | 0 | 0 | 0 |
| `lib/core/src/image_store.cpp` | 100.00 % | 76 | 0 | 0 | 2 | 0 | 0 |
| `lib/core/src/json_api.cpp` | 100.00 % | 329 | 0 | 0 | 22 | 12 | 0 |
| `lib/core/src/json_writer.cpp` | 97.09 % | 267 | 8 | 0 | 8 | 2 | 0 |
| `lib/core/src/lease_client.cpp` | 99.58 % | 479 | 2 | 0 | 12 | 8 | 0 |
| `lib/core/src/legacy_http.cpp` | 100.00 % | 76 | 0 | 0 | 2 | 6 | 0 |
| `lib/core/src/legacy_import.cpp` | 99.15 % | 468 | 4 | 0 | 43 | 26 | 0 |
| `lib/core/src/line_assembler.cpp` | 100.00 % | 54 | 0 | 9 | 0 | 0 | 0 |
| `lib/core/src/link_policy.cpp` | 98.36 % | 299 | 5 | 1 | 0 | 19 | 0 |
| `lib/core/src/log_sink.cpp` | 97.41 % | 113 | 3 | 0 | 0 | 8 | 0 |
| `lib/core/src/mqtt_policy.cpp` | 98.33 % | 236 | 4 | 0 | 8 | 26 | 0 |
| `lib/core/src/mqtt_topics.cpp` | 99.85 % | 650 | 1 | 1 | 36 | 40 | 0 |
| `lib/core/src/mqtt_values.cpp` | 96.51 % | 249 | 9 | 0 | 0 | 1 | 0 |
| `lib/core/src/net_policy.cpp` | 97.89 % | 92 | 2 | 1 | 0 | 13 | 0 |
| `lib/core/src/net_trial.cpp` | 97.79 % | 266 | 6 | 0 | 0 | 10 | 0 |
| `lib/core/src/ota_policy.cpp` | 95.28 % | 101 | 5 | 0 | 0 | 4 | 0 |
| `lib/core/src/poll_planner.cpp` | 95.43 % | 298 | 15 | 15 | 0 | 0 | 0 |
| `lib/core/src/reset_gate.cpp` | 100.00 % | 26 | 0 | 0 | 0 | 1 | 0 |
| `lib/core/src/restart_gate.cpp` | 100.00 % | 17 | 0 | 0 | 0 | 5 | 0 |
| `lib/core/src/stm_codec.cpp` | 100.00 % | 983 | 0 | 0 | 0 | 200 | 0 |
| `lib/core/src/stm_flasher.cpp` | 97.90 % | 1017 | 22 | 9 | 98 | 84 | 0 |
| `lib/core/src/stm_session.cpp` | 98.69 % | 449 | 6 | 2 | 9 | 10 | 0 |
| `lib/core/src/sys_health.cpp` | 96.64 % | 144 | 5 | 0 | 0 | 6 | 0 |
| `lib/core/src/target_store.cpp` | 98.94 % | 187 | 2 | 0 | 0 | 6 | 0 |
| `lib/core/src/valve_model.cpp` | 96.08 % | 710 | 29 | 0 | 0 | 21 | 0 |
| `lib/core/src/version.cpp` | 96.65 % | 202 | 7 | 0 | 0 | 9 | 0 |
| `lib/core/src/web_guard.cpp` | 99.62 % | 260 | 1 | 4 | 12 | 12 | 0 |
| **total** | **97.75 %** | 12981 | 301 | 72 | 345 | 1004 | 0 |

## NOMUTATE lines

| file:line | reason |
|---|---|
| `lib/core/src/config.cpp:120` | any size above the longest field is equivalent |
| `lib/core/src/config.cpp:259` | compile-time check |
| `lib/core/src/config.cpp:260` | compile-time check |
| `lib/core/src/config.cpp:261` | compile-time check |
| `lib/core/src/config.cpp:262` | compile-time check |
| `lib/core/src/config.cpp:266` | compile-time check |
| `lib/core/src/config.cpp:267` | compile-time check |
| `lib/core/src/config.cpp:269` | compile-time check |
| `lib/core/src/config.cpp:271` | compile-time check |
| `lib/core/src/config.cpp:272` | compile-time check |
| `lib/core/src/config.cpp:273` | compile-time check |
| `lib/core/src/config.cpp:274` | compile-time check |
| `lib/core/src/config.cpp:275` | compile-time check |
| `lib/core/src/config.cpp:276` | compile-time check |
| `lib/core/src/config.cpp:295` | root count 0 and 1 are equal |
| `lib/core/src/config.cpp:313` | root count 0 and 1 are equal |
| `lib/core/src/config.cpp:583` | i = 0 has no earlier item to compare |
| `lib/core/src/config.cpp:931` | any tiny epsilon is equivalent |
| `lib/core/src/config.cpp:1782` | payload length placeholder, patched below |
| `lib/core/src/config.cpp:1837` | the root tail (persistLog, a bool) never fails its rule |
| `lib/core/src/config.cpp:2082` | payload length placeholder, patched below |
| `lib/core/src/event_limiter.cpp:25` | any value below 1000 is equivalent |
| `lib/core/src/event_limiter.cpp:36` | start time irrelevant (full bucket) |
| `lib/core/src/event_limiter.cpp:37` | start time irrelevant (full bucket) |
| `lib/core/src/event_log.cpp:237` | attribute |
| `lib/core/src/event_log.cpp:637` | wraps after 2^32 events, not reachable in tests |
| `lib/core/src/mqtt_topics.cpp:59` | attribute |
| `lib/core/src/stm_codec.cpp:40` | UINT32_MAX has 10 digits; no builder sends that many |
| `lib/core/src/stm_codec.cpp:50` | exact size; larger is equivalent |
| `lib/core/src/stm_codec.cpp:58` | unreachable guard, longest request is 59 chars |
| `lib/core/src/stm_codec.cpp:66` | unreachable guard (see finish()) |
| `lib/core/src/stm_codec.cpp:67` | unreachable guard (see finish()) |
| `lib/core/src/stm_codec.cpp:90` | unreachable guard (see finish()) |
| `lib/core/src/stm_codec.cpp:811` | unreachable guard (see finish()) |
| `lib/core/src/version.cpp:22` | compile-time check |

## Surviving mutants (301)

<details><summary>list</summary>

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
- `lib/core/src/config.cpp:468:18` rel `<` -> `<=`
- `lib/core/src/config.cpp:488:10` bool `false` -> `true`
- `lib/core/src/config.cpp:496:35` const `0` -> `1`
- `lib/core/src/config.cpp:521:11` incdec `++` -> `--`
- `lib/core/src/config.cpp:521:16` incdec `++` -> `--`
- `lib/core/src/config.cpp:522:24` rel `==` -> `!=`
- `lib/core/src/config.cpp:523:24` rel `==` -> `!=`
- `lib/core/src/config.cpp:525:28` bool `true` -> `false`
- `lib/core/src/config.cpp:550:13` const `0` -> `1`
- `lib/core/src/config.cpp:634:22` const `0` -> `1`
- `lib/core/src/config.cpp:634:32` incdec `++` -> `--`
- `lib/core/src/config.cpp:639:16` const `0` -> `1`
- `lib/core/src/config.cpp:644:25` rel `<` -> `<=`
- `lib/core/src/config.cpp:835:11` rel `<` -> `<=`
- `lib/core/src/config.cpp:853:29` const `1` -> `2`
- `lib/core/src/config.cpp:878:25` rel `<` -> `<=`
- `lib/core/src/config.cpp:878:41` log `||` -> `&&`
- `lib/core/src/config.cpp:878:46` rel `>` -> `>=`
- `lib/core/src/config.cpp:932:22` rel `>=` -> `>`
- `lib/core/src/config.cpp:1029:11` rel `>` -> `>=`
- `lib/core/src/config.cpp:1029:25` bool `false` -> `true`
- `lib/core/src/config.cpp:1036:17` rel `>` -> `>=`
- `lib/core/src/config.cpp:1047:33` const `3` -> `4`
- `lib/core/src/config.cpp:1067:51` arith `+` -> `-`
- `lib/core/src/config.cpp:1067:53` const `1` -> `0`
- `lib/core/src/config.cpp:1067:53` const `1` -> `2`
- `lib/core/src/config.cpp:1068:14` const `0` -> `1`
- `lib/core/src/config.cpp:1068:16` log `||` -> `&&`
- `lib/core/src/config.cpp:1068:23` rel `>` -> `>=`
- `lib/core/src/config.cpp:1091:20` const `0` -> `1`
- `lib/core/src/config.cpp:1099:59` log `||` -> `&&`
- `lib/core/src/config.cpp:1125:12` const `64` -> `63`
- `lib/core/src/config.cpp:1125:12` const `64` -> `65`
- `lib/core/src/config.cpp:1135:15` const `32` -> `31`
- `lib/core/src/config.cpp:1135:15` const `32` -> `33`
- `lib/core/src/config.cpp:1162:16` const `16` -> `17`
- `lib/core/src/config.cpp:1176:36` const `1` -> `2`
- `lib/core/src/config.cpp:1264:14` rel `>=` -> `>`
- `lib/core/src/config.cpp:1299:68` bool `false` -> `true`
- `lib/core/src/config.cpp:1308:13` const `1` -> `2`
- `lib/core/src/config.cpp:1331:47` log `||` -> `&&`
- `lib/core/src/config.cpp:1334:28` arith `-` -> `+`
- `lib/core/src/config.cpp:1343:15` const `4` -> `5`
- `lib/core/src/config.cpp:1345:12` rel `<` -> `<=`
- `lib/core/src/config.cpp:1364:35` rel `<` -> `<=`
- `lib/core/src/config.cpp:1383:34` bool `false` -> `true`
- `lib/core/src/config.cpp:1404:20` rel `<` -> `<=`
- `lib/core/src/config.cpp:1408:12` bool `false` -> `true`
- `lib/core/src/config.cpp:1425:14` bool `false` -> `true`
- `lib/core/src/config.cpp:1478:38` log `&&` -> `||`
- `lib/core/src/config.cpp:1515:18` const `12` -> `11`
- `lib/core/src/config.cpp:1515:18` const `12` -> `13`
- `lib/core/src/config.cpp:1541:23` arith `+` -> `-`
- `lib/core/src/config.cpp:1541:25` const `1` -> `0`
- `lib/core/src/config.cpp:1541:25` const `1` -> `2`
- `lib/core/src/config.cpp:1544:21` arith `+` -> `-`
- `lib/core/src/config.cpp:1544:23` const `1` -> `0`
- `lib/core/src/config.cpp:1544:23` const `1` -> `2`
- `lib/core/src/config.cpp:1544:29` const `0` -> `1`
- `lib/core/src/config.cpp:1545:21` arith `+` -> `-`
- `lib/core/src/config.cpp:1545:23` const `1` -> `0`
- `lib/core/src/config.cpp:1545:23` const `1` -> `2`
- `lib/core/src/config.cpp:1545:29` const `0` -> `1`
- `lib/core/src/config.cpp:1556:41` bool `false` -> `true`
- `lib/core/src/config.cpp:1559:15` const `24` -> `23`
- `lib/core/src/config.cpp:1559:15` const `24` -> `25`
- `lib/core/src/config.cpp:1589:67` const `0` -> `1`
- `lib/core/src/config.cpp:1600:21` const `2` -> `3`
- `lib/core/src/config.cpp:1604:21` const `4` -> `5`
- `lib/core/src/config.cpp:1624:14` bool `false` -> `true`
- `lib/core/src/config.cpp:1631:17` const `0` -> `1`
- `lib/core/src/config.cpp:1636:15` const `2` -> `3`
- `lib/core/src/config.cpp:1636:21` const `0` -> `1`
- `lib/core/src/config.cpp:1636:24` const `0` -> `1`
- `lib/core/src/config.cpp:1641:15` const `4` -> `5`
- `lib/core/src/config.cpp:1641:21` const `0` -> `1`
- `lib/core/src/config.cpp:1641:24` const `0` -> `1`
- `lib/core/src/config.cpp:1641:27` const `0` -> `1`
- `lib/core/src/config.cpp:1641:30` const `0` -> `1`
- `lib/core/src/config.cpp:1811:32` arith `+` -> `-`
- `lib/core/src/config.cpp:1811:34` const `1` -> `0`
- `lib/core/src/config.cpp:1811:34` const `1` -> `2`
- `lib/core/src/config.cpp:1834:20` retval `encodeDefault<CalibScheduleConfig>(f, out, cap)` -> `0`
- `lib/core/src/config.cpp:1881:18` const `0` -> `1`
- `lib/core/src/config.cpp:1888:38` const `0` -> `1`
- `lib/core/src/config.cpp:1906:35` const `0` -> `1`
- `lib/core/src/config.cpp:1910:36` const `0` -> `1`
- `lib/core/src/config.cpp:1918:36` const `0` -> `1`
- `lib/core/src/config.cpp:1922:36` const `0` -> `1`
- `lib/core/src/config.cpp:1926:38` const `0` -> `1`
- `lib/core/src/config.cpp:1930:37` const `0` -> `1`
- `lib/core/src/config.cpp:1966:22` const `0` -> `1`
- `lib/core/src/config.cpp:1969:24` const `0` -> `1`
- `lib/core/src/config.cpp:1969:34` incdec `++` -> `--`
- `lib/core/src/config.cpp:1976:18` const `0` -> `1`
- `lib/core/src/config.cpp:1976:39` const `1` -> `0`
- `lib/core/src/config.cpp:1997:12` bool `true` -> `false`
- `lib/core/src/config.cpp:2157:32` const `16` -> `17`
- `lib/core/src/event_limiter.cpp:106:24` rel `>` -> `>=`
- `lib/core/src/event_limiter.cpp:110:59` rel `>` -> `>=`
- `lib/core/src/event_limiter.cpp:132:59` rel `>` -> `>=`
- `lib/core/src/event_log.cpp:243:11` rel `>` -> `>=`
- `lib/core/src/event_log.cpp:455:37` const `1460` -> `1459`
- `lib/core/src/event_log.cpp:455:50` const `36524` -> `36523`
- `lib/core/src/event_log.cpp:455:64` const `146096` -> `146095`
- `lib/core/src/event_log.cpp:494:27` rel `<` -> `<=`
- `lib/core/src/event_log.cpp:518:12` const `160` -> `159`
- `lib/core/src/event_log.cpp:518:12` const `160` -> `161`
- `lib/core/src/event_log.cpp:542:27` rel `<` -> `<=`
- `lib/core/src/event_log.cpp:702:15` rel `<` -> `<=`
- `lib/core/src/file_manager.cpp:19:14` rel `>=` -> `>`
- `lib/core/src/file_manager.cpp:22:31` rel `>=` -> `>`
- `lib/core/src/file_manager.cpp:22:43` rel `<=` -> `<`
- `lib/core/src/file_manager.cpp:33:29` const `2` -> `0`
- `lib/core/src/file_manager.cpp:33:29` const `2` -> `1`
- `lib/core/src/file_manager.cpp:95:24` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:255:50` rel `>=` -> `>`
- `lib/core/src/ha_discovery.cpp:320:14` const `60` -> `59`
- `lib/core/src/ha_discovery.cpp:320:14` const `60` -> `61`
- `lib/core/src/ha_discovery.cpp:722:39` rel `>` -> `>=`
- `lib/core/src/ha_discovery.cpp:788:15` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:908:25` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:1205:24` const `0` -> `1`
- `lib/core/src/ha_discovery.cpp:1205:29` rel `<` -> `<=`
- `lib/core/src/ha_discovery.cpp:1205:46` incdec `++` -> `--`
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
- `lib/core/src/legacy_import.cpp:154:40` bool `true` -> `false`
- `lib/core/src/legacy_import.cpp:372:40` bool `false` -> `true`
- `lib/core/src/legacy_import.cpp:603:52` bool `true` -> `false`
- `lib/core/src/legacy_import.cpp:675:24` const `0` -> `1`
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
- `lib/core/src/mqtt_topics.cpp:319:30` const `1` -> `2`
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
- `lib/core/src/net_trial.cpp:75:7` const `25` -> `26`
- `lib/core/src/net_trial.cpp:105:11` const `16` -> `15`
- `lib/core/src/net_trial.cpp:105:11` const `16` -> `17`
- `lib/core/src/net_trial.cpp:144:12` rel `>=` -> `>`
- `lib/core/src/net_trial.cpp:144:27` const `0` -> `1`
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
- `lib/core/src/sys_health.cpp:68:31` const `7` -> `6`
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
