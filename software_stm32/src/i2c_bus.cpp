/**HEADER*******************************************************************
  project : VdMot Controller
  Comments: I2C bus (EEPROM) recovery
***************************************************************************/

#include <Arduino.h>
#include "hardware.h"
#include "i2c_bus.h"

#define I2C_RECOVERY_HALF_CLOCK_US  5     // 100 kHz


// A slave that was reset in the middle of a transfer (e.g. the EEPROM during a read when the STM
// was reset) may hold SDA low forever. Clock SCL until it releases SDA, then send a STOP.
void i2c_bus_recover() {
  pinMode(I2C_SDA_PIN, INPUT);
  digitalWrite(I2C_SCL_PIN, HIGH);
  pinMode(I2C_SCL_PIN, OUTPUT_OPEN_DRAIN);
  delayMicroseconds(I2C_RECOVERY_HALF_CLOCK_US);

  for (uint8_t i = 0; i < 9 && digitalRead(I2C_SDA_PIN) == LOW; i++) {
    digitalWrite(I2C_SCL_PIN, LOW);
    delayMicroseconds(I2C_RECOVERY_HALF_CLOCK_US);
    digitalWrite(I2C_SCL_PIN, HIGH);
    delayMicroseconds(I2C_RECOVERY_HALF_CLOCK_US);
  }

  // STOP: SDA rises while SCL is high
  digitalWrite(I2C_SCL_PIN, LOW);
  digitalWrite(I2C_SDA_PIN, LOW);
  pinMode(I2C_SDA_PIN, OUTPUT_OPEN_DRAIN);
  delayMicroseconds(I2C_RECOVERY_HALF_CLOCK_US);
  digitalWrite(I2C_SCL_PIN, HIGH);
  delayMicroseconds(I2C_RECOVERY_HALF_CLOCK_US);
  digitalWrite(I2C_SDA_PIN, HIGH);
  delayMicroseconds(I2C_RECOVERY_HALF_CLOCK_US);

  pinMode(I2C_SDA_PIN, INPUT);
  pinMode(I2C_SCL_PIN, INPUT);
}


// not used yet: EEPROM retries do not restart the bus
void i2c_bus_restart() {
}
