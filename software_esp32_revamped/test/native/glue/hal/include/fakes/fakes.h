// Test side of the fake Arduino-ESP32 / ESP-IDF / library layer of the ESP glue tests: scripted
// inputs and recorded outputs of every fake. Included by tests, sibling fakes and support code
// only (STL allowed); the glue sees the framework headers of this directory.
//
// Persistent stores - NVS, the LittleFS tree, the RTC_NOINIT_ATTR section and the OTA partition
// states - belong to the device: the runner hooks (glue/runner_hooks.cpp) hand them to the next
// boot of a case, fakes::reset() leaves them alone. Everything else is volatile.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <deque>
#include <functional>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "Arduino.h"
#include "ETH.h"
#include "FS.h"
#include "PubSubClient.h"
#include "Update.h"
#include "WiFi.h"
#include "esp_ota_ops.h"
#include "esp_partition.h"
#include "esp_sntp.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"
#include "ping/ping_sock.h"

namespace fakes {

// ---------------------------------------------------------------- calls that do not return

struct Restarted {};    // esp_restart(), ESP.restart(), a rollback that reboots
struct YieldLimit {};   // vTaskDelay()/delay() after rtos().stopAfterYields(n) was used up
struct TaskDeleted {};  // vTaskDelete(nullptr): the calling task ends

// Volatile state of every fake back to the start of a boot; the persistent stores stay.
void reset();

// ---------------------------------------------------------------- journal

// One ordered list of side effects across all fakes and sibling fakes, e.g. "gpio 15=1",
// "delay 100", "esp_restart", "logger.service 0". vTaskDelay is not journaled (see
// rtos().delays): task loops would flood it.
std::vector<std::string>& journal();
void note(const std::string& entry);
// Index of the first entry equal to `entry` at or after `from`; -1 when there is none.
int find(const std::string& entry, int from = 0);
// Entries starting with `prefix`, in order.
std::vector<std::string> journalOf(const std::string& prefix);

// ---------------------------------------------------------------- time

// Monotonic fake clock since boot. millis() returns its low 32 bits (setMs(0xFFFFF000) tests
// the wrap), esp_timer_get_time() the microseconds.
uint64_t nowMs();
uint64_t nowUs();
void setMs(uint64_t ms);
void advanceMs(uint64_t ms);
void advanceUs(uint64_t us);
// Low 32 bits of nowMs(), kept current for readers that hold a reference (the STM simulator).
const uint32_t& ms32();
// Wall clock (gettimeofday(), time()): `epoch` seconds now, advancing with the fake clock. Before
// any call it is 1970-01-01 plus the time since boot, like an ESP32 before SNTP.
void setWallClock(int64_t epoch, int64_t usec = 0);
int64_t wallClockUs();

// ---------------------------------------------------------------- GPIO

constexpr int kPins = 40;
struct Gpio {
  uint8_t mode[kPins] = {};
  uint8_t level[kPins] = {};  // output latch
  uint8_t in[kPins];          // input level without an input hook (default HIGH)
  // Input level of a pin at the fake time; < 0 uses in[pin].
  std::function<int(int pin, uint64_t nowMs)> input;
  // Every digitalWrite (after the latch changed).
  std::function<void(int pin, uint8_t level)> onWrite;
  std::vector<std::pair<int, uint8_t>> writes;
  Gpio() {
    for (uint8_t& v : in) v = HIGH;
  }
};
Gpio& gpio();

// ---------------------------------------------------------------- serial ports

// Something on the other end of a UART (the fake STM).
class SerialPeer {
 public:
  virtual ~SerialPeer() = default;
  virtual void onBegin(uint32_t baud, uint32_t config) {
    (void)baud;
    (void)config;
  }
  virtual void onEnd() {}
  virtual void onTx(const uint8_t* data, size_t len) = 0;
  // Before every look at the RX side: move bytes that are due into the port.
  virtual void poll() {}
};

struct SerialPort {
  bool open = false;
  uint32_t baud = 0;
  uint32_t config = 0;
  int rxPin = -1;
  int txPin = -1;
  size_t rxBufferSize = 256;  // driver defaults of Arduino-ESP32 2.0.7
  size_t txBufferSize = 0;
  int begins = 0;
  int ends = 0;
  // Received bytes with the fake time (ms) they arrive; at most rxBufferSize due bytes are
  // kept, later ones are dropped and counted (a UART RX ring overflow).
  std::deque<std::pair<uint64_t, uint8_t>> rx;
  size_t rxDropped = 0;
  std::string tx;                   // every written byte
  std::vector<std::string> lines;   // println() lines (without CR LF)
  size_t txAccept = SIZE_MAX;       // write() accepts at most this many bytes per call
  SerialPeer* peer = nullptr;
  // Queues bytes to arrive at `atMs` (default: now).
  void inject(const std::string& bytes, uint64_t atMs = UINT64_MAX);
  std::string takeTx();             // tx since the last takeTx(), then cleared
};
SerialPort& serial(int n);  // 0 = Serial, 1 = Serial1, 2 = Serial2

// ---------------------------------------------------------------- chip

struct Esp {
  uint32_t heapSize = 327680;
  uint32_t freeHeap = 150000;
  uint32_t minFreeHeap = 120000;
  uint32_t maxAllocHeap = 110000;   // ESP.getMaxAllocHeap(), heap_caps_get_largest_free_block()
  uint32_t sketchSize = 1091984;
  uint32_t freeSketchSpace = 1310720;
  esp_reset_reason_t resetReason = ESP_RST_POWERON;
  uint8_t mac[6] = {0x24, 0x0A, 0xC4, 0x12, 0x34, 0x56};  // factory MAC (base, WiFi STA)
  int restarts = 0;
  // task watchdog
  esp_err_t wdtInitResult = ESP_OK;
  uint32_t wdtTimeoutS = 0;
  bool wdtPanic = false;
  int wdtInits = 0;
  int wdtAdds = 0;
  int wdtResets = 0;
  std::vector<TaskHandle_t> wdtTasks;
};
Esp& esp();

// ---------------------------------------------------------------- FreeRTOS

struct TaskRecord {
  TaskFunction_t fn;
  std::string name;
  uint32_t stackBytes;
  UBaseType_t priority;
  BaseType_t core;
  void* arg;
  TaskHandle_t handle;
};
struct Rtos {
  BaseType_t createResult = pdPASS;
  std::vector<TaskRecord> tasks;
  std::vector<uint32_t> delays;       // ticks of every vTaskDelay (delay() counts as one too)
  uint64_t delayCount = 0;            // all yields, also beyond the stored ones
  size_t delayStoreLimit = 100000;
  long yieldsLeft = -1;               // stopAfterYields(); -1 = no limit
  int violations = 0;
  std::vector<std::string> violationLog;
  int criticalDepth = 0;              // open portENTER_CRITICAL sections
  std::vector<TaskHandle_t> deleted;
  std::map<std::string, UBaseType_t> stackHighWater;   // bytes per task name
  std::map<std::string, TaskHandle_t> extraHandles;    // xTaskGetHandle() of unrecorded tasks
  TaskHandle_t current = nullptr;                      // xTaskGetCurrentTaskHandle()
  std::function<void(uint32_t ticks)> onDelay;         // after the clock advanced
  // The n-th following yield (vTaskDelay or delay) throws YieldLimit; 1 = the next one.
  void stopAfterYields(long n) { yieldsLeft = n; }
  void violation(const std::string& what);
};
Rtos& rtos();

// ---------------------------------------------------------------- LittleFS

struct FsNode {
  bool dir = false;
  std::vector<uint8_t> data;
};
struct FsFailure {
  std::string op;    // open, read, write, seek, rename, remove, mkdir, rmdir
  std::string path;  // "" = any path
  int count;         // operations that still fail
};
struct Fs {
  // persistent
  bool formatted = true;                 // the partition holds a LittleFS
  std::map<std::string, FsNode> nodes;   // absolute path -> node ("/" is implicit)
  // volatile
  bool mounted = false;
  bool mountOk = true;                   // begin() of a formatted partition
  bool formatOk = true;
  int begins = 0;
  int formats = 0;
  size_t totalBytes = 0x170000;          // spiffs partition of default.csv
  size_t blockSize = 4096;
  size_t baseUsedBytes = 2 * 4096;       // superblocks
  std::vector<FsFailure> failures;
  // wear counters
  int opens = 0;
  int writeOpens = 0;                    // opens with "w" or "a"
  int writes = 0;
  size_t bytesWritten = 0;
  int renames = 0;
  int removes = 0;
  int openHandles = 0;                   // File objects currently open
  // Called with the path before every write to an open file (another task running meanwhile).
  std::function<void(const std::string& path)> onWrite;

  // The next `count` operations `op` on `path` ("" = any) fail.
  void fail(const std::string& op, const std::string& path = "", int count = 1);
  bool shouldFail(const std::string& op, const std::string& path);
  void put(const std::string& path, const std::string& content);  // parents created
  void mkdirs(const std::string& path);
  bool exists(const std::string& path) const;
  std::string read(const std::string& path) const;                 // "" when missing
  size_t usedBytes() const;
};
Fs& fs();

// ---------------------------------------------------------------- NVS

enum class NvsType : uint8_t { U8, I8, U16, I16, U32, I32, U64, I64, Str, Blob };
struct NvsEntry {
  NvsType type;
  std::vector<uint8_t> bytes;  // little endian; Str with the terminating NUL
};
struct Nvs {
  // persistent
  std::map<std::string, std::map<std::string, NvsEntry>> ns;
  // volatile
  bool initOk = true;                      // false: every open fails (NOT_INITIALIZED)
  std::set<std::string> failOpen;          // namespaces whose open fails
  std::set<std::string> failSet;           // keys whose set fails (NOT_ENOUGH_SPACE)
  bool failCommit = false;
  bool failErase = false;
  std::map<std::string, int> sets;         // successful sets per "ns/key" (wear)
  int opens = 0;
  int openHandles = 0;

  void setU8(const std::string& n, const std::string& key, uint8_t v);
  void setI8(const std::string& n, const std::string& key, int8_t v);
  void setU16(const std::string& n, const std::string& key, uint16_t v);
  void setI16(const std::string& n, const std::string& key, int16_t v);
  void setU32(const std::string& n, const std::string& key, uint32_t v);
  void setI32(const std::string& n, const std::string& key, int32_t v);
  void setI64(const std::string& n, const std::string& key, int64_t v);
  void setStr(const std::string& n, const std::string& key, const std::string& v);
  void setBlob(const std::string& n, const std::string& key, const std::vector<uint8_t>& v);
  bool has(const std::string& n, const std::string& key) const;
  const NvsEntry* get(const std::string& n, const std::string& key) const;
  uint64_t getU(const std::string& n, const std::string& key) const;  // 0 when missing
  int64_t getI(const std::string& n, const std::string& key) const;
  std::vector<uint8_t> getBlob(const std::string& n, const std::string& key) const;
};
Nvs& nvs();

// ---------------------------------------------------------------- OTA

struct UpdateState {
  // scripted
  bool beginResult = true;
  uint8_t beginError = UPDATE_ERROR_SPACE;  // getError() after a refused begin
  size_t shortWriteAt = SIZE_MAX;   // total accepted bytes before a write comes up short
  bool endResult = true;
  uint8_t endError = UPDATE_ERROR_MD5;  // getError() after a refused end
  bool setMd5Result = true;
  std::string errorText;            // errorString() override ("" = the library's text)
  // recorded
  bool running = false;
  uint8_t error = 0;
  size_t beginSize = 0;
  int beginCommand = -1;
  std::string md5;
  std::vector<uint8_t> written;
  int begins = 0;
  int writes = 0;
  int ends = 0;
  int aborts = 0;
  bool endEvenIfRemaining = false;
};
struct Ota {
  // persistent: partition states and the boot selection
  esp_partition_t parts[2];
  esp_ota_img_states_t state[2] = {ESP_OTA_IMG_VALID, ESP_OTA_IMG_UNDEFINED};
  int running = 0;
  int boot = 0;
  // volatile knobs and records
  bool hasRunning = true;
  bool hasNext = true;
  esp_err_t stateResult = ESP_OK;
  esp_err_t markValidResult = ESP_OK;
  int markValid = 0;
  int markInvalid = 0;
  UpdateState update;
  Ota();
  // What the bootloader does at a reset: a NEW image selected for boot runs as PENDING_VERIFY,
  // a boot of an image still PENDING_VERIFY is ABORTED and the other image runs.
  void bootloader();
};
Ota& ota();

// ---------------------------------------------------------------- network

struct EthBegin {
  uint8_t phyAddr;
  int power;
  int mdc;
  int mdio;
  eth_phy_type_t type;
  eth_clock_mode_t clk;
};
struct IpConfig {
  uint32_t ip, gateway, mask, dns1, dns2;
};
struct UdpPacket {
  uint32_t ip;
  uint16_t port;
  std::string data;
};
struct PingSession {
  esp_ping_config_t config;
  esp_ping_callbacks_t cbs;
  bool started = false;
  bool stopped = false;
  bool deleted = false;
  uint32_t seqno = 0;
  uint32_t requests = 0;
  uint32_t replies = 0;
  bool ended = false;
};
struct TcpConnect {
  uint32_t ip;
  std::string host;
  uint16_t port;
  int32_t timeoutMs;
};
struct Net {
  // ETH
  bool ethBeginResult = true;
  std::vector<EthBegin> ethBegins;
  std::vector<IpConfig> ethConfigs;
  std::string ethHostname;
  bool ethLink = false;
  uint32_t ethIp = 0, ethMask = 0, ethGateway = 0, ethDns = 0;
  std::string ethMac = "A8:03:2A:A1:B2:C3";
  esp_eth_handle_t ethHandle = reinterpret_cast<esp_eth_handle_t>(0xE7);
  esp_err_t ethStopResult = ESP_OK;
  esp_err_t ethStartResult = ESP_OK;
  int ethStops = 0;
  int ethStarts = 0;
  // WiFi
  bool wifiPersistent = true;
  wifi_mode_t wifiMode = WIFI_OFF;
  std::vector<wifi_mode_t> wifiModes;
  std::string wifiHostname;
  bool autoReconnect = false;
  int wifiBegins = 0;
  std::string ssid;
  std::string pass;
  std::vector<IpConfig> wifiConfigs;
  int wifiDisconnects = 0;
  bool lastDisconnectWifiOff = false;
  int wifiReconnects = 0;
  wl_status_t wifiStatus = WL_DISCONNECTED;
  uint32_t wifiIp = 0, wifiMask = 0, wifiGateway = 0, wifiDns = 0;
  int8_t rssi = -60;
  std::string wifiMac = "24:0A:C4:12:34:56";
  // events (onEvent)
  struct Handler {
    WiFiEventFuncCb cb;
    arduino_event_id_t event;
    bool removed;
  };
  std::vector<Handler> handlers;
  // Delivers a system event to the registered callbacks like the event task (ETH_CONNECTED
  // carries ethHandle unless `info` names another handle).
  void fire(arduino_event_id_t event, arduino_event_info_t info = arduino_event_info_t{});
  // SNTP and TZ
  bool sntpEnabled = false;
  int sntpStops = 0;
  std::vector<std::pair<std::string, std::string>> tzConfigs;  // configTzTime(tz, server1)
  sntp_sync_time_cb_t sntpCb = nullptr;
  // Sets the wall clock and calls the sync callback like lwIP after an answer.
  void syncTime(int64_t epoch);
  // UDP
  int udpBeginResult = 1;
  int udpEndResult = 1;
  std::vector<UdpPacket> udpSent;
  // mDNS
  bool mdnsResult = true;
  std::vector<std::string> mdnsBegins;
  std::vector<std::string> mdnsServices;  // "http tcp 80"
  // ping (esp_ping)
  std::vector<std::unique_ptr<PingSession>> pings;
  esp_err_t pingNewResult = ESP_OK;
  std::deque<bool> pingAnswers;  // scripted: true = reply, false = timeout
  bool pingDefault = true;       // when pingAnswers is empty
  uint32_t pingTimeMs = 3;       // reply time (ESP_PING_PROF_TIMEGAP)
  // One answer to every started session; on_ping_end after `count` answers.
  void pingStep();
  // TCP clients (WiFiClient::connect other than the MQTT broker)
  int tcpConnectResult = 1;
  std::vector<TcpConnect> tcpConnects;
  // Answer of a connection to the request bytes written to it ("" = connection closed).
  std::function<std::string(const TcpConnect& to, const std::string& request)> tcpResponder;
  // Reply parts that arrive later: (fake ms, bytes), appended to the answer of the open connection
  // once the clock reached the time; the server keeps the connection open until the last one.
  std::deque<std::pair<uint64_t, std::string>> tcpLater;
};
Net& net();

// ---------------------------------------------------------------- MQTT broker (PubSubClient)

struct MqttMessage {
  std::string topic;
  std::string payload;
  bool retained;
};
struct Mqtt {
  // scripted
  bool connectResult = true;
  int failState = MQTT_CONNECT_FAILED;  // state() after a refused connect
  long failPublishAt = -1;   // index (0-based, over all publish calls) of a publish that fails
  std::string failPublishTopic;   // the next publish to this topic fails ("" = none)
  bool failSubscribe = false;
  std::deque<MqttMessage> inbox;  // delivered by loop(), one per call
  bool burst = false;             // loop() delivers the whole inbox at once
  // recorded
  std::string host;
  uint16_t port = 0;
  uint16_t keepAlive = 0;
  uint16_t socketTimeout = 0;
  uint16_t bufferSize = 0;
  int bufferSizeCalls = 0;
  std::string clientId;
  std::string user;
  std::string pass;
  bool userNull = true;
  bool passNull = true;
  std::string willTopic;
  uint8_t willQos = 0;
  bool willRetain = false;
  std::string willMessage;
  bool cleanSession = true;
  bool hasCallback = false;
  int connects = 0;       // connect() calls
  int disconnects = 0;    // disconnect() calls
  int loops = 0;
  int netStops = 0;       // WiFiClient::stop() of the broker socket
  long publishCalls = 0;
  std::vector<MqttMessage> published;
  std::vector<std::pair<std::string, uint8_t>> subscribed;
  std::vector<std::string> unsubscribed;
  bool connected = false;
  int state = MQTT_DISCONNECTED;
  bool inCallback = false;
  // The broker drops the session (state MQTT_CONNECTION_LOST).
  void dropConnection();
  std::vector<MqttMessage> publishedTo(const std::string& topic) const;
};
Mqtt& mqtt();

}  // namespace fakes
