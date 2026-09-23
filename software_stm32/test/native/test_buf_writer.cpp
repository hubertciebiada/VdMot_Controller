#include <stdint.h>
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/buf_writer.h"

using vdm::BufWriter;
using vdm::StaticBufWriter;

TEST_CASE("BufWriter: starts empty and terminated") {
  char buf[4] = {'x', 'x', 'x', 'x'};
  BufWriter w(buf, sizeof(buf));
  CHECK(w.length() == 0);
  CHECK(w.capacity() == 4);
  CHECK(w.ok());
  CHECK(buf[0] == '\0');
  CHECK(std::string(w.c_str()).empty());
}

TEST_CASE("BufWriter: strings and characters up to capacity") {
  StaticBufWriter<6> w;  // 5 characters
  CHECK(w.append("ab"));
  CHECK(w.append('c'));
  CHECK(w.append(""));
  CHECK(w.append("de"));
  CHECK(std::string(w.c_str()) == "abcde");
  CHECK(w.length() == 5);
  CHECK(w.ok());
  CHECK(w.append(""));  // empty always fits
  CHECK_FALSE(w.append('f'));
  CHECK_FALSE(w.ok());
  CHECK(std::string(w.c_str()) == "abcde");
}

TEST_CASE("BufWriter: all-or-nothing appends") {
  StaticBufWriter<6> w;
  CHECK(w.append("abc"));
  CHECK_FALSE(w.append("def"));  // needs 3, only 2 free
  CHECK(std::string(w.c_str()) == "abc");
  CHECK_FALSE(w.ok());
  // Later smaller appends still work but ok() stays false.
  CHECK(w.append("de"));
  CHECK(std::string(w.c_str()) == "abcde");
  CHECK_FALSE(w.ok());
  w.clear();
  CHECK(w.ok());
  CHECK(w.length() == 0);
  CHECK(std::string(w.c_str()).empty());
}

TEST_CASE("BufWriter: null string and degenerate buffers") {
  StaticBufWriter<8> w;
  CHECK_FALSE(w.append(static_cast<const char*>(nullptr)));
  CHECK_FALSE(w.ok());
  CHECK(w.length() == 0);

  BufWriter none(nullptr, 10);
  CHECK(none.capacity() == 0);
  CHECK_FALSE(none.append('a'));
  CHECK_FALSE(none.append(""));
  CHECK(std::string(none.c_str()).empty());
  none.clear();
  none.truncate(0);
  CHECK(none.length() == 0);

  char one[1] = {'x'};
  BufWriter tiny(one, 1);
  CHECK(one[0] == '\0');
  CHECK(tiny.append(""));
  CHECK_FALSE(tiny.append('a'));
  CHECK(one[0] == '\0');

  char zero[1] = {'x'};
  BufWriter empty(zero, 0);
  CHECK(zero[0] == 'x');  // capacity 0: buffer is never touched
  CHECK_FALSE(empty.append(""));
  CHECK(zero[0] == 'x');
}

TEST_CASE("BufWriter: unsigned numbers") {
  StaticBufWriter<64> w;
  CHECK(w.appendUnsigned(0));
  CHECK(w.append(' '));
  CHECK(w.appendUnsigned(9));
  CHECK(w.append(' '));
  CHECK(w.appendUnsigned(10));
  CHECK(w.append(' '));
  CHECK(w.appendUnsigned(65535));
  CHECK(w.append(' '));
  CHECK(w.appendUnsigned(UINT32_MAX));
  CHECK(std::string(w.c_str()) == "0 9 10 65535 4294967295");
}

TEST_CASE("BufWriter: signed numbers") {
  StaticBufWriter<64> w;
  CHECK(w.appendSigned(0));
  CHECK(w.append(' '));
  CHECK(w.appendSigned(-1));
  CHECK(w.append(' '));
  CHECK(w.appendSigned(-1270));
  CHECK(w.append(' '));
  CHECK(w.appendSigned(INT32_MAX));
  CHECK(w.append(' '));
  CHECK(w.appendSigned(INT32_MIN));
  CHECK(w.append(' '));
  CHECK(w.appendSigned(10));
  CHECK(std::string(w.c_str()) == "0 -1 -1270 2147483647 -2147483648 10");
}

TEST_CASE("BufWriter: numbers that do not fit are not written") {
  StaticBufWriter<4> w;  // 3 characters
  CHECK(w.appendUnsigned(999));
  CHECK_FALSE(w.appendUnsigned(1));
  w.clear();
  CHECK_FALSE(w.appendUnsigned(1000));
  CHECK(w.length() == 0);
  CHECK(w.appendSigned(-99));
  w.clear();
  CHECK_FALSE(w.appendSigned(-100));
  CHECK(w.length() == 0);
  CHECK_FALSE(w.appendSigned(INT32_MIN));
  CHECK(std::string(w.c_str()).empty());
}

TEST_CASE("BufWriter: hex bytes") {
  StaticBufWriter<16> w;
  CHECK(w.appendHex2(0x00));
  CHECK(w.appendHex2(0x0f));
  CHECK(w.appendHex2(0xa0));
  CHECK(w.appendHex2(0xff));
  CHECK(w.appendHex2(0x19));
  CHECK(std::string(w.c_str()) == "000fa0ff19");
  StaticBufWriter<2> small;
  CHECK_FALSE(small.appendHex2(0x12));
  CHECK(small.length() == 0);
}

TEST_CASE("BufWriter: 1-Wire addresses match the v1 wire format") {
  const uint8_t addr[8] = {0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23};
  StaticBufWriter<24> w;  // exactly 23 characters + NUL
  CHECK(w.appendOneWireAddress(addr));
  CHECK(std::string(w.c_str()) == "28-84-37-94-97-ff-03-23");
  CHECK(w.length() == 23);

  const uint8_t zero[8] = {};
  StaticBufWriter<23> tooSmall;
  CHECK_FALSE(tooSmall.appendOneWireAddress(zero));
  CHECK(tooSmall.length() == 0);

  StaticBufWriter<32> z;
  CHECK(z.appendOneWireAddress(zero));
  CHECK(std::string(z.c_str()) == "00-00-00-00-00-00-00-00");
}

TEST_CASE("BufWriter: truncate") {
  StaticBufWriter<16> w;
  CHECK(w.append("hello"));
  w.truncate(10);  // longer than content: no-op
  CHECK(std::string(w.c_str()) == "hello");
  w.truncate(2);
  CHECK(std::string(w.c_str()) == "he");
  CHECK(w.length() == 2);
  CHECK_FALSE(w.append("0123456789abcdef"));
  w.truncate(0);
  CHECK(std::string(w.c_str()).empty());
  CHECK_FALSE(w.ok());  // truncate does not reset the error flag
}
