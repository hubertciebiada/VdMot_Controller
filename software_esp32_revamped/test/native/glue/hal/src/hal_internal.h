// Links between the hal sources (not for tests).
#pragma once

namespace fakes {

// Volatile state of each area back to the start of a boot (fakes::reset()).
void resetFsVolatile();
void resetNvsVolatile();
void resetOtaVolatile();
void resetNetVolatile();
void resetMqttVolatile();
void resetWebVolatile();

// The WiFiClient a PubSubClient was built with is the broker socket: its stop() ends the session.
bool isMqttSocket(const void* client);
void mqttSocketStopped();
bool mqttSocketConnected();

}  // namespace fakes
