// Request driver of the ESP glue tests: drives the handlers of the AsyncWebServer fake the way
// AsyncWebServer_WT32_ETH01 1.6.2 (with the VDM patch) drives them:
//   1. after the headers the first handler with filter() && canHandle() owns the request
//      (WebServer.cpp _attachHandler), then every header not registered with
//      addInterestingHeader() (or "ANY") is dropped (WebRequest.cpp _removeNotInterestingHeaders);
//   2. Content-Length 0: handleRequest() at once; otherwise the body arrives in TCP segments
//      (Request::segment, default 1460) and handleRequest() runs when all of it arrived:
//      - application/x-www-form-urlencoded, or text/plain starting with "name=": a plain post,
//        the fields become POST params and handleBody() is not called;
//      - multipart: only for a non-trivial handler: fields become POST params, file data goes to
//        handleUpload() in pieces of at most 1460 bytes (also cut at segment ends) and one
//        final call with the rest (empty when nothing is left); a '-' right after the closing
//        "\r\n--<boundary>" sets Content-Length to its offset + 4 (_parseMultipartPostByte,
//        DASH3_OR_RETURN2), so a body that ends without the CRLF after "--<boundary>--" or has
//        an epilogue in the same segment is never handled (not for a body padded with zeros);
//      - anything else: handleBody(data, len, index, total) per segment, also for a trivial
//        handler;
//      the segment with the last body bytes also carries Request::pipelined, which the library
//      passes on with them (_onData does not stop at Content-Length): the request is then never
//      handled, its parsed length ran past Content-Length;
//   3. onDisconnect() keeps one callback, the last one set; it runs when the client is gone;
//   4. beginResponse_P() data and chunked fillers are read when the response is transmitted
//      (Exchange::finish()), not at send(): a buffer released too early shows as a wrong body;
//   5. server().failNextResponse makes the next beginResponse*() return nullptr (out of memory);
//   6. server().recycleRequests builds the next request in the memory of the last one deleted,
//      like the heap handing a freed block back: state keyed by request pointers meets a
//      recycled address.
// Invariant checked by the runner hooks: every request that reached handleRequest() was answered
// exactly once (Response::sends == 1) by the time it ended.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "AsyncWebServer_WT32_ETH01.h"

namespace fakes {
namespace http {

struct Part {
  std::string name;
  std::string filename;  // "" = a form field
  std::string data;
  std::string contentType = "application/octet-stream";
};

struct Request {
  WebRequestMethodComposite method = HTTP_GET;
  std::string url = "/";           // path; a "?query" in it becomes the GET params
  std::string host = "vdmot.local";
  std::vector<std::pair<std::string, std::string>> headers;
  std::string contentType;
  std::string body;                // raw body (ignored when parts are given)
  std::vector<Part> parts;         // multipart/form-data body
  std::string boundary = "----vdmotBoundary7MA4YWxk";
  size_t contentLength = SIZE_MAX;  // SIZE_MAX = the length of the body
  size_t segment = 1460;           // TCP segment size of the body
  uint32_t remoteIp = 0x3201A8C0;  // 192.168.1.50
  uint32_t localIp = 0;            // the connection's local address (0 = unknown)
  bool zeroLengthFinal = false;    // uploads: the rest in a non-final call, then an empty final
  std::string pipelined;           // bytes after the body in the segment with its last bytes

  Request& header(const std::string& name, const std::string& value) {
    headers.emplace_back(name, value);
    return *this;
  }
  // The body bytes as they go over the wire (multipart framing included).
  std::string wireBody() const;
};

Request get(const std::string& url);
Request del(const std::string& url);
Request post(const std::string& url, const std::string& body,
             const std::string& contentType = "application/json");
// POST multipart/form-data with the fields first, then the file part "file".
Request upload(const std::string& url, const std::string& filename, const std::string& data,
               const std::vector<std::pair<std::string, std::string>>& fields = {});

struct Response {
  int sends = 0;  // answers given; the invariant wants exactly 1
  int code = 0;
  std::string contentType;
  std::vector<std::pair<std::string, std::string>> headers;
  std::string body;  // transmitted content
  bool chunked = false;
  bool transmitted = false;
  std::string header(const std::string& name) const;  // "" when absent
};

// One request from connect to disconnect, for interleaving several of them.
class Exchange {
 public:
  // Connects, sends the headers (handler attached, headers filtered; a request without a body
  // is handled at once).
  explicit Exchange(const Request& r);
  ~Exchange();
  Exchange(const Exchange&) = delete;
  Exchange& operator=(const Exchange&) = delete;

  // The next `n` body bytes (in segments); true when the body is complete.
  bool sendBody(size_t n);
  // The rest of the body, then the response is transmitted and the client disconnects.
  const Response& finish();
  // The client goes away now (body or response incomplete).
  void disconnect();

  const Response& response() const { return response_; }
  AsyncWebServerRequest* request() { return req_; }
  bool handled() const { return handled_; }

  // library side (called by the AsyncWebServerRequest fake)
  void onSend(AsyncWebServerResponse* response);

 private:
  void deliver(const uint8_t* data, size_t len);
  void deliverMultipartByte(size_t pos, uint8_t b, bool last);
  void closeDelimiter(size_t pos, uint8_t b);
  void flushUpload(bool final);
  void addPostParams(const std::string& form);
  void transmit();
  void end();

  Request r_;
  std::string wire_;
  size_t sent_ = 0;
  AsyncClient client_;
  AsyncWebServerRequest* req_ = nullptr;
  AsyncWebServerResponse* pending_ = nullptr;
  Response response_;
  bool handled_ = false;
  bool ended_ = false;
  bool plainPost_ = false;
  bool plainPostChecked_ = false;
  // body parser state (multipart ranges, plain post fields)
  struct PartState;
  std::unique_ptr<PartState> mp_;
};

// One request: the whole body, the response transmitted, the client disconnected.
Response perform(const Request& r);

struct Server {
  AsyncWebServer* instance = nullptr;  // the last constructed AsyncWebServer
  uint16_t port = 0;
  int begins = 0;
  bool failNextResponse = false;
  int exchanges = 0;                   // requests handled so far
  std::vector<std::string> violations; // requests not answered exactly once
  int openExchanges = 0;               // Exchange objects not ended yet
  bool recycleRequests = false;        // a new request takes the memory of the last one deleted
  void* spareRequest = nullptr;        // that memory (recycleRequests)
};
Server& server();

}  // namespace http
}  // namespace fakes
