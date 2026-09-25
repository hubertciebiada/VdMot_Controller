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
#include "otasupport.h"
#include "boot_jump.h"

    
uint8_t bootstate = 0;

void BootSetup(void) {

  // LED
  pinMode(LED, OUTPUT);
  digitalWrite(LED, LOW);

  // UART to ESP32
  Serial1.setRx(PA10);			    //STM32F401 blackpill USART1 RX PA10
  Serial1.setTx(PA9);		        //STM32F401 blackpill USART1 TX PA9
  Serial1.begin(115200, SERIAL_8E1);
  while(!Serial1);
  delay(10);

  while (Serial1.available()) Serial1.read();

}


void BootLoop(void) {

  static unsigned int timer = 0;
  static int ledtimer = 0;
  static int state = 0;
  unsigned char buffer2[10];

  if (state == 0) {
    timer++;
    if(timer > 3000) state = 2;
  
    if (Serial1.available() >= 8) {        
      Serial1.readBytes(buffer2, 8);
      if(memcmp("DEADBEEF",&buffer2[0],8) == 0) {
        state = 1;
        digitalWrite(LED, LOW);
      }
    }

    if (ledtimer > 100) {
      digitalWrite(LED, !digitalRead(LED));   // toggle LED
      ledtimer=0;
    }
    else ledtimer++;

    delay(1);
  }
  else if(state == 1) {
      digitalWrite(LED, HIGH);
      delay(10);        // delay for init
      Serial1.println("BEEFIT");
      Serial1.flush();
      delay(200);        // wait for send
      JumpToBootloader();
      
  }
  else if (state == 2) {
      bootstate = 1;
  }

}
