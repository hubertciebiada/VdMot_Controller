// OtaValidator, normalizeMd5.
#include <string>

#include "doctest.h"
#include "vdm/ota_policy.h"

using namespace vdm;

using D = OtaValidator::Decision;

namespace {

// Self-checks ok every 10 s from `from` to `to` (inclusive) with update() every second.
D runHealthy(OtaValidator& v, uint32_t from, uint32_t to, bool net, bool link) {
  D last = D::Wait;
  for (uint32_t i = 0; i <= (to - from) / 1000; ++i) {
    const uint32_t t = from + i * 1000;
    if (v.selfCheckDue(true, t)) v.onSelfCheck(true, t);
    last = v.update(net, link, t);
    if (last != D::Wait) return last;
  }
  return last;
}

}  // namespace

TEST_CASE("OtaValidator: not pending -> NotPending forever") {
  OtaValidator v;
  CHECK(v.update(true, true, 0) == D::NotPending);
  v.begin(false, true, 0);
  CHECK(v.update(true, true, 0) == D::NotPending);
  CHECK(v.update(false, false, 4000000000u) == D::NotPending);
  CHECK_FALSE(v.pending());
  CHECK(v.stmRequired());
  CHECK_FALSE(v.selfCheckDue(true, 0));
  CHECK(v.remainingMs(0) == 0);
  CHECK(v.healthyForMs(0) == 0);
  CHECK(v.missing() == 0);
}

TEST_CASE("OtaValidator: self-check due at once, then every 10 s, only with the web server") {
  OtaValidator v;
  v.begin(true, false, 0);
  CHECK_FALSE(v.selfCheckDue(false, 0));
  CHECK(v.selfCheckDue(true, 0));
  CHECK(v.selfCheckDue(true, 5000));
  v.onSelfCheck(true, 0);
  CHECK_FALSE(v.selfCheckDue(true, 9999));
  CHECK(v.selfCheckDue(true, 10000));
  CHECK_FALSE(v.selfCheckDue(false, 10000));
  v.onSelfCheck(false, 10000);
  CHECK_FALSE(v.selfCheckDue(true, 19999));
  CHECK(v.selfCheckDue(true, 20000));
}

TEST_CASE("OtaValidator: net + http, stm not required: valid at exactly 120 s") {
  OtaValidator v;
  v.begin(true, false, 0);
  CHECK(v.pending());
  CHECK_FALSE(v.stmRequired());
  CHECK(v.remainingMs(0) == 900000);
  CHECK(runHealthy(v, 0, 119000, true, false) == D::Wait);
  CHECK(v.healthyForMs(119000) == 119000);
  CHECK(v.healthyForMs(119999) == 119999);
  CHECK(v.remainingMs(119000) == 781000);
  CHECK(v.missing() == 0);
  CHECK(v.update(true, false, 119999) == D::Wait);
  CHECK(v.update(true, false, 120000) == D::MarkValid);
  CHECK_FALSE(v.pending());
  CHECK(v.update(true, false, 120001) == D::NotPending);
  CHECK(v.remainingMs(120001) == 0);
  CHECK(v.healthyForMs(120001) == 0);
}

TEST_CASE("OtaValidator: stm required and link down -> rollback at 900 s, missing stm") {
  OtaValidator v;
  v.begin(true, true, 0);
  CHECK(runHealthy(v, 0, 899000, true, false) == D::Wait);
  CHECK(v.missing() == OtaValidator::kCheckStm);
  CHECK(v.healthyForMs(899000) == 0);
  CHECK(v.remainingMs(899999) == 1);
  CHECK(v.update(true, false, 899999) == D::Wait);
  CHECK(v.update(true, false, 900000) == D::Rollback);
  CHECK(v.missing() == OtaValidator::kCheckStm);
  CHECK(v.update(true, true, 900001) == D::NotPending);
}

TEST_CASE("OtaValidator: stm required and link up -> valid") {
  OtaValidator v;
  v.begin(true, true, 1000);
  CHECK(runHealthy(v, 1000, 121000, true, true) == D::MarkValid);
}

TEST_CASE("OtaValidator: the missing bits") {
  OtaValidator v;
  v.begin(true, true, 0);
  CHECK(v.update(false, false, 0) ==  D::Wait);
  CHECK(v.missing() == (OtaValidator::kCheckNet | OtaValidator::kCheckHttp |
                        OtaValidator::kCheckStm));
  v.onSelfCheck(true, 0);
  CHECK(v.update(false, true, 1000) == D::Wait);
  CHECK(v.missing() == OtaValidator::kCheckNet);
  CHECK(v.update(true, true, 2000) == D::Wait);
  CHECK(v.missing() == 0);
  CHECK(OtaValidator::kCheckNet == 1);
  CHECK(OtaValidator::kCheckHttp == 2);
  CHECK(OtaValidator::kCheckStm == 4);
}

TEST_CASE("OtaValidator: a self-check result is fresh for 30 s") {
  OtaValidator v;
  v.begin(true, false, 0);
  v.onSelfCheck(true, 0);
  CHECK(v.httpOk(29999));
  CHECK_FALSE(v.httpOk(30000));
  CHECK(v.update(true, false, 29999) == D::Wait);
  CHECK(v.missing() == 0);
  CHECK(v.update(true, false, 30000) == D::Wait);
  CHECK(v.missing() == OtaValidator::kCheckHttp);
  CHECK(v.healthyForMs(30000) == 0);
}

TEST_CASE("OtaValidator: a failed self-check ends the healthy period at once") {
  OtaValidator v;
  v.begin(true, false, 0);
  CHECK(runHealthy(v, 0, 60000, true, false) == D::Wait);
  CHECK(v.healthyForMs(60000) == 60000);
  v.onSelfCheck(false, 60500);
  CHECK_FALSE(v.httpOk(60500));
  CHECK(v.healthyForMs(60500) == 0);
  CHECK(v.update(true, false, 61000) == D::Wait);
  CHECK(v.missing() == OtaValidator::kCheckHttp);
  v.onSelfCheck(true, 62000);
  CHECK(v.update(true, false, 62000) == D::Wait);
  // The healthy period restarted at 62 s.
  CHECK(runHealthy(v, 63000, 181000, true, false) == D::Wait);
  CHECK(v.update(true, false, 182000) == D::MarkValid);
}

TEST_CASE("OtaValidator: network and link without a self-check never mark valid") {
  OtaValidator v;
  v.begin(true, true, 0);
  for (uint32_t t = 0; t < 900000; t += 1000) {
    REQUIRE(v.update(true, true, t) == D::Wait);
  }
  CHECK(v.update(true, true, 900000) == D::Rollback);
  CHECK(v.missing() == OtaValidator::kCheckHttp);
}

TEST_CASE("OtaValidator: an unhealthy second restarts the period") {
  OtaValidator v;
  v.begin(true, false, 0);
  CHECK(runHealthy(v, 0, 60000, true, false) == D::Wait);
  CHECK(v.update(false, false, 61000) == D::Wait);
  CHECK(v.healthyForMs(61000) == 0);
  CHECK(runHealthy(v, 62000, 181000, true, false) == D::Wait);
  CHECK(v.healthyForMs(181000) == 119000);
  CHECK(v.update(true, false, 182000) == D::MarkValid);
}

TEST_CASE("OtaValidator: MarkValid wins over Rollback in the same second") {
  OtaValidator v(10, 30);
  v.begin(true, false, 0);
  v.onSelfCheck(true, 20);
  CHECK(v.update(true, false, 20) == D::Wait);
  CHECK(v.update(true, false, 30) == D::MarkValid);
  OtaValidator r(10, 30);
  r.begin(true, false, 0);
  r.onSelfCheck(true, 25);
  CHECK(r.update(true, false, 25) == D::Wait);
  CHECK(r.update(true, false, 30) == D::Rollback);
}

TEST_CASE("OtaValidator: begin twice re-arms everything") {
  OtaValidator v;
  v.begin(true, true, 0);
  v.onSelfCheck(true, 0);
  CHECK(v.update(true, false, 900000) == D::Rollback);
  v.begin(true, false, 1000000);
  CHECK(v.pending());
  CHECK_FALSE(v.stmRequired());
  CHECK(v.missing() == 0);
  CHECK(v.selfCheckDue(true, 1000000));
  CHECK_FALSE(v.httpOk(1000000));
  CHECK(v.remainingMs(1000000) == 900000);
  CHECK(v.update(true, false, 1000000) == D::Wait);
  CHECK(v.missing() == OtaValidator::kCheckHttp);
}

TEST_CASE("OtaValidator: start near the 32-bit wrap") {
  OtaValidator v;
  const uint32_t s = 0xFFFFF000u;
  v.begin(true, false, s);
  CHECK(v.remainingMs(s + 1000u) == 899000);
  CHECK(runHealthy(v, s, s + 119000u, true, false) == D::Wait);
  CHECK(v.update(true, false, s + 120000u) == D::MarkValid);
  OtaValidator r;
  r.begin(true, false, s);
  CHECK(r.update(false, false, s + 899999u) == D::Wait);
  CHECK(r.update(false, false, s + 900000u) == D::Rollback);
}

TEST_CASE("OtaValidator: confirmBeforeRestart over userRequested x netUp x linkUp x stmRequired") {
  for (int stmReq = 0; stmReq < 2; ++stmReq) {
    for (int user = 0; user < 2; ++user) {
      for (int netUp = 0; netUp < 2; ++netUp) {
        for (int link = 0; link < 2; ++link) {
          OtaValidator v;
          v.begin(true, stmReq != 0, 0);
          const bool expected = user && netUp && (!stmReq || link);
          INFO(stmReq << user << netUp << link);
          CHECK(v.confirmBeforeRestart(user != 0, netUp != 0, link != 0) == expected);
          CHECK(v.pending() == !expected);
          CHECK_FALSE(v.confirmBeforeRestart(true, true, true) == expected);
        }
      }
    }
  }
  OtaValidator notPending;
  notPending.begin(false, false, 0);
  CHECK_FALSE(notPending.confirmBeforeRestart(true, true, true));
  OtaValidator confirmed;
  confirmed.begin(true, false, 0);
  CHECK(confirmed.confirmBeforeRestart(true, true, false));
  CHECK(confirmed.update(false, false, 900000) == D::NotPending);
}

TEST_CASE("normalizeMd5") {
  char out[33] = "unchanged";
  CHECK(normalizeMd5("09afAF0123456789abcdefABCDEF0123", out));
  CHECK(std::string(out) == "09afaf0123456789abcdefabcdef0123");
  CHECK(normalizeMd5("GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG", out) == false);
  CHECK(std::string(out) == "09afaf0123456789abcdefabcdef0123");
  CHECK_FALSE(normalizeMd5(nullptr, out));
  CHECK_FALSE(normalizeMd5("", out));
  CHECK_FALSE(normalizeMd5("09afAF0123456789abcdefABCDEF012", out));    // 31
  CHECK_FALSE(normalizeMd5("09afAF0123456789abcdefABCDEF01234", out));  // 33
  CHECK_FALSE(normalizeMd5("09afAF0123456789abcdefABCDEF012g", out));
  CHECK_FALSE(normalizeMd5("/9afAF0123456789abcdefABCDEF0123", out));   // '0' - 1
  CHECK_FALSE(normalizeMd5(":9afAF0123456789abcdefABCDEF0123", out));   // '9' + 1
  CHECK_FALSE(normalizeMd5("`9afAF0123456789abcdefABCDEF0123", out));   // 'a' - 1
  CHECK_FALSE(normalizeMd5("@9afAF0123456789abcdefABCDEF0123", out));   // 'A' - 1
  CHECK_FALSE(normalizeMd5("G9afAF0123456789abcdefABCDEF0123", out));
  CHECK(normalizeMd5("0123456789abcdefABCDEF0000000000", out));
  CHECK(std::string(out) == "0123456789abcdefabcdef0000000000");
  CHECK(normalizeMd5("ffffffffffffffffffffffffffffffff", out));
  CHECK(std::string(out) == "ffffffffffffffffffffffffffffffff");
  CHECK(normalizeMd5("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF", out));
  CHECK(std::string(out) == "ffffffffffffffffffffffffffffffff");
  CHECK(normalizeMd5("99999999999999999999999999999999", out));
  CHECK(std::string(out) == "99999999999999999999999999999999");
}
