// Fake khoih-prog/AsyncWebServer_WT32_ETH01 1.6.2 (with the VDM patch of tools/patch_libs.py): the
// classes and members the web glue uses, with the library's signatures (the glue's `override`
// keywords turn signature drift into compile errors). Requests are driven by fakes/http.h, which
// reproduces the library's order of calls (WebServer.cpp _attachHandler, WebRequest.cpp _onData,
// _removeNotInterestingHeaders, onDisconnect).
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <functional>
#include <memory>
#include <string>
#include <vector>

#include "Arduino.h"
#include "AsyncTCP.h"
#include "FS.h"
#include "WString.h"

typedef enum {
  HTTP_GET = 0b00000001,
  HTTP_POST = 0b00000010,
  HTTP_DELETE = 0b00000100,
  HTTP_PUT = 0b00001000,
  HTTP_PATCH = 0b00010000,
  HTTP_HEAD = 0b00100000,
  HTTP_OPTIONS = 0b01000000,
  HTTP_ANY = 0b01111111,
} WebRequestMethod;

// if this value is returned when asked for data, packet will not be sent and you will be asked
// for data again
#define RESPONSE_TRY_AGAIN 0xFFFFFFFF

typedef uint8_t WebRequestMethodComposite;
typedef std::function<void(void)> ArDisconnectHandler;

class AsyncWebServer;
class AsyncWebServerRequest;
class AsyncWebServerResponse;
class AsyncWebHandler;

namespace fakes {
namespace http {
class Exchange;
}  // namespace http
}  // namespace fakes

class AsyncWebParameter {
 public:
  AsyncWebParameter(const String& name, const String& value, bool form = false, bool file = false,
                    size_t size = 0)
      : name_(name), value_(value), size_(size), isForm_(form), isFile_(file) {}
  const String& name() const { return name_; }
  const String& value() const { return value_; }
  size_t size() const { return size_; }
  bool isPost() const { return isForm_; }
  bool isFile() const { return isFile_; }

 private:
  String name_;
  String value_;
  size_t size_;
  bool isForm_;
  bool isFile_;
};

class AsyncWebHeader {
 public:
  AsyncWebHeader(const String& name, const String& value) : name_(name), value_(value) {}
  const String& name() const { return name_; }
  const String& value() const { return value_; }
  String toString() const { return name_ + ": " + value_ + "\r\n"; }

 private:
  String name_;
  String value_;
};

typedef enum {
  RCT_NOT_USED = -1,
  RCT_DEFAULT = 0,
  RCT_HTTP,
  RCT_WS,
  RCT_EVENT,
  RCT_MAX
} RequestedConnectionType;

typedef std::function<size_t(uint8_t*, size_t, size_t)> AwsResponseFiller;
typedef std::function<String(const String&)> AwsTemplateProcessor;

class AsyncWebServerResponse {
 public:
  AsyncWebServerResponse() = default;
  virtual ~AsyncWebServerResponse() = default;
  virtual void setCode(int code) { code_ = code; }
  virtual void setContentLength(size_t len) { contentLength_ = len; }
  virtual void setContentType(const String& type) { contentType_ = type; }
  virtual void addHeader(const String& name, const String& value) {
    headers_.emplace_back(name.str(), value.str());
  }

 private:
  friend class AsyncWebServerRequest;
  friend class fakes::http::Exchange;
  enum class Kind : uint8_t { Text, Bytes, Chunked, Filler };
  int code_ = 200;
  String contentType_;
  size_t contentLength_ = 0;
  std::vector<std::pair<std::string, std::string>> headers_;
  Kind kind_ = Kind::Text;
  std::string text_;                  // Text
  const uint8_t* data_ = nullptr;     // Bytes: read when the response is transmitted
  size_t length_ = 0;
  AwsResponseFiller filler_;          // Chunked / Filler: called when transmitted
};

class AsyncWebServerRequest {
  using File = fs::File;
  using FS = fs::FS;

 public:
  AsyncWebServerRequest(AsyncWebServer* server, AsyncClient* client);
  ~AsyncWebServerRequest();
  AsyncWebServerRequest(const AsyncWebServerRequest&) = delete;
  AsyncWebServerRequest& operator=(const AsyncWebServerRequest&) = delete;

  AsyncClient* client() { return client_; }
  uint8_t version() const { return 1; }
  WebRequestMethodComposite method() const { return method_; }
  const String& url() const { return url_; }
  const String& host() const { return host_; }
  const String& contentType() const { return contentType_; }
  size_t contentLength() const { return contentLength_; }
  bool multipart() const { return isMultipart_; }
  const char* methodToString() const;
  RequestedConnectionType requestedConnType() const { return RCT_HTTP; }

  void onDisconnect(ArDisconnectHandler fn);
  void setHandler(AsyncWebHandler* handler) { handler_ = handler; }
  void addInterestingHeader(const String& name);
  void redirect(const String& url);

  void send(AsyncWebServerResponse* response);
  void send(int code, const String& contentType = String(), const String& content = String());
  void send(int code, const String& contentType, const char* content,
            bool nonDetructiveSend = true);
  void send(const String& contentType, size_t len, AwsResponseFiller callback,
            AwsTemplateProcessor templateCallback = nullptr);
  void sendChunked(const String& contentType, AwsResponseFiller callback,
                   AwsTemplateProcessor templateCallback = nullptr);
  void send_P(int code, const String& contentType, const uint8_t* content, size_t len,
              AwsTemplateProcessor callback = nullptr);
  void send_P(int code, const String& contentType, PGM_P content,
              AwsTemplateProcessor callback = nullptr);

  AsyncWebServerResponse* beginResponse(int code, const String& contentType = String(),
                                        const String& content = String());
  AsyncWebServerResponse* beginResponse(int code, const String& contentType,
                                        const char* content = nullptr);
  AsyncWebServerResponse* beginResponse(int code, const String& contentType,
                                        const uint8_t* content, size_t len,
                                        AwsTemplateProcessor callback = nullptr);
  AsyncWebServerResponse* beginResponse(const String& contentType, size_t len,
                                        AwsResponseFiller callback,
                                        AwsTemplateProcessor templateCallback = nullptr);
  AsyncWebServerResponse* beginChunkedResponse(const String& contentType,
                                               AwsResponseFiller callback,
                                               AwsTemplateProcessor templateCallback = nullptr);
  AsyncWebServerResponse* beginResponse_P(int code, const String& contentType,
                                          const uint8_t* content, size_t len,
                                          AwsTemplateProcessor callback = nullptr);
  AsyncWebServerResponse* beginResponse_P(int code, const String& contentType, PGM_P content,
                                          AwsTemplateProcessor callback = nullptr);

  size_t headers() const;
  bool hasHeader(const String& name) const;
  AsyncWebHeader* getHeader(const String& name) const;
  AsyncWebHeader* getHeader(size_t num) const;
  size_t params() const;
  bool hasParam(const String& name, bool post = false, bool file = false) const;
  AsyncWebParameter* getParam(const String& name, bool post = false, bool file = false) const;
  AsyncWebParameter* getParam(size_t num) const;
  size_t args() const { return params(); }
  const String& arg(const String& name) const;
  const String& arg(size_t i) const;
  const String& argName(size_t i) const;
  bool hasArg(const char* name) const;
  const String& header(const char* name) const;
  const String& header(size_t i) const;
  const String& headerName(size_t i) const;
  String urlDecode(const String& text) const;

 private:
  friend class fakes::http::Exchange;
  AsyncWebServerResponse* newResponse(int code, const String& contentType);
  void record(AsyncWebServerResponse* response);

  AsyncWebServer* server_;
  AsyncClient* client_;
  AsyncWebHandler* handler_ = nullptr;
  WebRequestMethodComposite method_ = HTTP_GET;
  String url_;
  String host_;
  String contentType_;
  size_t contentLength_ = 0;
  bool isMultipart_ = false;
  std::vector<std::unique_ptr<AsyncWebHeader>> headers_;
  std::vector<std::unique_ptr<AsyncWebParameter>> params_;
  std::vector<String> interesting_;
  ArDisconnectHandler onDisconnect_;
  std::vector<std::unique_ptr<AsyncWebServerResponse>> responses_;
  fakes::http::Exchange* exchange_ = nullptr;
};

typedef std::function<bool(AsyncWebServerRequest* request)> ArRequestFilterFunction;

class AsyncWebHandler {
 protected:
  ArRequestFilterFunction _filter;
  String _username;
  String _password;

 public:
  AsyncWebHandler() : _username(""), _password("") {}
  AsyncWebHandler& setFilter(ArRequestFilterFunction fn) {
    _filter = fn;
    return *this;
  }
  AsyncWebHandler& setAuthentication(const char* username, const char* password) {
    _username = String(username);
    _password = String(password);
    return *this;
  }
  bool filter(AsyncWebServerRequest* request) { return _filter == nullptr || _filter(request); }
  virtual ~AsyncWebHandler() {}
  virtual bool canHandle(AsyncWebServerRequest* request __attribute__((unused))) { return false; }
  virtual void handleRequest(AsyncWebServerRequest* request __attribute__((unused))) {}
  virtual void handleUpload(AsyncWebServerRequest* request __attribute__((unused)),
                            const String& filename __attribute__((unused)),
                            size_t index __attribute__((unused)),
                            uint8_t* data __attribute__((unused)),
                            size_t len __attribute__((unused)),
                            bool final __attribute__((unused))) {}
  virtual void handleBody(AsyncWebServerRequest* request __attribute__((unused)),
                          uint8_t* data __attribute__((unused)),
                          size_t len __attribute__((unused)),
                          size_t index __attribute__((unused)),
                          size_t total __attribute__((unused))) {}
  virtual bool isRequestHandlerTrivial() { return true; }
};

typedef std::function<void(AsyncWebServerRequest* request)> ArRequestHandlerFunction;
typedef std::function<void(AsyncWebServerRequest* request, const String& filename, size_t index,
                           uint8_t* data, size_t len, bool final)>
    ArUploadHandlerFunction;
typedef std::function<void(AsyncWebServerRequest* request, uint8_t* data, size_t len, size_t index,
                           size_t total)>
    ArBodyHandlerFunction;

// Registers itself as fakes::http::server().instance; the request driver asks its handlers.
class AsyncWebServer {
 public:
  explicit AsyncWebServer(uint16_t port);
  ~AsyncWebServer();
  void begin();
  void end();
  AsyncWebHandler& addHandler(AsyncWebHandler* handler);
  bool removeHandler(AsyncWebHandler* handler);
  void onNotFound(ArRequestHandlerFunction fn);
  void reset();

  uint16_t port() const { return port_; }
  const std::vector<AsyncWebHandler*>& handlers() const { return handlers_; }

 private:
  uint16_t port_;
  std::vector<AsyncWebHandler*> handlers_;
  ArRequestHandlerFunction notFound_;
  friend class fakes::http::Exchange;
};
