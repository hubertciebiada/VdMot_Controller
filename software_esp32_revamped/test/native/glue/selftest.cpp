// Self-test of the ESP glue harness: process isolation, multi-boot hand-over of the persistent
// stores, the runner invariants, and the behaviour of the fakes the glue relies on. The cases of
// the suite "xfail" must fail; selftest.sh runs them one by one and checks the verdicts.
#include <string.h>
#include <sys/time.h>
#include <time.h>

#include <ETH.h>
#include <LittleFS.h>
#include <Preferences.h>
#include <PubSubClient.h>
#include <Update.h>
#include <WiFi.h>
#include <esp_attr.h>
#include <esp_ota_ops.h>
#include <freertos/FreeRTOS.h>
#include <freertos/queue.h>
#include <freertos/semphr.h>
#include <nvs.h>

#include <string>

#include "fake_stm.h"
#include "glue_test.h"

namespace {

bool g_touched = false;
RTC_NOINIT_ATTR uint32_t g_rtcWord;

// Answers every request with 200 "<method> <url>", records the body segments and uploads.
class EchoHandler : public AsyncWebHandler {
 public:
  bool canHandle(AsyncWebServerRequest* req) override {
    req->addInterestingHeader("X-Keep");
    return true;
  }
  void handleRequest(AsyncWebServerRequest* req) override {
    req->onDisconnect([this]() { ++disconnects; });
    if (silent) return;
    req->send(200, "text/plain", String(req->methodToString()) + " " + req->url());
    if (twice) req->send(200, "text/plain", "again");
  }
  void handleBody(AsyncWebServerRequest*, uint8_t* data, size_t len, size_t index,
                  size_t total) override {
    bodies.push_back({std::string(reinterpret_cast<char*>(data), len), index, total});
  }
  void handleUpload(AsyncWebServerRequest*, const String& filename, size_t index, uint8_t* data,
                    size_t len, bool final) override {
    uploads.push_back(
        {filename.str(), index, std::string(reinterpret_cast<char*>(data), len), final});
  }
  bool isRequestHandlerTrivial() override { return false; }

  struct Body {
    std::string data;
    size_t index;
    size_t total;
  };
  struct Upload {
    std::string filename;
    size_t index;
    std::string data;
    bool final;
  };
  std::vector<Body> bodies;
  std::vector<Upload> uploads;
  int disconnects = 0;
  bool silent = false;
  bool twice = false;
};

}  // namespace

// ---------------------------------------------------------------- isolation and boots

TEST_CASE("isolation: a static set in one case") {
  glue::begin();
  g_touched = true;
  CHECK(g_touched);
}

TEST_CASE("isolation: is unset in the next case") {
  glue::begin();
  CHECK_FALSE(g_touched);
}

TEST_CASE("boots: NVS, LittleFS and the RTC section of boot 0 are there at boot 1") {
  glue::begin();
  if (testkit::boot() == 0) {
    CHECK(g_rtcWord == 0xA5A5A5A5u);  // power-on content of RTC slow memory
    CHECK(esp_reset_reason() == ESP_RST_POWERON);
    g_rtcWord = 0x12345678u;
    fakes::nvs().setU32("vdmrev", "boots", 7);
    fakes::fs().put("/log/events.log", "line\n");
    testkit::reboot(testkit::Reset::Software);
  }
  CHECK(testkit::boot() == 1);
  CHECK(esp_reset_reason() == ESP_RST_SW);
  CHECK(g_rtcWord == 0x12345678u);
  CHECK(fakes::nvs().getU("vdmrev", "boots") == 7);
  CHECK(fakes::fs().read("/log/events.log") == "line\n");
  CHECK_FALSE(fakes::fs().mounted);  // mounted again by the firmware after every boot
}

TEST_CASE("boots: a power-on clears the RTC section and keeps NVS and flash") {
  glue::begin();
  if (testkit::boot() == 0) {
    g_rtcWord = 1;
    fakes::nvs().setU8("vdmrev", "imported", 1);
    testkit::reboot(testkit::Reset::PowerOn);
  }
  CHECK(esp_reset_reason() == ESP_RST_POWERON);
  CHECK(g_rtcWord == 0xA5A5A5A5u);
  CHECK(fakes::nvs().getU("vdmrev", "imported") == 1);
  CHECK(glue::rtcSnapshot().size() >= sizeof g_rtcWord);
}

TEST_CASE("boots: esp_restart() inside glue::run() goes on at the next boot") {
  glue::begin();
  if (testkit::boot() == 0) {
    fakes::nvs().setU8("vdmrev", "k", 3);
    glue::run([] { esp_restart(); });
    FAIL("esp_restart() returned");
  }
  CHECK(esp_reset_reason() == ESP_RST_SW);
  CHECK(fakes::nvs().getU("vdmrev", "k") == 3);
}

TEST_CASE("boots: a pin and a watchdog reset give ESP_RST_EXT and ESP_RST_TASK_WDT") {
  glue::begin();
  if (testkit::boot() == 0) testkit::reboot(testkit::Reset::Pin);
  if (testkit::boot() == 1) {
    CHECK(esp_reset_reason() == ESP_RST_EXT);
    testkit::reboot(testkit::Reset::Watchdog);
  }
  CHECK(esp_reset_reason() == ESP_RST_TASK_WDT);
}

TEST_CASE("ota: a new image boots pending, an unconfirmed one is rolled back") {
  glue::begin();
  if (testkit::boot() == 0) {
    REQUIRE(Update.begin(UPDATE_SIZE_UNKNOWN, U_FLASH));
    uint8_t data[4] = {0xE9, 1, 2, 3};
    CHECK(Update.write(data, 4) == 4);
    REQUIRE(Update.end(true));
    CHECK(fakes::ota().state[1] == ESP_OTA_IMG_NEW);
    testkit::reboot(testkit::Reset::Software);
  }
  if (testkit::boot() == 1) {
    esp_ota_img_states_t st;
    REQUIRE(esp_ota_get_state_partition(esp_ota_get_running_partition(), &st) == ESP_OK);
    CHECK(st == ESP_OTA_IMG_PENDING_VERIFY);
    CHECK(esp_ota_get_running_partition()->address == 0x150000u);
    testkit::reboot(testkit::Reset::Software);  // not confirmed
  }
  CHECK(fakes::ota().running == 0);
  CHECK(fakes::ota().state[1] == ESP_OTA_IMG_ABORTED);
}

TEST_CASE("ota: a rollback without a valid image to go back to returns ESP_FAIL") {
  glue::begin();
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  fakes::ota().state[1] = ESP_OTA_IMG_UNDEFINED;
  CHECK(esp_ota_mark_app_invalid_rollback_and_reboot() == ESP_FAIL);
  fakes::ota().state[1] = ESP_OTA_IMG_VALID;
  CHECK_THROWS_AS(esp_ota_mark_app_invalid_rollback_and_reboot(), fakes::Restarted);
  CHECK(fakes::ota().boot == 1);
}

// ---------------------------------------------------------------- RTOS and time

TEST_CASE("rtos: a queue is a ring of the caller's storage") {
  glue::begin();
  static StaticQueue_t q;
  static uint8_t items[3 * sizeof(int)];
  QueueHandle_t h = xQueueCreateStatic(3, sizeof(int), items, &q);
  for (int i = 1; i <= 3; ++i) CHECK(xQueueSend(h, &i, 0) == pdTRUE);
  const int four = 4;
  CHECK(xQueueSend(h, &four, 0) == errQUEUE_FULL);
  int out = 0;
  CHECK(xQueueReceive(h, &out, 0) == pdTRUE);
  CHECK(out == 1);
  CHECK(xQueueSend(h, &four, 0) == pdTRUE);
  for (int want : {2, 3, 4}) {
    CHECK(xQueueReceive(h, &out, 0) == pdTRUE);
    CHECK(out == want);
  }
  CHECK(xQueueReceive(h, &out, 0) == pdFALSE);
}

TEST_CASE("rtos: vTaskDelay advances the clock and ends a loop after stopAfterYields") {
  glue::begin();
  fakes::rtos().stopAfterYields(3);
  int loops = 0;
  CHECK_THROWS_AS(
      [&] {
        for (;;) {
          ++loops;
          vTaskDelay(pdMS_TO_TICKS(100));
        }
      }(),
      fakes::YieldLimit);
  CHECK(loops == 3);
  CHECK(millis() == 300);
  CHECK(fakes::rtos().delays == std::vector<uint32_t>{100, 100, 100});
}

TEST_CASE("time: millis() wraps at 32 bits, the wall clock follows the fake clock") {
  glue::begin();
  fakes::setMs(0xFFFFF000ull);
  fakes::advanceMs(0x1000 + 5);
  CHECK(millis() == 5);
  CHECK(time(nullptr) == static_cast<time_t>((0xFFFFF000ull + 0x1005) / 1000));
  fakes::setWallClock(1767225600);  // 2026-01-01T00:00:00Z
  fakes::advanceMs(1500);
  struct timeval tv;
  gettimeofday(&tv, nullptr);
  CHECK(tv.tv_sec == 1767225601);
  CHECK(tv.tv_usec == 500000);
}

// ---------------------------------------------------------------- NVS, Preferences, LittleFS

TEST_CASE("nvs: typed lookup, length query, small buffer, read-only open of a missing namespace") {
  glue::begin();
  fakes::nvs().setU32("legacy", "n", 5);
  fakes::nvs().setBlob("legacy", "b", {1, 2, 3});
  nvs_handle_t h;
  CHECK(nvs_open("nothere", NVS_READONLY, &h) == ESP_ERR_NVS_NOT_FOUND);
  REQUIRE(nvs_open("legacy", NVS_READONLY, &h) == ESP_OK);
  uint8_t u8 = 0;
  CHECK(nvs_get_u8(h, "n", &u8) == ESP_ERR_NVS_NOT_FOUND);
  uint32_t u32 = 0;
  CHECK(nvs_get_u32(h, "n", &u32) == ESP_OK);
  CHECK(u32 == 5);
  size_t len = 0;
  CHECK(nvs_get_blob(h, "b", nullptr, &len) == ESP_OK);
  CHECK(len == 3);
  uint8_t small[2];
  len = sizeof small;
  CHECK(nvs_get_blob(h, "b", small, &len) == ESP_ERR_NVS_INVALID_LENGTH);
  CHECK(nvs_set_u8(h, "x", 1) == ESP_ERR_NVS_READ_ONLY);
  nvs_close(h);
  CHECK(fakes::nvs().openHandles == 0);
}

TEST_CASE("preferences: return values of the library") {
  glue::begin();
  Preferences p;
  REQUIRE(p.begin("vdmrev", false));
  CHECK(p.putUChar("a", 1) == 1);
  CHECK(p.putULong("b", 7) == 4);
  CHECK(p.putLong64("c", -9) == 8);
  const uint8_t blob[5] = {1, 2, 3, 4, 5};
  CHECK(p.putBytes("d", blob, 5) == 5);
  CHECK(p.getUChar("b", 42) == 42);  // stored as u32
  CHECK(p.getULong("b", 0) == 7);
  CHECK(p.getLong64("c", 0) == -9);
  uint8_t out[4];
  CHECK(p.getBytesLength("d") == 5);
  CHECK(p.getBytes("d", out, sizeof out) == 0);  // too small
  CHECK(p.remove("a"));
  CHECK_FALSE(p.remove("a"));
  CHECK(p.putUChar("toolongkeyname16", 1) == 0);
  fakes::nvs().failSet.insert("e");
  CHECK(p.putUChar("e", 1) == 0);
  CHECK(p.clear());
  CHECK_FALSE(p.isKey("b"));
}

TEST_CASE("littlefs: open modes, parents, rename, remove, capacity") {
  glue::begin();
  REQUIRE(LittleFS.begin(false));
  CHECK_FALSE(LittleFS.open("/nodir/x.txt", FILE_WRITE));
  CHECK_FALSE(LittleFS.open("/missing.txt", FILE_READ));
  File f = LittleFS.open("/a.txt", FILE_WRITE);
  REQUIRE(f);
  CHECK(f.write(reinterpret_cast<const uint8_t*>("abc"), 3) == 3);
  f.close();
  f = LittleFS.open("/a.txt", FILE_APPEND);
  CHECK(f.write('d') == 1);
  f.close();
  CHECK(fakes::fs().read("/a.txt") == "abcd");
  f = LittleFS.open("/a.txt", FILE_WRITE);
  f.close();
  CHECK(fakes::fs().read("/a.txt").empty());
  fakes::fs().put("/b.txt", "B");
  CHECK(LittleFS.rename("/b.txt", "/a.txt"));
  CHECK(fakes::fs().read("/a.txt") == "B");
  CHECK_FALSE(LittleFS.exists("/b.txt"));
  CHECK_FALSE(LittleFS.remove("/b.txt"));
  CHECK(LittleFS.mkdir("/d"));
  CHECK_FALSE(LittleFS.mkdir("/d"));
  fakes::fs().fail("rename", "/a.txt");
  CHECK_FALSE(LittleFS.rename("/a.txt", "/c.txt"));
  CHECK(LittleFS.rename("/a.txt", "/c.txt"));
  fakes::fs().totalBytes = LittleFS.usedBytes() + 4096;
  f = LittleFS.open("/big.bin", FILE_WRITE);  // takes the last free block
  const std::string data(5000, 'x');
  CHECK(f.write(reinterpret_cast<const uint8_t*>(data.data()), data.size()) == 4096);
  f.close();
  File dir = LittleFS.open("/");
  REQUIRE(dir.isDirectory());
  std::vector<std::string> names;
  for (File e = dir.openNextFile(); e; e = dir.openNextFile()) names.push_back(e.name());
  CHECK(names == std::vector<std::string>{"big.bin", "c.txt", "d"});
}

// ---------------------------------------------------------------- UART, GPIO, network, MQTT

TEST_CASE("uart: bytes arrive at their time and a full RX ring drops the rest") {
  glue::begin();
  Serial2.setRxBufferSize(200);
  Serial2.begin(115200, SERIAL_8N1, 5, 17);
  fakes::serial(2).inject("ab", 10);
  CHECK(Serial2.available() == 0);
  fakes::advanceMs(10);
  CHECK(Serial2.available() == 2);
  fakes::serial(2).inject(std::string(300, 'x'));
  CHECK(Serial2.available() == 200);
  CHECK(fakes::serial(2).rxDropped == 102);
  CHECK(Serial2.setRxBufferSize(4096) == 0);  // only before begin()
}

TEST_CASE("network: events reach the registered callbacks") {
  glue::begin();
  std::vector<arduino_event_id_t> seen;
  WiFi.onEvent([&seen](arduino_event_id_t e, arduino_event_info_t) { seen.push_back(e); });
  WiFi.onEvent([&seen](arduino_event_id_t e, arduino_event_info_t) { seen.push_back(e); },
               ARDUINO_EVENT_ETH_GOT_IP);
  fakes::net().fire(ARDUINO_EVENT_ETH_START);
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
  CHECK(seen == std::vector<arduino_event_id_t>{ARDUINO_EVENT_ETH_START, ARDUINO_EVENT_ETH_GOT_IP,
                                                ARDUINO_EVENT_ETH_GOT_IP});
  fakes::net().ethIp = IPAddress(192, 168, 1, 7);
  CHECK(static_cast<uint32_t>(ETH.localIP()) == 0x0701A8C0u);
}

TEST_CASE("mqtt: publish needs a session and room in the buffer, loop delivers one message") {
  glue::begin();
  WiFiClient net;
  PubSubClient c(net);
  CHECK_FALSE(c.publish("t", "x", false));
  REQUIRE(c.setBufferSize(20));
  REQUIRE(c.connect("id", nullptr, nullptr, "lwt", 0, true, "offline"));
  CHECK(c.publish("topic", "01234567", true));        // 5 + 2 + 5 + 8 = 20
  CHECK_FALSE(c.publish("topic", "012345678", true));  // 21 > 20
  std::vector<std::string> got;
  c.setCallback([&got](char* t, uint8_t* p, unsigned int n) {
    got.push_back(std::string(t) + "=" + std::string(reinterpret_cast<char*>(p), n));
  });
  fakes::mqtt().inbox.push_back({"a", "1", false});
  fakes::mqtt().inbox.push_back({"b", "2", false});
  CHECK(c.loop());
  CHECK(got == std::vector<std::string>{"a=1"});
  net.stop();  // the broker socket: the session is gone
  CHECK_FALSE(c.connected());
  CHECK(c.state() == MQTT_CONNECTION_LOST);
}

// ---------------------------------------------------------------- web request driver

TEST_CASE("http: headers are filtered, the body arrives in segments, the client disconnects") {
  glue::begin();
  AsyncWebServer server(80);
  EchoHandler h;
  server.addHandler(&h);
  fakes::http::Request r = fakes::http::post("/api/x?a=1", std::string(3000, 'j'));
  r.header("X-Keep", "1").header("X-Drop", "2");
  fakes::http::Exchange e(r);
  CHECK(e.request()->hasHeader("X-Keep"));
  CHECK_FALSE(e.request()->hasHeader("X-Drop"));
  CHECK(e.request()->getParam("a")->value() == "1");
  const fakes::http::Response& res = e.finish();
  CHECK(res.code == 200);
  CHECK(res.body == "POST /api/x");
  REQUIRE(h.bodies.size() == 3);
  CHECK(h.bodies[0].data.size() == 1460);
  CHECK(h.bodies[2].index == 2920);
  CHECK(h.bodies[2].total == 3000);
  CHECK(h.disconnects == 1);
}

TEST_CASE("http: an upload arrives in pieces of at most 1460 bytes and a final call") {
  glue::begin();
  AsyncWebServer server(80);
  EchoHandler h;
  server.addHandler(&h);
  std::string file(3000, 'f');
  file[2999] = 'e';
  fakes::http::Request r = fakes::http::upload("/api/up", "fw.bin", file, {{"md5", "abc"}});
  r.segment = 100000;  // one segment: the pieces are cut by the 1460-byte buffer only
  const fakes::http::Response res = fakes::http::perform(r);
  CHECK(res.code == 200);
  REQUIRE(h.uploads.size() == 3);
  CHECK(h.uploads[0].data.size() == 1460);
  CHECK(h.uploads[1].index == 1460);
  CHECK(h.uploads[2].final);
  CHECK(h.uploads[2].data.size() == 80);
  CHECK(h.uploads[2].data.back() == 'e');
  CHECK(h.bodies.empty());
}

TEST_CASE("http: failNextResponse makes the next beginResponse() return nullptr") {
  glue::begin();
  AsyncWebServer server(80);
  EchoHandler h;
  h.silent = true;
  server.addHandler(&h);
  fakes::http::server().failNextResponse = true;
  fakes::http::Exchange e(fakes::http::get("/"));
  CHECK(e.request()->beginResponse(200, "text/plain", "x") == nullptr);
  CHECK(e.request()->beginResponse(200, "text/plain", "x") != nullptr);
  e.request()->send(204);
  CHECK(e.finish().code == 204);
}

// ---------------------------------------------------------------- fake STM

TEST_CASE("fake stm: answers by protocol and holds still in reset") {
  glue::begin();
  glue::FakeStm stm;
  Serial2.begin(115200, SERIAL_8N1, 5, 17);
  auto ask = [](const char* line) {
    Serial2.write(reinterpret_cast<const uint8_t*>(line), strlen(line));
    fakes::advanceMs(10);
    std::string out;
    while (Serial2.available() > 0) out.push_back(static_cast<char>(Serial2.read()));
    return out;
  };
  CHECK(ask("gproto\r\n") == "gproto 2\r\n");
  CHECK(ask("gvlvd 7\r\n") == "gvlvd 7 42 18 1 215 -500 57 3120 3350 230 0 \r\n");
  stm.protocol(1);
  CHECK(ask("gproto\r\n").empty());
  digitalWrite(15, HIGH);
  CHECK(ask("gvers\r\n").empty());
  digitalWrite(15, LOW);
  CHECK(stm.resets() == 1);
  fakes::advanceMs(stm.bootMs);
  CHECK(ask("gvers\r\n") == "gvers 1.4.9_Dev_C2 1712345678 \r\n");
  stm.tooOld(true);
  CHECK(ask("gvers\r\n") == "gvers 1.3.5_C2\r\n");
  CHECK(ask("ghwin\r\n").empty());
  CHECK(stm.requestsOf("gvers").size() == 3);
}

// ---------------------------------------------------------------- cases that must fail

TEST_CASE("xfail: an assertion" * doctest::test_suite("xfail")) {
  glue::begin();
  CHECK(1 == 2);
}

TEST_CASE("xfail: a second take of a held mutex" * doctest::test_suite("xfail")) {
  glue::begin();
  static StaticSemaphore_t storage;
  SemaphoreHandle_t m = xSemaphoreCreateMutexStatic(&storage);
  xSemaphoreTake(m, portMAX_DELAY);
  xSemaphoreTake(m, portMAX_DELAY);  // deadlock on the target
  xSemaphoreGive(m);
}

TEST_CASE("xfail: a critical section left entered" * doctest::test_suite("xfail")) {
  glue::begin();
  static portMUX_TYPE mux = portMUX_INITIALIZER_UNLOCKED;
  portENTER_CRITICAL(&mux);
}

TEST_CASE("xfail: an HTTP request without an answer" * doctest::test_suite("xfail")) {
  glue::begin();
  AsyncWebServer server(80);
  EchoHandler h;
  h.silent = true;
  server.addHandler(&h);
  fakes::http::perform(fakes::http::get("/api/status"));
}

TEST_CASE("xfail: an HTTP request answered twice" * doctest::test_suite("xfail")) {
  glue::begin();
  AsyncWebServer server(80);
  EchoHandler h;
  h.twice = true;
  server.addHandler(&h);
  fakes::http::perform(fakes::http::get("/"));
}

TEST_CASE("xfail: the topic of an MQTT callback is gone after a publish in it" *
          doctest::test_suite("xfail")) {
  glue::begin();
  WiFiClient net;
  PubSubClient c(net);
  REQUIRE(c.connect("id"));
  size_t len = 0;
  c.setCallback([&c, &len](char* topic, uint8_t*, unsigned int) {
    c.publish("reply", "1");
    len = strlen(topic);  // use after the library reused its buffer: ASan
  });
  fakes::mqtt().inbox.push_back({"cmd/target", "50", false});
  c.loop();
  CHECK(len == 10);
}
