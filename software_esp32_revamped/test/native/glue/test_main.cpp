// Smoke tests of src/main.cpp: the Arduino-ESP32 hooks and entry points.
#include <Arduino.h>

#include "glue_test.h"

TEST_CASE("main: verifyRollbackLater keeps a new image pending verification") {
  glue::begin();
  CHECK(verifyRollbackLater());
}

TEST_CASE("main: initVariant releases the STM from reset, nothing else") {
  glue::begin();
  initVariant();
  CHECK(sib::stmLink().releaseResets == 1);
  CHECK(sib::app().setupCalls == 0);
  CHECK(fakes::journal() == std::vector<std::string>{"stm_link.releaseReset"});
}

TEST_CASE("main: setup runs app::setup once") {
  glue::begin();
  setup();
  CHECK(sib::app().setupCalls == 1);
  CHECK(sib::stmLink().releaseResets == 0);
}

TEST_CASE("main: the Arduino loop task deletes itself") {
  glue::begin();
  CHECK_THROWS_AS(loop(), fakes::TaskDeleted);
  CHECK(fakes::rtos().deleted == std::vector<TaskHandle_t>{nullptr});
}
