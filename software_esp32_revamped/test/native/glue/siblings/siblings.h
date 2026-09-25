// Sibling fakes of the ESP glue modules (namespace sib): every function of src/<module>.h with
// scripted results and recorded calls. The executable of glue file X links all sibling fakes
// except siblings/fake_X.cpp, so X runs against fakes of the modules it calls. Calls that
// matter for ordering are journaled ("stm_link.begin", "logger.service 0", ...).
//
// The state of the fake of module X is declared in sib_X.h: a new function in src/X.h gets its
// fake in fake_X.cpp and its knobs in sib_X.h, both kept with src/X.cpp.
#pragma once

#include "sib_app.h"
#include "sib_logger.h"
#include "sib_mqtt_client.h"
#include "sib_net.h"
#include "sib_ota.h"
#include "sib_stm_link.h"
#include "sib_stm_service.h"
#include "sib_storage.h"
#include "sib_web_server.h"

namespace sib {

// Every sibling back to its defaults (glue::begin()).
void reset();

}  // namespace sib
