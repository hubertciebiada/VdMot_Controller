// OtaValidator.
#include "doctest.h"
#include "vdm/ota_policy.h"

using namespace vdm;

TEST_CASE("OtaValidator") {
  OtaValidator none;
  CHECK(none.update(true, true, 0) == OtaValidator::Decision::NotPending);
  none.begin(false, 0);
  CHECK(none.update(true, true, 999999) == OtaValidator::Decision::NotPending);

  OtaValidator v;
  v.begin(true, 5000);
  CHECK(v.update(true, true, 5000) == OtaValidator::Decision::Wait);
  CHECK(v.update(true, true, 5000 + 119999) == OtaValidator::Decision::Wait);
  CHECK(v.update(true, true, 5000 + 120000) == OtaValidator::Decision::MarkValid);
  CHECK(v.update(true, true, 5000 + 120001) == OtaValidator::Decision::NotPending);

  OtaValidator gap;
  gap.begin(true, 0);
  CHECK(gap.update(true, true, 0) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, false, 60000) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, true, 61000) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, true, 180999) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, true, 181000) == OtaValidator::Decision::MarkValid);

  OtaValidator netOnly;
  netOnly.begin(true, 0);
  CHECK(netOnly.update(false, true, 0) == OtaValidator::Decision::Wait);
  CHECK(netOnly.update(true, false, 599999) == OtaValidator::Decision::Wait);
  CHECK(netOnly.update(false, false, 600000) == OtaValidator::Decision::Wait);
  CHECK(netOnly.update(true, false, 600001) == OtaValidator::Decision::MarkValid);
  OtaValidator netExact;
  netExact.begin(true, 0);
  CHECK(netExact.update(true, false, 600000) == OtaValidator::Decision::MarkValid);

  // The healthy period starts with the first healthy observation.
  OtaValidator late;
  late.begin(true, 0);
  CHECK(late.update(true, true, 50) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 120000) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 120050) == OtaValidator::Decision::MarkValid);
  // begin() forgets an earlier healthy period.
  late.begin(true, 200000);
  CHECK(late.update(true, true, 200100) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 320000) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 320100) == OtaValidator::Decision::MarkValid);

  OtaValidator rb;
  rb.begin(true, 0xFFFFF000u);  // wrap-safe
  CHECK(rb.update(false, true, 0xFFFFF000u + 899999u) == OtaValidator::Decision::Wait);
  CHECK(rb.update(false, true, 0xFFFFF000u + 900000u) == OtaValidator::Decision::Rollback);
  CHECK(rb.update(true, true, 0xFFFFF000u + 900001u) == OtaValidator::Decision::NotPending);

  OtaValidator custom(10, 20, 30);
  custom.begin(true, 0);
  CHECK(custom.update(true, true, 0) == OtaValidator::Decision::Wait);
  CHECK(custom.update(true, true, 9) == OtaValidator::Decision::Wait);
  CHECK(custom.update(true, true, 10) == OtaValidator::Decision::MarkValid);
  custom.begin(true, 100);  // a second begin re-arms
  CHECK(custom.update(false, false, 129) == OtaValidator::Decision::Wait);
  CHECK(custom.update(false, false, 130) == OtaValidator::Decision::Rollback);
}

TEST_CASE("OtaValidator: a user restart of a pending image confirms it first") {
  OtaValidator v;
  v.begin(true, 0);
  CHECK(v.pending());
  CHECK_FALSE(v.confirmBeforeRestart(false, true));  // network watchdog etc.
  CHECK_FALSE(v.confirmBeforeRestart(true, false));  // network down: no proof
  CHECK(v.pending());
  CHECK(v.update(true, false, 1000) == OtaValidator::Decision::Wait);
  CHECK(v.confirmBeforeRestart(true, true));
  CHECK_FALSE(v.pending());
  CHECK_FALSE(v.confirmBeforeRestart(true, true));  // once
  CHECK(v.update(false, false, 900000) == OtaValidator::Decision::NotPending);

  OtaValidator notPending;
  notPending.begin(false, 0);
  CHECK_FALSE(notPending.confirmBeforeRestart(true, true));
  // Decided already (rolled back or valid): nothing left to confirm.
  OtaValidator decided(10, 20, 30);
  decided.begin(true, 0);
  CHECK(decided.update(true, true, 0) == OtaValidator::Decision::Wait);
  CHECK(decided.update(true, true, 10) == OtaValidator::Decision::MarkValid);
  CHECK_FALSE(decided.confirmBeforeRestart(true, true));
}
