#!/usr/bin/env python3
"""Tests of tools/mock_api.py: the mock answers like the firmware (rows M1-M26).

    python3 tools/test_mock_api.py

Standard library only: the mock runs on a free port in a thread; every test
gets a fresh simulated device.
"""
from __future__ import annotations

import http.client
import json
import os
import sys
import threading
import time
import unittest
from http.server import ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mock_api  # noqa: E402

SERVER = None
PORT = 0


def setUpModule():
    global SERVER, PORT
    mock_api.Handler.dev = mock_api.Device("1", 2)
    SERVER = ThreadingHTTPServer(("127.0.0.1", 0), mock_api.Handler)
    SERVER.daemon_threads = True
    PORT = SERVER.server_address[1]
    threading.Thread(target=SERVER.serve_forever, daemon=True).start()


def tearDownModule():
    SERVER.shutdown()
    SERVER.server_close()


class Resp:
    def __init__(self, r):
        self.status = r.status
        self.headers = {k.lower(): v for k, v in r.getheaders()}
        raw = r.read()
        self.text = raw.decode("utf-8", "replace")
        try:
            self.json = json.loads(raw) if raw else None
        except ValueError:
            self.json = None


def req(method, path, body=None, headers=None, host=None, raw=None, ctype="application/json", marker=True):
    """One request; API writes get X-VdMot and JSON like the dashboard sends them."""
    c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=10)
    h = {"Host": host if host is not None else f"127.0.0.1:{PORT}"}
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    if data is not None:
        h["Content-Type"] = ctype
    if marker and method in ("POST", "DELETE") and path.startswith("/api/"):
        h["X-VdMot"] = "1"
    h.update(headers or {})
    c.request(method, path, body=data, headers=h)
    r = Resp(c.getresponse())
    c.close()
    return r


def device(proto=2, scenarios=(), import_report=False, station_name=None):
    d = mock_api.Device("1", proto, scenarios, import_report, station_name)
    mock_api.Handler.dev = d
    return d


def restart_now(d):
    """Completes a requested restart without waiting for the simulated boot."""
    d.restart_at = time.time() - 1
    d.tick()
    d.down_until = time.time() - 1
    d.tick()


class Guard(unittest.TestCase):  # M1
    def test_host_rules(self):
        device()
        self.assertEqual(req("GET", "/api/status").status, 200)
        self.assertEqual(req("GET", "/api/status", host="localhost:8080").status, 200)
        self.assertEqual(req("GET", "/api/status", host="vdmot-1.local").status, 200)
        r = req("GET", "/api/status", host="evil.com")
        self.assertEqual(r.status, 403)
        self.assertEqual(r.json, {"error": "host_not_allowed",
                                  "detail": "evil.com: use 127.0.0.1 or add the name to web.allowedHosts"})
        self.assertEqual(req("GET", "/", host="evil.com").status, 200)  # assets are not checked

    def test_origin_marker_type(self):
        device()
        r = req("POST", "/api/valves/1/target", {"target": 5}, headers={"Origin": "http://evil.com"})
        self.assertEqual((r.status, r.json["error"]), (403, "origin_not_allowed"))
        r = req("POST", "/api/valves/1/target", {"target": 5}, headers={"Origin": f"http://127.0.0.1:{PORT}"})
        self.assertEqual(r.status, 202)
        r = req("POST", "/api/valves/1/target", {"target": 5}, marker=False)
        self.assertEqual(r.json, {"error": "header_required", "detail": "X-VdMot: 1"})
        r = req("POST", "/api/valves/1/target", {"target": 5}, ctype="text/plain")
        self.assertEqual((r.status, r.json["error"]), (415, "unsupported_media_type"))
        r = req("POST", "/api/valves/1/target", {"target": 5}, ctype="application/json; charset=utf-8")
        self.assertEqual(r.status, 202)
        self.assertEqual(req("GET", "/api/valves", marker=False).status, 200)


class NoLogin(unittest.TestCase):  # M2
    def test_no_credentials_needed(self):
        device()
        headers = {"Authorization": "Basic YWRtaW46eA=="}  # admin:x, ignored
        self.assertEqual(req("POST", "/api/mqtt/reconnect", {}, headers=headers).status, 202)
        self.assertEqual(req("POST", "/setvalve", {"valve": 1, "value": 20}).status, 200)
        self.assertNotIn("auth", req("GET", "/api/status").json)

    def test_retired_login_keys(self):
        d = device()
        before = req("GET", "/api/config").json
        self.assertNotIn("user", before["web"])
        r = req("POST", "/api/config", {"web": {"user": "admin", "password": "pw", "passwordSet": True,
                                                "protectRead": True, "allowedHosts": "heating.lan"}})
        self.assertEqual(r.status, 200)
        self.assertEqual(d.config["web"], {"allowedHosts": "heating.lan"})


class Legacy(unittest.TestCase):  # M3
    def test_aliases(self):
        d = device()
        r = req("GET", "/valves")
        self.assertEqual(r.status, 200)
        self.assertEqual(r.json["valves"][0]["idx"], 1)
        self.assertIn("controlActive", r.json["valves"][0])
        self.assertIsInstance(req("GET", "/temps").json, list)
        volts = req("GET", "/volts").json
        self.assertEqual(volts[0]["unit"], "V")
        r = req("POST", "/setvalve", {"valve": 1, "value": 43.7, "ctrlValue": 1}, marker=False)
        self.assertEqual((r.status, r.json), (200, {"res": "ok"}))
        self.assertEqual(d.valves[0]["target"], 44)
        r = req("POST", "/setvalve", {"valve": 13, "value": 1})
        self.assertEqual(r.json, {"error": "out_of_range", "detail": "valve 1..12, value 0..100"})
        self.assertEqual(req("GET", "/setvalve").status, 405)

    def test_gone_and_others(self):
        device()
        r = req("GET", "/netinfo")
        self.assertEqual((r.status, r.json), (410, {"error": "gone", "detail": "/api/status"}))
        r = req("POST", "/netconfig", raw=b"x" * 2048, ctype="text/plain")
        self.assertEqual((r.status, r.json["detail"]), (410, "/api/config"))
        self.assertEqual(req("POST", "/nope", raw=b"{}").status, 405)
        self.assertEqual(req("GET", "/nope").status, 404)


class Config(unittest.TestCase):  # M4
    def test_patch_errors_and_schema(self):
        device()
        cfg = req("GET", "/api/config").json
        self.assertEqual(cfg["schema"], 2)
        for key in ("allowedHosts",):
            self.assertIn(key, cfg["web"])
        for key in ("rootTopic", "clientId", "discoveryPrefix"):
            self.assertIn(key, cfg["mqtt"])
        self.assertEqual(cfg["failsafe"], {"timeoutMin": 60})
        r = req("POST", "/api/config", {"calib": {"hour": 24}})
        self.assertEqual(r.json, {"error": "invalid", "detail": "calib.hour"})
        r = req("POST", "/api/config", {"nope": 1})
        self.assertEqual(r.json, {"error": "invalid", "detail": "nope"})
        r = req("POST", "/api/config", {"failsafe": {"timeoutMin": 3}})
        self.assertEqual(r.json, {"error": "invalid", "detail": "failsafe.timeoutMin"})

    def test_restart_members_and_dry_run(self):  # E18-3
        d = device()
        r = req("POST", "/api/config?dryRun=1", {"net": {"dhcp": False, "ip": "192.168.1.60", "mask": "255.255.255.0",
                                                         "gateway": "192.168.1.1"}})
        self.assertEqual(r.json, {"restartRequired": True, "netTrial": True})
        self.assertTrue(d.config["net"]["dhcp"])
        r = req("POST", "/api/config", {"calib": {"hour": 4}})
        self.assertEqual((r.json["restartRequired"], r.json["netTrial"]), (False, False))
        self.assertEqual(list(r.json)[-2:], ["restartRequired", "netTrial"])
        r = req("POST", "/api/config", {"net": {"reconnectTimeoutMin": 9}})
        self.assertEqual((r.json["restartRequired"], r.json["netTrial"]), (False, False))
        r = req("POST", "/api/config", {"net": {"dhcp": False, "ip": "192.168.1.60", "mask": "255.255.255.0",
                                                "gateway": "192.168.1.1"}})
        self.assertEqual((r.json["restartRequired"], r.json["netTrial"]), (True, True))

    def test_export_without_secrets(self):
        device()
        for path in ("/api/config/export", "/api/config/export?secrets=1"):
            r = req("GET", path)
            self.assertEqual(r.status, 200)
            self.assertNotIn("password", r.json["mqtt"])
            self.assertTrue(r.json["mqtt"]["passwordSet"])
            self.assertIn("vdmot-config.json", r.headers["content-disposition"])


class Status(unittest.TestCase):  # M5, E16
    def test_members(self):
        device(station_name="Dom Północ", import_report=True)
        st = req("GET", "/api/status").json
        self.assertEqual(list(st)[0], "station")
        self.assertEqual(st["station"], "Dom Północ")
        self.assertEqual(st["net"]["hostname"], "Dom-P-noc")
        self.assertIsNone(st["net"]["trial"])
        self.assertEqual(st["config"], {"source": "stored", "repairs": 0, "newerSchema": False})
        self.assertTrue(st["importReport"])
        self.assertEqual(req("GET", "/api/status", host="dom-p-noc.local").status, 200)

    def test_hostname_port(self):
        self.assertEqual(mock_api.build_hostname("VdMot"), "VdMot")
        self.assertEqual(mock_api.build_hostname("  "), "VdMot")
        self.assertEqual(mock_api.build_hostname("a_b-c d"), "a_b-c-d")
        self.assertEqual(mock_api.build_hostname("x" * 30), "x" * 20)


class Valves(unittest.TestCase):  # M6
    def test_sensor_position_and_ext(self):
        device()
        v = req("GET", "/api/valves").json["valves"]
        self.assertEqual([s["sensor"] for s in v[2]["sensors"]], [2])  # valve 3: only sensor 2
        self.assertEqual(v[0]["sensors"][0]["sensor"], 1)
        for key in ("calEarlyStop", "calLastFailed"):
            self.assertIn(key, v[0]["ext"])
        self.assertEqual(v[0]["failsafe"], {"state": "off", "pct": 50})

    def test_coast(self):
        device()  # protocol 2: the STM does not report the coast
        v = req("GET", "/api/valves").json["valves"]
        self.assertEqual((v[0]["ext"]["coastOpen"], v[0]["ext"]["coastClose"]), (None, None))
        device(proto=3)
        v = req("GET", "/api/valves").json["valves"]
        self.assertEqual((v[0]["ext"]["coastOpen"], v[0]["ext"]["coastClose"]), (8, 6))
        self.assertEqual((v[11]["ext"]["coastOpen"], v[11]["ext"]["coastClose"]), (0, 0))  # inactive


class Volts(unittest.TestCase):  # M7, E15-1
    def test_formula(self):
        d = device()
        c = d.config["volts"][0]
        c.update(offset=0.5, factor=2.0)
        x = req("GET", "/api/sensors").json["volts"][0]
        self.assertAlmostEqual(x["value"], round((x["raw"] / 100 + 0.5) * 2.0, 3), places=3)


class Inactive(unittest.TestCase):  # M8
    def test_actions_on_inactive_valves(self):
        device()
        for what in ("calibrate", "assembly"):
            self.assertEqual(req("POST", f"/api/valves/12/{what}", {}).status, 202)
        r = req("POST", "/api/valves/12/service-move", {"dir": "open", "counts": 10, "maxmA": 20})
        self.assertEqual(r.status, 202)
        self.assertEqual(req("POST", "/api/valves/12/target", {"target": 1}).json["error"], "inactive")


class Errors(unittest.TestCase):  # M9
    def test_codes(self):
        device()
        self.assertEqual(req("POST", "/api/valves/1/target", raw=b"{x").json["error"], "bad_request")
        self.assertEqual(req("GET", "/api/events?since=x").json, {"error": "bad_request", "detail": "since/limit/valve"})
        self.assertEqual(req("POST", "/api/stm/reset", {}).json["error"], "confirm_required")
        self.assertEqual(req("POST", "/api/system/factory-reset", {}).json["error"], "confirm_required")
        self.assertEqual(req("POST", "/api/stm/flash/abort", {}).json["error"], "idle")
        self.assertEqual(req("GET", "/api/nothing").json, {"error": "not_found", "detail": "/api/nothing"})
        self.assertEqual(req("DELETE", "/api/status").json["error"], "method_not_allowed")


class Answers(unittest.TestCase):  # M10
    def test_queued_and_limits(self):
        device()
        self.assertEqual(req("POST", "/api/valves/detect", {}).json, {"result": "queued"})
        self.assertEqual(req("POST", "/api/config", raw=b" " * 8193).status, 413)
        r = req("POST", "/api/ota/esp", raw=b"--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"a.bin\"\r\n"
                b"\r\n\xe9abc\r\n--b--\r\n", ctype="multipart/form-data; boundary=b")
        self.assertEqual(r.json, {"result": "ok", "restart": True})


class Validation(unittest.TestCase):  # M11
    def test_rules(self):
        device()
        self.assertEqual(req("POST", "/api/config", {"net": {"ssid": "open"}}).status, 200)  # open WiFi
        self.assertEqual(req("POST", "/api/config", {"valves": {"1": {"name": "Küche 1"}}}).status, 200)
        self.assertEqual(req("POST", "/api/config", {"valves": {"1": {"name": "a/b"}}}).status, 400)
        self.assertEqual(req("POST", "/api/config", {"mqtt": {"password": "pässwort"}}).status, 200)


class TrialFilesReport(unittest.TestCase):  # M12
    def test_trial(self):
        d = device()
        self.assertEqual(req("POST", "/api/system/network/confirm", {}).json["error"], "no_trial")
        req("POST", "/api/config", {"net": {"dhcp": False, "ip": "192.168.1.60", "mask": "255.255.255.0",
                                            "gateway": "192.168.1.1"}})
        restart_now(d)
        self.assertGreater(req("GET", "/api/status").json["net"]["trial"]["remainS"], 100)
        self.assertEqual(req("POST", "/api/system/network/confirm", {}).json, {"result": "queued"})
        self.assertIsNone(req("GET", "/api/status").json["net"]["trial"])
        self.assertFalse(d.config["net"]["dhcp"])

    def test_trial_reverts_after_the_window(self):
        d = device()
        req("POST", "/api/config", {"net": {"dhcp": False, "ip": "192.168.1.60", "mask": "255.255.255.0",
                                            "gateway": "192.168.1.1"}})
        restart_now(d)
        d.trial["until"] = time.time() - 1
        d.tick()
        self.assertTrue(d.config["net"]["dhcp"])
        self.assertIsNone(d.trial)

    def test_files(self):
        device()
        f = req("GET", "/api/files").json
        kinds = {x["path"]: (x["kind"], x["deletable"]) for x in f["files"]}
        self.assertEqual(kinds["/sys/cfg.bak"], ("internal", False))
        self.assertEqual(kinds["/VdMot_1.4.9.bin"], ("legacy_image", True))
        self.assertEqual(req("DELETE", "/api/files?path=/sys/cfg.bak").json["error"], "protected")
        self.assertEqual(req("DELETE", "/api/files?path=//x").json["error"], "bad_path")
        self.assertEqual(req("DELETE", "/api/files?path=/VdMot_1.4.9.bin").status, 204)
        self.assertEqual(req("DELETE", "/api/files?path=/VdMot_1.4.9.bin").status, 404)

    def test_import_report(self):
        device()
        self.assertEqual(req("GET", "/api/import-report").status, 404)
        device(import_report=True)
        self.assertEqual(req("GET", "/api/import-report").json["imported"], 57)
        self.assertEqual(req("DELETE", "/api/import-report").status, 204)
        self.assertEqual(req("DELETE", "/api/import-report").status, 404)


class Scenarios(unittest.TestCase):  # M13
    def test_health(self):
        device(scenarios=["health"])
        v = req("GET", "/api/valves").json["valves"]
        self.assertEqual(v[2]["age"], 400)
        self.assertIn("stale", v[2]["health"])
        self.assertEqual(v[3]["sync"], "failed")
        self.assertIsNone(v[4]["sensors"][0]["temp"])

    def test_busy_and_queue(self):
        device(scenarios=["busy"])
        codes = [req("GET", "/api/status").status for _ in range(4)]
        self.assertEqual(codes, [200, 200, 200, 503])
        device(scenarios=["queue"])
        codes = [req("POST", "/api/valves/1/target", {"target": 5}).status for _ in range(2)]
        self.assertEqual(codes, [202, 503])


class Ota(unittest.TestCase):  # M14, E19
    def test_version_bump(self):
        d = device()
        req("POST", "/api/ota/esp", raw=b"--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"a.bin\"\r\n"
            b"\r\n\xe9abc\r\n--b--\r\n", ctype="multipart/form-data; boundary=b")
        self.assertEqual(d.esp_version, "2.1.1-revamped-dev")


class Health(unittest.TestCase):  # M15
    def test_public(self):
        device()
        r = req("GET", "/api/health")
        self.assertEqual(r.status, 200)
        self.assertTrue(r.json["ok"])
        self.assertEqual(set(r.json), {"ok", "version", "uptime", "heap", "tasks", "net", "ota", "log"})


class StopAndSafeMode(unittest.TestCase):  # M16, M17
    def test_stop(self):
        device(proto=2)
        self.assertEqual(req("POST", "/api/valves/1/stop", {}).json["error"], "stm_unsupported")
        device(proto=3)
        self.assertEqual(req("POST", "/api/valves/1/stop", {}).status, 202)
        self.assertEqual(req("POST", "/api/valves/stop", {}).status, 202)

    def test_safe_mode(self):
        d = device(proto=3, scenarios=["safemode"])
        self.assertTrue(req("GET", "/api/status").json["stm"]["status"]["safeMode"])
        self.assertEqual(req("POST", "/api/stm/safe-mode/leave", {}).status, 202)
        self.assertFalse(d.safe_mode)


class Lease(unittest.TestCase):  # M18
    def test_failsafe_states(self):
        device(proto=3, scenarios=["failsafe"])
        st = req("GET", "/api/status").json
        self.assertEqual(st["stm"]["lease"]["state"], "expired")
        self.assertGreater(st["stm"]["lease"]["failsafeMask"], 0)
        v = req("GET", "/api/valves").json["valves"][0]
        self.assertEqual(v["failsafe"]["state"], "lease")
        self.assertIn("failsafe", v["health"])
        device(proto=3)
        self.assertEqual(req("GET", "/api/status").json["stm"]["lease"]["state"], "running")


class TooOld(unittest.TestCase):  # M19
    def test_refused_actions(self):
        device(scenarios=["tooold"])
        self.assertEqual(req("GET", "/api/status").json["stm"]["support"], "too_old")
        r = req("POST", "/api/valves/1/calibrate", {})
        self.assertEqual((r.status, r.json["error"]), (409, "stm_unsupported"))
        self.assertEqual(req("POST", "/api/valves/1/target", {"target": 5}).status, 202)


class Board(unittest.TestCase):  # M20
    def test_board_checks(self):
        d = device()
        img = d.add_image("fw", b"\x00\x40\x00\x20\x09\x00\x00\x08" + b"DEADBEEF BEEFIT VDM-HW:C1" + b"\x00" * 64,
                          scanned_at=0)
        self.assertEqual(req("GET", "/api/stm/images").json[0]["hw"], "C1")
        self.assertEqual(img["hw"], "C1")
        r = req("POST", "/api/stm/flash", {"image": "fw"})
        self.assertEqual(r.json, {"error": "board_mismatch", "detail": "image C1, board C2"})
        r = req("POST", "/api/stm/flash", {"image": "fw", "mode": "blank"})
        self.assertEqual(r.json["error"], "board_required")
        self.assertEqual(req("POST", "/api/stm/flash", {"image": "fw", "mode": "blank", "board": "C1"}).status, 202)
        self.assertEqual(req("GET", "/api/stm/flash").json["boardHw"], "C1")


class Discovery(unittest.TestCase):  # M21
    def test_e24(self):
        d = device()
        d.config["mqtt"]["mode"] = 0
        self.assertEqual(req("POST", "/api/mqtt/discovery", {"action": "publish"}).json,
                         {"error": "disabled", "detail": "MQTT is off"})
        d.config["mqtt"].update(mode=1, separate=False)
        self.assertEqual(req("POST", "/api/mqtt/discovery", {"action": "publish"}).json["error"], "separate_required")
        self.assertEqual(req("POST", "/api/mqtt/discovery", {"action": "delete"}).status, 202)
        d.config["mqtt"]["separate"] = True
        self.assertEqual(req("POST", "/api/mqtt/discovery", {"action": "republish"}).status, 202)


class Log(unittest.TestCase):  # M22
    def test_seq_prefix(self):
        device()
        lines = req("GET", "/api/log").text.splitlines()
        self.assertTrue(lines[0].startswith("#1 "))


class TrialRoutes(unittest.TestCase):  # M23
    def test_revert(self):
        d = device()
        self.assertEqual(req("POST", "/api/system/network/revert", {}).status, 409)
        req("POST", "/api/config", {"net": {"dhcp": False, "ip": "192.168.1.60", "mask": "255.255.255.0",
                                            "gateway": "192.168.1.1"}})
        restart_now(d)
        self.assertEqual(req("POST", "/api/system/network/revert", {}).status, 202)
        self.assertTrue(d.config["net"]["dhcp"])


class RestartRules(unittest.TestCase):  # M24
    def test_port(self):
        d = device()
        a = json.loads(json.dumps(d.config))
        b = json.loads(json.dumps(d.config))
        self.assertEqual(mock_api.restart_reasons(a, b), 0)
        b["station"] = "Other"
        self.assertEqual(mock_api.restart_reasons(a, b), 2)
        b = json.loads(json.dumps(d.config))
        b["net"]["ssid"] = "x"  # Ethernet in both: WiFi fields do not matter
        self.assertEqual(mock_api.restart_reasons(a, b), 0)
        b["net"]["iface"] = 0
        self.assertEqual(mock_api.restart_reasons(a, b), 1)

    def test_rounding(self):
        self.assertEqual(mock_api.round_target(43.5), 44)
        self.assertEqual(mock_api.round_target(0.49), 0)
        self.assertIsNone(mock_api.round_target(100.4))
        self.assertIsNone(mock_api.round_target(-0.4))
        self.assertIsNone(mock_api.round_target(True))
        self.assertIsNone(mock_api.round_target("5"))


class ImageEviction(unittest.TestCase):  # M25
    def test_a_full_store_loses_the_lowest_version_but_the_lowest_of_all(self):
        d = device()

        def image(v):
            return b"\x00\x40\x00\x20\x09\x00\x00\x08DEADBEEF BEEFIT\x00" + v.encode() + b"\x00" * 64

        for n, v in (("stm217", "2.1.7-revamped"), ("stm226", "2.2.6-revamped"), ("stm225", "2.2.5-revamped")):
            d.add_image(n, image(v), scanned_at=0)
        d.add_image("last_good", image("2.2.6-revamped"), scanned_at=0)

        def upload(name, v):
            return req("POST", "/api/stm/images", raw=b"--b\r\nContent-Disposition: form-data; name=\"image\"; "
                       b"filename=\"" + name.encode() + b".bin\"\r\n\r\n" + image(v) + b"\r\n--b--\r\n",
                       ctype="multipart/form-data; boundary=b")

        r = upload("stm227", "2.2.7-revamped")
        self.assertEqual(r.status, 201)
        self.assertEqual(r.json["evicted"], "stm225")
        self.assertEqual(sorted(d.images), ["last_good", "stm217", "stm226", "stm227"])
        # stm227 is not scanned yet
        r = upload("stm228", "2.2.8-revamped")
        self.assertEqual((r.status, r.json["detail"]), (507, "too_many_images"))
        d.images["stm227"]["scanned_at"] = 0
        self.assertEqual(upload("stm228", "2.2.8-revamped").json["evicted"], "stm227")
        self.assertIsNone(upload("stm226", "2.2.6-revamped").json["evicted"])


class Flow(unittest.TestCase):  # the flow layer (Rust 2.2.8)
    OFF = {"enabled": False, "state": "off", "counts": None, "targetCounts": 0, "lpm": None, "targetLpm": None,
           "ms": None, "peak": None}
    CURVE = {"enabled": True, "strokeCounts": 2400, "points": [[630, 0], [888, 1.0], [1180, 2.6]]}

    def test_config_document_and_patch(self):
        device(proto=3)
        r = req("GET", "/api/valves/1/flow")
        self.assertEqual(r.status, 200)
        self.assertEqual(list(r.json), ["valve", "config", "flow", "refDelta"])
        self.assertEqual(r.json["flow"], self.OFF)
        self.assertEqual(r.json["config"]["qmax"], 2.7)
        self.assertEqual(r.json["config"]["minIntervalS"], 60)
        self.assertNotIn("driftD", r.json["config"])
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"], self.OFF)
        r = req("POST", "/api/valves/1/flow", {"enabled": True, "points": [[630, 0], [1180, 2.6]]})
        self.assertEqual((r.status, r.json["detail"]),
                         (400, "strokeCounts 0..10000 (enabled or refSide open needs 1..)"))
        r = req("POST", "/api/valves/1/flow", self.CURVE)
        self.assertEqual(r.status, 200)
        self.assertTrue(r.json["config"]["enabled"])
        self.assertEqual(r.json["config"]["points"], [[630, 0], [888, 1.0], [1180, 2.6]])
        # the lift and the floor of R2 (Rust 2.2.14), in the order of the firmware
        self.assertEqual((r.json["config"]["liftCounts"], r.json["config"]["floorCounts"]), (0, 0))
        keys = list(r.json["config"])
        self.assertEqual(keys[keys.index("preClose"):], ["preClose", "liftCounts", "floorCounts", "points"])
        r = req("POST", "/api/valves/1/flow", {"refSide": "open", "liftCounts": 100, "floorCounts": 700})
        self.assertEqual(r.status, 200)
        self.assertEqual((r.json["config"]["liftCounts"], r.json["config"]["floorCounts"]), (100, 700))
        r = req("POST", "/api/valves/1/flow", {"liftCounts": 1000, "floorCounts": 2399})
        self.assertEqual(r.status, 200)
        self.assertEqual(req("GET", "/api/valves/1/flow").json["config"]["floorCounts"], 2399)
        lift, floor = "liftCounts 0 or 60..1000", "floorCounts 0..10000, below strokeCounts"
        for body, detail in [({"x": 1}, "unknown key"), ({"qmax": 0}, "qmax 0.01..20 l/min"),
                             ({"points": [[1, 0], [1, 1]]}, "points counts 0..10000, increasing"),
                             ({"refSide": "up"}, "refSide closed|open"), ({"enabled": 1}, "enabled true|false"),
                             ({"liftCounts": 1001}, lift), ({"liftCounts": 1.5}, lift), ({"liftCounts": -1}, lift),
                             ({"liftCounts": 59}, lift),
                             ({"floorCounts": 10001}, floor), ({"floorCounts": True}, floor),
                             ({"floorCounts": 2400}, floor),
                             ({"refSide": "closed"}, "liftCounts above 0 needs refSide open"),
                             ({"floorCounts": 0}, "liftCounts above 0 needs floorCounts above 0"),
                             ({"driftD": 1}, "unknown key")]:
            r = req("POST", "/api/valves/1/flow", body)
            self.assertEqual((r.status, r.json["detail"]), (400, detail), body)
        # up to 32 points (Rust 2.2.15)
        pts = [[0, 0]] + [[1070 + 10 * i, round(0.5 + 0.1 * i, 1)] for i in range(31)]
        r = req("POST", "/api/valves/1/flow", {"points": pts})
        self.assertEqual((r.status, r.json["config"]["points"]), (200, pts))
        r = req("POST", "/api/valves/1/flow", {"points": pts + [[1380, 3.6]]})
        self.assertEqual((r.status, r.json["detail"]), (400, "points 2..32 (enabled needs them)"))
        self.assertEqual(req("DELETE", "/api/valves/1/flow").status, 405)
        self.assertEqual(req("GET", "/api/valves/1/flow/counts").status, 405)

    def test_counts_and_targets(self):
        d = device(proto=3)
        d.stm_version = "2.2.7-revamped_C2"
        r = req("POST", "/api/valves/1/flow/counts", {"counts": 700})
        self.assertEqual((r.status, r.json["error"]), (409, "flow_off"))
        req("POST", "/api/valves/1/flow", self.CURVE)
        r = req("POST", "/api/valves/1/flow/counts", {"counts": 700})
        self.assertEqual((r.status, r.json["error"]), (409, "unsupported"))
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"]["state"], "unsupported")
        d.stm_version = "2.2.8-revamped_C2"
        r = req("POST", "/api/valves/1/flow/counts", {"counts": 10001})
        self.assertEqual((r.status, r.json["detail"]), (400, "counts 0..10000"))
        self.assertEqual(req("POST", "/api/valves/1/flow/counts", {"counts": 700}).status, 202)
        v = req("GET", "/api/valves").json["valves"][0]
        self.assertEqual((v["flow"]["state"], v["flow"]["counts"], v["flow"]["lpm"]), ("held", 700, 0.271))
        self.assertEqual(v["pos"], 29)
        # a target is a percentage of the stroke, the position follows
        self.assertEqual(req("POST", "/api/valves/1/target", {"target": 37}).status, 202)
        v = req("GET", "/api/valves").json["valves"][0]
        self.assertEqual((v["target"], v["flow"]["counts"], v["pos"]), (37, 888, 37))
        self.assertEqual(v["flow"]["targetLpm"], None)
        self.assertTrue(any(e["code"] == 416 for e in req("GET", "/api/events").json["events"]))

    def test_flow_target(self):
        d = device(proto=3)
        d.stm_version = "2.2.7-revamped_C2"
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 1})
        self.assertEqual((r.status, r.json["error"]), (409, "flow_off"))
        req("POST", "/api/valves/1/flow", {"enabled": True, "qmax": 2.6, "strokeCounts": 2400,
                                           "points": [[630, 0], [888, 1.0], [1070, 2.3]]})
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 1})
        self.assertEqual((r.status, r.json["error"]), (409, "unsupported"))
        d.stm_version = "2.2.8-revamped_C2"
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 1.0})
        self.assertEqual((r.status, r.json["valve"], r.json["target"], r.json["clipped"]), (202, 1, 37, False))
        v = req("GET", "/api/valves").json["valves"][0]
        self.assertEqual((v["target"], v["flow"]["state"], v["flow"]["targetLpm"]), (37, "held", 1.0))
        # below the first flow above 0 of the curve: that point, clipped (Rust 2.2.15); 0 closes
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 0.5})
        self.assertEqual((r.status, r.json), (202, {"valve": 1, "lpm": 1.0, "target": 37, "clipped": True}))
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"]["counts"], 888)
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 0})
        self.assertEqual((r.status, r.json), (202, {"valve": 1, "lpm": 0, "target": 0, "clipped": False}))
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 2.6})
        self.assertEqual((r.status, r.json), (202, {"valve": 1, "lpm": 2.3, "target": 45, "clipped": True}))
        r = req("POST", "/api/valves/1/flow/target", {"lpm": 2.61})
        self.assertEqual((r.status, r.json), (202, {"valve": 1, "lpm": 2.3, "target": 45, "clipped": True}))
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"]["targetLpm"], 2.6)
        r = req("POST", "/api/valves/1/flow/target", {"lpm": -0.1})
        self.assertEqual((r.status, r.json),
                         (400, {"error": "out_of_range", "detail": "lpm a plain number from 0"}))
        r = req("POST", "/api/valves/1/flow/target", {"x": 1})
        self.assertEqual((r.status, r.json), (400, {"error": "invalid", "detail": "unknown key"}))
        self.assertEqual(req("GET", "/api/valves/1/flow/target").status, 405)

    def test_brake(self):
        d = device(proto=3)
        r = req("POST", "/api/valves/1/brake", {"maxMs": 40, "gapMs": 5})
        self.assertEqual((r.status, r.json), (409, {"error": "stm_unsupported", "detail": "STM 2.2.9 required"}))
        d.stm_version = "2.2.9-revamped_C2"
        r = req("POST", "/api/valves/1/brake", {"maxMs": 40, "gapMs": 5})
        self.assertEqual((r.status, r.json), (200, {"valve": 1, "result": "ok"}))
        r = req("POST", "/api/valves/1/brake", {"maxMs": 40, "gapMs": 0, "mult": 3})
        self.assertEqual(r.status, 200)
        for body in ({"maxMs": 101, "gapMs": 5}, {"maxMs": 40, "gapMs": 101}, {"maxMs": 40, "gapMs": 5, "mult": 11},
                     {"maxMs": 40}):
            r = req("POST", "/api/valves/1/brake", body)
            self.assertEqual((r.status, r.json["detail"]), (400, "maxMs 0..100, gapMs 0..100, mult 0..10"), body)
        r = req("POST", "/api/valves/1/brake", {"maxMs": 40, "gapMs": 0})
        self.assertEqual((r.status, r.json), (400, {"error": "invalid", "detail": "maxMs above 0 needs gapMs or mult"}))
        r = req("POST", "/api/valves/1/brake", {"maxMs": 0, "gapMs": 0})
        self.assertEqual(r.status, 200)
        device(proto=2)
        r = req("POST", "/api/valves/1/brake", {"maxMs": 0, "gapMs": 0})
        self.assertEqual((r.status, r.json["error"]), (409, "stm_unsupported"))


class FlowScenario(unittest.TestCase):  # M26, what the dashboard's curve and Stored view read
    VALVE_KEYS = ["idx", "name", "active", "known", "state", "stateKey", "calibrating", "pos", "target",
                  "targetSource", "sync", "stmTarget", "meanCur", "moves", "oc", "cc", "dc", "cr", "health", "age",
                  "sensors", "ext", "failsafe", "calibrationEnd", "flow"]

    def test_valves_keep_their_members(self):
        # /api/valves has about 300 B left of the 16 KiB of the firmware: new data goes elsewhere
        device(proto=3, scenarios=("flow",))
        for v in req("GET", "/api/valves").json["valves"]:
            self.assertEqual(list(v), self.VALVE_KEYS)

    def test_curves(self):
        d = device(proto=3, scenarios=("flow",))
        active = [i for i, v in enumerate(d.valves) if v["active"]]
        self.assertEqual(len(active), 8)
        for i in active[:-1]:
            doc = req("GET", f"/api/valves/{i + 1}/flow").json
            pts = doc["config"]["points"]
            self.assertEqual((doc["config"]["enabled"], doc["config"]["refSide"], len(pts)), (True, "open", 14))
            self.assertLess(pts[-1][0], doc["config"]["strokeCounts"])
            self.assertEqual(pts[0][1], 0)
            self.assertLessEqual(pts[-1][1], doc["config"]["qmax"])
            # the firmware's rules hold: a curve it would refuse shows nothing real
            self.assertIsNone(d.flow_patch(dict(doc["config"]), {"points": pts}))
        last = req("GET", f"/api/valves/{active[-1] + 1}/flow").json
        self.assertEqual((last["config"]["enabled"], last["config"]["points"], last["flow"]["state"]),
                         (False, [], "off"))

    def test_states_of_the_rows(self):
        device(proto=3, scenarios=("flow",))
        vs = req("GET", "/api/valves").json["valves"]
        self.assertEqual((vs[0]["flow"]["state"], vs[0]["flow"]["targetLpm"]), ("held", 0.4))
        self.assertAlmostEqual(vs[0]["flow"]["lpm"], 0.4, delta=0.01)
        self.assertEqual((vs[1]["flow"]["state"], vs[1]["flow"]["targetLpm"], vs[1]["pos"]), ("held", None, 30))
        self.assertEqual((vs[2]["flow"]["state"], vs[2]["flow"]["counts"], vs[2]["flow"]["lpm"]),
                         ("failed", None, None))
        self.assertEqual(req("GET", "/api/valves/1/flow").json["refDelta"], 18)

    def test_version_order_is_numeric(self):
        # "2.2.12" sorts below "2.2.8" as text: the layer of an STM 2.2.12 is not unsupported
        d = device(proto=3, scenarios=("flow",))
        self.assertEqual(d.stm_version, mock_api.STM_VERSION_FLOW)
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"]["state"], "held")
        d.stm_version = "2.2.7-revamped_C2"
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"]["state"], "unsupported")

    def test_the_scenario_is_for_protocol_3(self):
        d = device(proto=2, scenarios=("flow",))
        self.assertEqual(d.stm_version, mock_api.STM_VERSION_V2)
        self.assertEqual(req("GET", "/api/valves").json["valves"][0]["flow"]["state"], "unsupported")


if __name__ == "__main__":
    unittest.main(verbosity=1)
