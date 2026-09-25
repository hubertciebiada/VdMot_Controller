// vdm::driveTarget: where a valve is driven to and why (contracts 2.5).
#include <stdint.h>

#include "doctest.h"
#include "vdm/failsafe.h"

using vdm::DriveSource;

namespace {

void checkDrive(uint8_t target, uint8_t fs, uint8_t status, bool expired, bool hold, uint8_t pos, DriveSource src) {
  CAPTURE(+target);
  CAPTURE(+fs);
  CAPTURE(+status);
  CAPTURE(expired);
  CAPTURE(hold);
  const vdm::Drive d = vdm::driveTarget(target, fs, status, expired, hold);
  CHECK(+d.position == +pos);
  CHECK(d.source == src);
}

}  // namespace

TEST_CASE("driveTarget: truth table over status, failsafe, lease and assembly hold") {
  CHECK(static_cast<int>(DriveSource::Target) == 0);
  CHECK(static_cast<int>(DriveSource::LeaseFailsafe) == 1);
  CHECK(static_cast<int>(DriveSource::BlockedFailsafe) == 2);
  const uint8_t statuses[] = {1, 4, 6, 8, 9};
  const uint8_t positions[] = {0, 50, 100, 255};
  for (uint8_t status : statuses) {
    for (uint8_t fs : positions) {
      for (int e = 0; e < 2; e++) {
        for (int h = 0; h < 2; h++) {
          const bool expired = e != 0;
          const bool hold = h != 0;
          if (fs == 255) {
            checkDrive(30, fs, status, expired, hold, 30, DriveSource::Target);
          } else if (status == 9) {
            checkDrive(30, fs, status, expired, hold, fs, DriveSource::BlockedFailsafe);
          } else if (status == 4 || status == 6) {
            checkDrive(30, fs, status, expired, hold, 30, DriveSource::Target);
          } else if (expired && !hold) {
            checkDrive(30, fs, status, expired, hold, fs, DriveSource::LeaseFailsafe);
          } else {
            checkDrive(30, fs, status, expired, hold, 30, DriveSource::Target);
          }
        }
      }
    }
  }
  // the other states follow the lease like idle
  checkDrive(70, 20, 2, true, false, 20, DriveSource::LeaseFailsafe);
  checkDrive(70, 20, 3, true, false, 20, DriveSource::LeaseFailsafe);
  checkDrive(70, 20, 5, true, false, 20, DriveSource::LeaseFailsafe);
  checkDrive(70, 20, 7, true, true, 70, DriveSource::Target);
  checkDrive(70, 20, 7, false, false, 70, DriveSource::Target);
  checkDrive(70, 254, 9, false, false, 254, DriveSource::BlockedFailsafe);
}
