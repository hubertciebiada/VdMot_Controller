// Fake AsyncWebServer_WT32_ETH01 and the request driver (fakes/http.h).
#include <ctype.h>
#include <stdlib.h>
#include <string.h>

#include <memory>
#include <string>
#include <vector>

#include "AsyncWebServer_WT32_ETH01.h"
#include "fakes/fakes.h"
#include "fakes/http.h"
#include "hal_internal.h"

namespace fakes {
namespace http {

namespace {

// Constructed on first use: the firmware's AsyncWebServer is a static object that registers
// itself during static initialisation, possibly before this file's globals are initialised.
Server& state() {
  static Server s;
  return s;
}

bool equalsIgnoreCase(const std::string& a, const std::string& b) {
  if (a.size() != b.size()) return false;
  for (size_t i = 0; i < a.size(); ++i) {
    if (tolower(static_cast<unsigned char>(a[i])) != tolower(static_cast<unsigned char>(b[i]))) {
      return false;
    }
  }
  return true;
}

// WebRequest.cpp __is_param_char
bool isParamChar(char c) { return c != 0 && c != '{' && c != '[' && c != '&' && c != '='; }

std::string decode(const std::string& text) {
  // AsyncWebServerRequest::urlDecode(), including its "%" handling at the end of the text
  std::string out;
  size_t i = 0;
  const size_t len = text.size();
  while (i < len) {
    const char c = text[i++];
    if (c == '%' && i + 1 < len) {
      char hex[3] = {text[i], text[i + 1], 0};
      i += 2;
      out.push_back(static_cast<char>(strtol(hex, nullptr, 16)));
    } else {
      out.push_back(c == '+' ? ' ' : c);
    }
  }
  return out;
}

}  // namespace

Server& server() { return state(); }

std::string Request::wireBody() const {
  if (parts.empty()) return body;
  std::string out;
  for (const Part& p : parts) {
    out += "--" + boundary + "\r\n";
    out += "Content-Disposition: form-data; name=\"" + p.name + "\"";
    if (!p.filename.empty()) out += "; filename=\"" + p.filename + "\"";
    out += "\r\n";
    if (!p.filename.empty()) out += "Content-Type: " + p.contentType + "\r\n";
    out += "\r\n" + p.data + "\r\n";
  }
  out += "--" + boundary + "--\r\n";
  return out;
}

Request get(const std::string& url) {
  Request r;
  r.method = HTTP_GET;
  r.url = url;
  return r;
}

Request del(const std::string& url) {
  Request r;
  r.method = HTTP_DELETE;
  r.url = url;
  return r;
}

Request post(const std::string& url, const std::string& body, const std::string& contentType) {
  Request r;
  r.method = HTTP_POST;
  r.url = url;
  r.body = body;
  r.contentType = contentType;
  return r;
}

Request upload(const std::string& url, const std::string& filename, const std::string& data,
               const std::vector<std::pair<std::string, std::string>>& fields) {
  Request r;
  r.method = HTTP_POST;
  r.url = url;
  for (const auto& f : fields) r.parts.push_back({f.first, "", f.second, ""});
  r.parts.push_back({"file", filename, data, "application/octet-stream"});
  r.contentType = "multipart/form-data; boundary=" + r.boundary;
  return r;
}

std::string Response::header(const std::string& name) const {
  for (const auto& h : headers) {
    if (equalsIgnoreCase(h.first, name)) return h.second;
  }
  return "";
}

// Byte ranges of the multipart body.
struct Exchange::PartState {
  struct Range {
    size_t dataStart;
    size_t dataEnd;
    size_t endAt;  // first byte after the delimiter that closes the part
    bool file;
    std::string name;
    std::string filename;
  };
  std::vector<Range> ranges;
  size_t current = 0;
  std::vector<uint8_t> buffer;  // _itemBuffer
  size_t itemSize = 0;          // file bytes so far
  std::string value;            // form field value
  std::string plain;            // plain post: fields not complete yet
};

Exchange::Exchange(const Request& r) : r_(r), client_(r.remoteIp) {
  wire_ = r_.wireBody();
  ++state().openExchanges;
  AsyncWebServer* srv = state().instance;
  req_ = new AsyncWebServerRequest(srv, &client_);
  req_->exchange_ = this;
  req_->method_ = r_.method;
  const size_t q = r_.url.find('?');
  req_->url_ = String(decode(r_.url.substr(0, q)));
  if (q != std::string::npos) {
    const std::string params = r_.url.substr(q + 1);
    size_t start = 0;
    while (start < params.size()) {
      size_t end = params.find('&', start);
      if (end == std::string::npos) end = params.size();
      size_t equal = params.find('=', start);
      if (equal == std::string::npos || equal > end) equal = end;
      const std::string name = params.substr(start, equal - start);
      const std::string value = equal + 1 < end ? params.substr(equal + 1, end - equal - 1) : "";
      req_->params_.emplace_back(
          new AsyncWebParameter(String(decode(name)), String(decode(value))));
      start = end + 1;
    }
  }
  // headers as the client sends them
  req_->headers_.emplace_back(new AsyncWebHeader("Host", String(r_.host)));
  req_->host_ = String(r_.host);
  size_t length = r_.contentLength != SIZE_MAX ? r_.contentLength : wire_.size();
  if (!r_.contentType.empty()) {
    req_->headers_.emplace_back(new AsyncWebHeader("Content-Type", String(r_.contentType)));
    const size_t semi = r_.contentType.find(';');
    req_->contentType_ = String(r_.contentType.substr(0, semi));
    if (r_.contentType.compare(0, 10, "multipart/") == 0) req_->isMultipart_ = true;
  }
  if (length > 0) {
    req_->headers_.emplace_back(
        new AsyncWebHeader("Content-Length", String(std::to_string(length))));
  }
  for (const auto& h : r_.headers) {
    req_->headers_.emplace_back(new AsyncWebHeader(String(h.first), String(h.second)));
  }
  req_->contentLength_ = length;
  if (wire_.size() > length) wire_.resize(length);

  // WebServer.cpp _attachHandler
  AsyncWebHandler* handler = nullptr;
  if (srv != nullptr) {
    for (AsyncWebHandler* h : srv->handlers_) {
      if (h->filter(req_) && h->canHandle(req_)) {
        handler = h;
        break;
      }
    }
  }
  if (handler == nullptr) req_->addInterestingHeader("ANY");
  req_->setHandler(handler);
  // WebRequest.cpp _removeNotInterestingHeaders
  bool any = false;
  for (const String& i : req_->interesting_) any = any || i.equalsIgnoreCase("ANY");
  if (!any) {
    auto& hs = req_->headers_;
    for (auto it = hs.begin(); it != hs.end();) {
      bool keep = false;
      for (const String& i : req_->interesting_) keep = keep || i.equalsIgnoreCase((*it)->name());
      it = keep ? it + 1 : hs.erase(it);
    }
  }
  mp_.reset(new PartState());
  if (req_->isMultipart_) {
    size_t pos = 0;
    const std::string delimiter = "\r\n--" + r_.boundary;
    for (const Part& p : r_.parts) {
      pos = wire_.find("\r\n\r\n", pos);
      if (pos == std::string::npos) break;
      const size_t dataStart = pos + 4;
      const size_t dataEnd = dataStart + p.data.size();
      mp_->ranges.push_back({dataStart, dataEnd, dataEnd + delimiter.size(), !p.filename.empty(),
                             p.name, p.filename});
      pos = dataEnd;
    }
  }
  if (length == 0) {
    handled_ = true;
    ++state().exchanges;
    if (handler != nullptr) {
      handler->handleRequest(req_);
    } else {
      req_->send(500);
    }
  }
}

Exchange::~Exchange() {
  if (!ended_) end();
}

bool Exchange::sendBody(size_t n) {
  if (ended_) return true;  // the client is gone, the request deleted
  const size_t total = req_->contentLength_;
  while (n > 0 && sent_ < total && !ended_) {
    size_t len = r_.segment < n ? r_.segment : n;
    if (len > total - sent_) len = total - sent_;
    // bytes missing from the wire (Content-Length larger than the body) arrive as zeros
    std::string seg = sent_ < wire_.size() ? wire_.substr(sent_, len) : std::string();
    seg.resize(len, '\0');
    deliver(reinterpret_cast<const uint8_t*>(seg.data()), len);
    n -= len;
  }
  return sent_ >= total;
}

void Exchange::deliver(const uint8_t* data, size_t len) {
  AsyncWebHandler* h = req_->handler_;
  const bool needParse = h != nullptr && !h->isRequestHandlerTrivial();
  const size_t start = sent_;
  if (req_->isMultipart_) {
    if (needParse) {
      for (size_t i = 0; i < len; ++i) deliverMultipartByte(start + i, data[i], i == len - 1);
    }
    sent_ += len;
  } else {
    if (start == 0 && !plainPostChecked_) {
      plainPostChecked_ = true;
      const std::string ct = req_->contentType_.str();
      if (ct.compare(0, 33, "application/x-www-form-urlencoded") == 0) {
        plainPost_ = true;
      } else if (ct == "text/plain" && isParamChar(static_cast<char>(data[0]))) {
        size_t i = 0;
        while (i < len && isParamChar(static_cast<char>(data[i++]))) {
        }
        if (i < len && data[i - 1] == '=') plainPost_ = true;
      }
    }
    if (!plainPost_) {
      if (h != nullptr) {
        // an exact-size copy: a read past `len` is an ASan error
        std::unique_ptr<uint8_t[]> copy(new uint8_t[len]);
        memcpy(copy.get(), data, len);
        h->handleBody(req_, copy.get(), len, sent_, req_->contentLength_);
      }
    } else if (needParse) {
      addPostParams(std::string(reinterpret_cast<const char*>(data), len));
    }
    sent_ += len;
  }
  if (sent_ == req_->contentLength_ && !handled_) {
    if (plainPost_ && needParse) addPostParams(std::string());
    handled_ = true;
    ++state().exchanges;
    if (h != nullptr) {
      h->handleRequest(req_);
    } else {
      req_->send(501);
    }
  }
}

void Exchange::addPostParams(const std::string& form) {
  // WebRequest.cpp _parsePlainPostChar: a field ends at '&' or with the body ("" = the end)
  std::string& pending = mp_->plain;
  pending += form;
  const bool atEnd = form.empty();
  size_t start = 0;
  for (;;) {
    const size_t amp = pending.find('&', start);
    if (amp == std::string::npos && !atEnd) break;
    const std::string item =
        pending.substr(start, amp == std::string::npos ? std::string::npos : amp - start);
    std::string name = "body";
    std::string value = item;
    const size_t eq = item.find('=');
    if (item.compare(0, 1, "{") != 0 && item.compare(0, 1, "[") != 0 && eq != std::string::npos &&
        eq > 0) {
      name = item.substr(0, eq);
      value = item.substr(eq + 1);
    }
    req_->params_.emplace_back(
        new AsyncWebParameter(String(decode(name)), String(decode(value)), true));
    if (amp == std::string::npos) {
      start = pending.size();
      break;
    }
    start = amp + 1;
  }
  pending = pending.substr(start);
}

void Exchange::deliverMultipartByte(size_t pos, uint8_t b, bool last) {
  PartState& s = *mp_;
  if (s.current >= s.ranges.size()) return;
  PartState::Range& r = s.ranges[s.current];
  if (pos < r.dataStart) return;
  if (pos < r.dataEnd) {
    if (r.file) {
      s.buffer.push_back(b);
      ++s.itemSize;
      if (last || s.buffer.size() == 1460) flushUpload(false);
    } else {
      s.value.push_back(static_cast<char>(b));
    }
    return;
  }
  if (pos + 1 < r.endAt) return;
  // the delimiter after the part is complete
  if (r.file) {
    if (r_.zeroLengthFinal && !s.buffer.empty()) flushUpload(false);
    flushUpload(true);
  } else {
    req_->params_.emplace_back(new AsyncWebParameter(String(r.name), String(s.value), true));
  }
  s.buffer.clear();
  s.itemSize = 0;
  s.value.clear();
  ++s.current;
}

void Exchange::flushUpload(bool final) {
  PartState& s = *mp_;
  const PartState::Range& r = s.ranges[s.current];
  const size_t n = s.buffer.size();
  std::unique_ptr<uint8_t[]> copy(new uint8_t[n > 0 ? n : 1]);
  if (n > 0) memcpy(copy.get(), s.buffer.data(), n);
  s.buffer.clear();
  AsyncWebHandler* h = req_->handler_;
  if (h != nullptr) h->handleUpload(req_, String(r.filename), s.itemSize - n, copy.get(), n, final);
}

const Response& Exchange::finish() {
  if (!ended_) {
    sendBody(SIZE_MAX);
    transmit();
    end();
  }
  return response_;
}

void Exchange::disconnect() {
  if (!ended_) end();
}

void Exchange::onSend(AsyncWebServerResponse* response) {
  ++response_.sends;
  pending_ = response;
  if (response == nullptr) {
    response_.code = 0;  // the library closes the connection
    return;
  }
  response_.code = response->code_;
  response_.contentType = response->contentType_.str();
  response_.headers = response->headers_;
  response_.chunked = response->kind_ == AsyncWebServerResponse::Kind::Chunked;
}

void Exchange::transmit() {
  if (pending_ == nullptr || response_.transmitted) return;
  response_.transmitted = true;
  AsyncWebServerResponse* res = pending_;
  switch (res->kind_) {
    case AsyncWebServerResponse::Kind::Text:
      response_.body = res->text_;
      break;
    case AsyncWebServerResponse::Kind::Bytes:
      response_.body.assign(reinterpret_cast<const char*>(res->data_), res->length_);
      break;
    case AsyncWebServerResponse::Kind::Chunked:
    case AsyncWebServerResponse::Kind::Filler: {
      const bool limited = res->kind_ == AsyncWebServerResponse::Kind::Filler;
      uint8_t buf[1024];
      for (int guard = 0; guard < 100000; ++guard) {
        size_t space = sizeof buf;
        if (limited && res->length_ - response_.body.size() < space) {
          space = res->length_ - response_.body.size();
        }
        if (space == 0) break;
        const size_t n = res->filler_(buf, space, response_.body.size());
        if (n == RESPONSE_TRY_AGAIN) continue;
        if (n == 0) break;
        response_.body.append(reinterpret_cast<const char*>(buf), n > space ? space : n);
      }
      break;
    }
  }
}

void Exchange::end() {
  ended_ = true;
  --state().openExchanges;
  if (handled_ && response_.sends != 1) {
    state().violations.push_back(std::string(req_->methodToString()) + " " + r_.url +
                                  " answered " + std::to_string(response_.sends) + " times");
  }
  // WebRequest.cpp _onDisconnect: the callback, then the request is deleted
  ArDisconnectHandler fn = req_->onDisconnect_;
  if (fn) fn();
  delete req_;
  req_ = nullptr;
}

Response perform(const Request& r) {
  Exchange e(r);
  return e.finish();
}

}  // namespace http

void resetWebVolatile() {
  http::Server next;
  next.instance = http::state().instance;
  next.port = http::state().port;
  http::state() = next;
}

}  // namespace fakes

// ---------------------------------------------------------------- AsyncWebServer

AsyncWebServer::AsyncWebServer(uint16_t port) : port_(port) {
  fakes::http::server().instance = this;
  fakes::http::server().port = port;
}

AsyncWebServer::~AsyncWebServer() {
  if (fakes::http::server().instance == this) fakes::http::server().instance = nullptr;
}

void AsyncWebServer::begin() {
  ++fakes::http::server().begins;
  fakes::note("web.begin " + std::to_string(port_));
}

void AsyncWebServer::end() {}

AsyncWebHandler& AsyncWebServer::addHandler(AsyncWebHandler* handler) {
  handlers_.push_back(handler);
  return *handler;
}

bool AsyncWebServer::removeHandler(AsyncWebHandler* handler) {
  for (auto it = handlers_.begin(); it != handlers_.end(); ++it) {
    if (*it == handler) {
      handlers_.erase(it);
      return true;
    }
  }
  return false;
}

void AsyncWebServer::onNotFound(ArRequestHandlerFunction fn) { notFound_ = fn; }

void AsyncWebServer::reset() {
  handlers_.clear();
  notFound_ = nullptr;
}

// ---------------------------------------------------------------- AsyncWebServerRequest

AsyncWebServerRequest::AsyncWebServerRequest(AsyncWebServer* server, AsyncClient* client)
    : server_(server), client_(client) {}

AsyncWebServerRequest::~AsyncWebServerRequest() = default;

const char* AsyncWebServerRequest::methodToString() const {
  switch (method_) {
    case HTTP_GET: return "GET";
    case HTTP_POST: return "POST";
    case HTTP_DELETE: return "DELETE";
    case HTTP_PUT: return "PUT";
    case HTTP_PATCH: return "PATCH";
    case HTTP_HEAD: return "HEAD";
    case HTTP_OPTIONS: return "OPTIONS";
    default: return "UNKNOWN";
  }
}

void AsyncWebServerRequest::onDisconnect(ArDisconnectHandler fn) { onDisconnect_ = fn; }

void AsyncWebServerRequest::addInterestingHeader(const String& name) {
  for (const String& i : interesting_) {
    if (i.equalsIgnoreCase(name)) return;
  }
  interesting_.push_back(name);
}

void AsyncWebServerRequest::redirect(const String& url) {
  AsyncWebServerResponse* res = beginResponse(302);
  if (res != nullptr) res->addHeader("Location", url);
  send(res);
}

AsyncWebServerResponse* AsyncWebServerRequest::newResponse(int code, const String& contentType) {
  if (fakes::http::server().failNextResponse) {
    fakes::http::server().failNextResponse = false;
    return nullptr;
  }
  responses_.emplace_back(new AsyncWebServerResponse());
  AsyncWebServerResponse* r = responses_.back().get();
  r->code_ = code;
  r->contentType_ = contentType;
  return r;
}

void AsyncWebServerRequest::record(AsyncWebServerResponse* response) {
  if (exchange_ != nullptr) exchange_->onSend(response);
}

void AsyncWebServerRequest::send(AsyncWebServerResponse* response) { record(response); }

void AsyncWebServerRequest::send(int code, const String& contentType, const String& content) {
  responses_.emplace_back(new AsyncWebServerResponse());
  AsyncWebServerResponse* r = responses_.back().get();
  r->code_ = code;
  r->contentType_ = contentType;
  r->text_ = content.str();
  record(r);
}

void AsyncWebServerRequest::send(int code, const String& contentType, const char* content, bool) {
  send(code, contentType, String(content != nullptr ? content : ""));
}

void AsyncWebServerRequest::send(const String& contentType, size_t len, AwsResponseFiller callback,
                                 AwsTemplateProcessor) {
  responses_.emplace_back(new AsyncWebServerResponse());
  AsyncWebServerResponse* r = responses_.back().get();
  r->contentType_ = contentType;
  r->kind_ = AsyncWebServerResponse::Kind::Filler;
  r->length_ = len;
  r->filler_ = callback;
  record(r);
}

void AsyncWebServerRequest::sendChunked(const String& contentType, AwsResponseFiller callback,
                                        AwsTemplateProcessor) {
  responses_.emplace_back(new AsyncWebServerResponse());
  AsyncWebServerResponse* r = responses_.back().get();
  r->contentType_ = contentType;
  r->kind_ = AsyncWebServerResponse::Kind::Chunked;
  r->filler_ = callback;
  record(r);
}

void AsyncWebServerRequest::send_P(int code, const String& contentType, const uint8_t* content,
                                   size_t len, AwsTemplateProcessor) {
  responses_.emplace_back(new AsyncWebServerResponse());
  AsyncWebServerResponse* r = responses_.back().get();
  r->code_ = code;
  r->contentType_ = contentType;
  r->kind_ = AsyncWebServerResponse::Kind::Bytes;
  r->data_ = content;
  r->length_ = len;
  record(r);
}

void AsyncWebServerRequest::send_P(int code, const String& contentType, PGM_P content,
                                   AwsTemplateProcessor) {
  send(code, contentType, String(content));
}

AsyncWebServerResponse* AsyncWebServerRequest::beginResponse(int code, const String& contentType,
                                                             const String& content) {
  AsyncWebServerResponse* r = newResponse(code, contentType);
  if (r != nullptr) r->text_ = content.str();
  return r;
}

AsyncWebServerResponse* AsyncWebServerRequest::beginResponse(int code, const String& contentType,
                                                             const char* content) {
  return beginResponse(code, contentType, String(content != nullptr ? content : ""));
}

AsyncWebServerResponse* AsyncWebServerRequest::beginResponse(int code, const String& contentType,
                                                             const uint8_t* content, size_t len,
                                                             AwsTemplateProcessor) {
  AsyncWebServerResponse* r = newResponse(code, contentType);
  if (r != nullptr) r->text_.assign(reinterpret_cast<const char*>(content), len);
  return r;
}

AsyncWebServerResponse* AsyncWebServerRequest::beginResponse(const String& contentType, size_t len,
                                                             AwsResponseFiller callback,
                                                             AwsTemplateProcessor) {
  AsyncWebServerResponse* r = newResponse(200, contentType);
  if (r != nullptr) {
    r->kind_ = AsyncWebServerResponse::Kind::Filler;
    r->length_ = len;
    r->filler_ = callback;
  }
  return r;
}

AsyncWebServerResponse* AsyncWebServerRequest::beginChunkedResponse(const String& contentType,
                                                                    AwsResponseFiller callback,
                                                                    AwsTemplateProcessor) {
  AsyncWebServerResponse* r = newResponse(200, contentType);
  if (r != nullptr) {
    r->kind_ = AsyncWebServerResponse::Kind::Chunked;
    r->filler_ = callback;
  }
  return r;
}

AsyncWebServerResponse* AsyncWebServerRequest::beginResponse_P(int code, const String& contentType,
                                                               const uint8_t* content, size_t len,
                                                               AwsTemplateProcessor) {
  AsyncWebServerResponse* r = newResponse(code, contentType);
  if (r != nullptr) {
    r->kind_ = AsyncWebServerResponse::Kind::Bytes;
    r->data_ = content;
    r->length_ = len;
  }
  return r;
}

AsyncWebServerResponse* AsyncWebServerRequest::beginResponse_P(int code, const String& contentType,
                                                               PGM_P content,
                                                               AwsTemplateProcessor) {
  return beginResponse_P(code, contentType, reinterpret_cast<const uint8_t*>(content),
                         strlen(content));
}

size_t AsyncWebServerRequest::headers() const { return headers_.size(); }

bool AsyncWebServerRequest::hasHeader(const String& name) const {
  return getHeader(name) != nullptr;
}

AsyncWebHeader* AsyncWebServerRequest::getHeader(const String& name) const {
  for (const auto& h : headers_) {
    if (h->name().equalsIgnoreCase(name)) return h.get();
  }
  return nullptr;
}

AsyncWebHeader* AsyncWebServerRequest::getHeader(size_t num) const {
  return num < headers_.size() ? headers_[num].get() : nullptr;
}

size_t AsyncWebServerRequest::params() const { return params_.size(); }

bool AsyncWebServerRequest::hasParam(const String& name, bool post, bool file) const {
  return getParam(name, post, file) != nullptr;
}

AsyncWebParameter* AsyncWebServerRequest::getParam(const String& name, bool post,
                                                   bool file) const {
  for (const auto& p : params_) {
    if (p->name() == name && p->isPost() == post && p->isFile() == file) return p.get();
  }
  return nullptr;
}

AsyncWebParameter* AsyncWebServerRequest::getParam(size_t num) const {
  return num < params_.size() ? params_[num].get() : nullptr;
}

namespace {

const String& emptyString() {
  static const String s;
  return s;
}

}  // namespace

const String& AsyncWebServerRequest::arg(const String& name) const {
  for (const auto& p : params_) {
    if (p->name() == name) return p->value();
  }
  return emptyString();
}

const String& AsyncWebServerRequest::arg(size_t i) const {
  return i < params_.size() ? params_[i]->value() : emptyString();
}

const String& AsyncWebServerRequest::argName(size_t i) const {
  return i < params_.size() ? params_[i]->name() : emptyString();
}

bool AsyncWebServerRequest::hasArg(const char* name) const {
  for (const auto& p : params_) {
    if (p->name() == name) return true;
  }
  return false;
}

const String& AsyncWebServerRequest::header(const char* name) const {
  AsyncWebHeader* h = getHeader(String(name));
  return h != nullptr ? h->value() : emptyString();
}

const String& AsyncWebServerRequest::header(size_t i) const {
  return i < headers_.size() ? headers_[i]->value() : emptyString();
}

const String& AsyncWebServerRequest::headerName(size_t i) const {
  return i < headers_.size() ? headers_[i]->name() : emptyString();
}

String AsyncWebServerRequest::urlDecode(const String& text) const {
  return String(fakes::http::decode(text.str()));
}
