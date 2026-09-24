/**HEADER*******************************************************************
  project : VdMot Controller
  author : Lenti84
  Comments:
  Version :
  Modifcations :
***************************************************************************
*
* THIS SOFTWARE IS PROVIDED "AS IS" AND ANY EXPRESSED OR
* IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
* OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
* IN NO EVENT SHALL THE DEVELOPER OR ANY CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
* INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
* (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
* SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
* HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
* STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
* IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF
* THE POSSIBILITY OF SUCH DAMAGE.
*
**************************************************************************
  This program is free software: you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation, either version 3 of the License.
  See the GNU General Public License for more details.
  You should have received a copy of the GNU General Public License
  along with this program.  If not, see <http://www.gnu.org/licenses/>.
  Copyright (C) 2021 Lenti84  https://github.com/Lenti84/VdMot_Controller
*END************************************************************************/

#include <Arduino.h>
#include "hardware.h"
#include <Wire.h>
//#include <LiquidCrystal_PCF8574.h>
#include "motor.h"
#include "app.h"
#include "terminal.h"
#include "communication.h"
#include "owDevices.h"
#include "eeprom.h"
#include "otasupport.h"
#include "STM32TimerInterrupt.h"      
#include <IWatchdog.h>
#ifdef useCan
  #include "mycan.h"
#endif
#ifdef useRS485
  #include "rs485.h"
#endif

//using Matthias Hertel driver https://github.com/mathertel/LiquidCrystal_PCF8574
//LiquidCrystal_PCF8574 lcd(0x27);  // set the LCD address to 0x27 for a 16 chars and 2 line display

// info
// serial rx/tx buffer sizes (1024) are set by build flags in platformio.ini,
// no changes to the arduino core are needed

// Init STM32 timer TIM2
STM32Timer ITimer1(TIM2);

// independent watchdog, started once the boot window for STM flashing has passed and fed by the
// main loop only while the valve state machine (TIM2) runs and makes progress.
// The LSI clock of the IWDG may run at 17..47 kHz instead of 32 kHz, so 8 s nominal is 5.4..15 s real.
// The longest blocking operations stay below 5 s, also on a faulty bus: 1-Wire enumeration
// (2 searches of at most 64 passes, ~2 s), EEPROM layout write (~0.2 s, or up to ~0.3 s as it
// stops at the first I2C error), EEPROM layout read (~0.1 s, or up to ~2 s as it stops after the
// first block that fails 3 times). setup_system() feeds the watchdog between these steps.
#define WATCHDOG_TIMEOUT_US   8000000UL

#define I2C_RECOVERY_HALF_CLOCK_US  5     // 100 kHz

void setup_system();
void loop_system();


void setup() {
  valve_pins_safe();
  BootSetup();
}


void loop() {

  BootLoop();

  if (bootstate) {
    setup_system() ;

    while(1) {
      loop_system();
    }
  }
}


// A slave that was reset in the middle of a transfer (e.g. the EEPROM during a read when the STM
// was reset) may hold SDA low forever. Clock SCL until it releases SDA, then send a STOP.
static void i2c_bus_recover() {
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


void setup_system() {

  //JumpToBootloader();

  // the ESP may only flash the STM in the boot window before (BootLoop), the IWDG cannot be stopped
  const bool watchdogReset = IWatchdog.isReset(true);
  IWatchdog.begin(WATCHDOG_TIMEOUT_US);

  i2c_bus_recover();
  Wire.setSDA(I2C_SDA_PIN); // pins must be set before begin()
  Wire.setSCL(I2C_SCL_PIN); // 
  Wire.begin();

  // while(1){
  // Wire.beginTransmission(0x71);
  // Wire.write('v');
  // Wire.endTransmission();
  // delay(1000);
  // }

  // power enable for valves
  pinMode(POWER_ENA, OUTPUT_OPEN_DRAIN);
  digitalWrite(POWER_ENA, 1);   // high to disable valve PSU

  // status led
  pinMode(LED, OUTPUT);

  // button 
  pinMode(BUTTON, INPUT_PULLUP);

  // analog inputs
  analogReadResolution(12);
  pinMode(ANINCURRENT, INPUT_ANALOG);
  pinMode(ANINREFHALF, INPUT_ANALOG);

  // terminal for debug
  Terminal_Init();
  if (watchdogReset) COMM_DBG.println("reset by watchdog");

  // serial communication to ESP32
  communication_setup();

  // LCD
  // lcd.begin(16,2);  
  // lcd.setBacklight(255);
  // lcd.setCursor(0, 0);
  // lcd.print("Menu 4.x LCD");
  // lcd.setCursor(0, 1);
  // lcd.print("r-site.net");
  delay(500);

  // EEPROM
  IWatchdog.reload();
  eepromsetup();
  eeprom_read_layout (&eep_content);
 
  // setup onewire temperature sensor at DS2482
  IWatchdog.reload();
  temperature_setup();
  IWatchdog.reload();


  // valve app setup
  app_setup();
  valve_setup();

  #ifdef useCan
    // can
    //mycan_setup();          // can hardware testcode setup
  #endif
  
  #ifdef useRS485
    // rs485
    //rs485_setup();          // rs485 hardware testcode setup
  #endif

  // hardware timer for motor loop
  if (ITimer1.attachInterruptInterval(10 * 1000, valve_loop))
  {
    #ifdef motDebug 
        COMM_DBG.print(F("Starting ITimer1 OK, millis() = ")); COMM_DBG.println(millis());
    #endif
  }
  else {
    #ifdef motDebug
      COMM_DBG.println(F("Can't set ITimer1. Select another freq. or timer"));
    #endif
  }
}


void loop_system() {
  //static int x = 0;
  static int time10s = 0;
  static uint32_t loop_10ms = 0;
  static uint32_t loop_100ms = 0;
  static uint32_t loop_1000ms = 0;

  static uint8_t buttontest = 0;
  static uint8_t ledTimer = 0;
  static uint32_t lastValveTicks = 0;

  int16_t recvcmd;
  

  // 1000 ms loop
  if ((millis()-loop_1000ms) > (uint32_t) 1000 ) {  
    loop_1000ms = millis();

    if(time10s>=10) {
      time10s = 0;
      app_10s_loop();
      // todo sync with comm COMM_SER.println("STMalive ");   // send alive to ESP32
    }
    else time10s++;

    //digitalWrite(LED, !digitalRead(LED));   // toggle LED    
    eepromloop();

  }


  // 100 ms loop
  if ((millis()-loop_100ms) > (uint32_t) 100 ) {  
    loop_100ms = millis();  
    ledTimer++;
    if (ledTimer==30) {
      digitalWrite(LED,LOW);  
    }
    if (ledTimer==31) {
      digitalWrite(LED,HIGH);
      ledTimer=0;  
    }
    recvcmd = Terminal_Serve();
    
    // button test
    if (digitalRead(BUTTON) > 0 && buttontest == 0) 
    {
      buttontest = 1;
      COMM_DBG.println("Button pressed");
    }
    else buttontest = 0;
      
  }


  // 10 ms loop
  if ((millis()-loop_10ms) > (uint32_t) 10 ) {  
    loop_10ms = millis();  

    const uint32_t valveTicks = valve_loop_ticks;
    if (valveTicks != lastValveTicks && !valve_loop_stalled) {
      lastValveTicks = valveTicks;
      IWatchdog.reload();
    }
    
    app_loop();  
    communication_loop();
    temperature_loop();
  }
}