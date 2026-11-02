> This fork ships **VdMot Revamped** (unofficial), firmware in Rust since 2.2.0 — see [docs/revamped/README.md](docs/revamped/README.md) and [docs/rust/README.md](docs/rust/README.md)

## VdMot Revamped: this fork

VdMot Revamped is an unofficial, independently maintained firmware for the VdMot Controller: the
ESP32 (WT32-ETH01) and the STM32 BlackPill (F401, F411), in Rust since 2.2.0. It is a fork of
VdMot_Controller by Lenti84 and SurfGargano. The upstream project does not support it.

- **Download:** [latest release](https://github.com/hubertciebiada/VdMot_Controller/releases/latest)
- **Install, update, way back:** [docs/rust/INSTALL.md](docs/rust/INSTALL.md)
- **What it adds:**
  - valve targets in l/min from a measured flow curve per loop, optional and off by default;
  - a failsafe position when the regulator goes silent, on the STM, also with a dead ESP;
  - targets and calibrations that survive restarts;
  - a new dashboard with an event log and diagnostics;
  - a trial of every new ESP image with a way back, a checked STM flash from the dashboard;
  - MQTT and Home Assistant discovery, compatible with the legacy topics.
- **What it does not build:** RS485, Modbus, CAN and the DS2482 are not part of VdMot Revamped. The
  PI controller, the window logic and the alarms of the original ESP firmware are gone too.

The rest of this page is the description of the upstream project. It holds for the hardware; notes
mark what differs in VdMot Revamped.

# VdMot_Controller
This is a controller for HmIP-VdMot actuators.

It controls up to 12 HmIP-VdMot actuators or other DC motor valves and is based on custom hardware/software.

It's an alternative to the original HmIP-FALMOT-C12 hardware.
My intention was to create a cheap solution to operate my 18 valves (floor heating + radiators).
I wanted a pure offline system (no cloud connection) and to be free of the constraint to use further components like CCU2/CCU3.

**Hardware for VdMot_Controller is available via mail. Please write to vdmot@storbecknet.de. **

![-](./hardware/c2-sample_pcb_soldered.jpg "C2 sample PCB with soldered components")

## Features:
- controls up to 12 HmIP-VdMot actuators or other DC motor valves
- learns valve movement by counting motor revolutions based on Back EMF
  - so positioning accuracy should be (very) good
- valve current is evaluated and can be monitored
- interfaces
  - actual: MQTT, JSON
  - optional: MODBUS-RTU via RS485, CAN via external MCP2515 (not built in VdMot Revamped)
- integrated 1-wire master
  - a lot of additional temperature sensors like DS18B20 could be connected
  - sensor values can be linked to a valve for combined data evaluation
  - usefull for hydraulic balancing ("Hydraulischer Abgleich")
  
## Status
- b-sample hardware is working
- c-sample hardware is working
- c2-sample hardware finished and working
- valve learning, opening and closing is working
- connection to [FHEM](https://fhem.de/) or [IOBroker] established via MQTT
- read and set values via JSON
- first productive test passed
- tests of 2 different actuators done, see [system/actuators.md](./system/actuators.md)
- one wire sensors working
  - sensors can be coupled to a valve
  - therefore the sensor adresses are stored and assigned at startup
- eeprom working
- STM32 can be flashed by ESP32 via WebUI without additional hardware
- simple test of RS485 interface with modbus-rtu master sucessfully performed
- simple test of CAN interface via MCP2515 sucessfully performed
- replaced relay with fet transistors

## Hardware
- find description of hardware here [hardware/hardware.md](./hardware/hardware.md)

## Software
For details please see [software.md](./software.md) (upstream firmware; VdMot Revamped is in Rust, see above)
- written in C / C++
- uses great arduino libraries
- developed using PlatformIO
- STM32 BlackPill
  - controls the valves / dc-motors
    - endstop by real current measurement
    - counting motor revolutions by back-EMF
    - state detection
  - controlls 1-wire devices via GPIO
    - optional via DS2482-100 1-wire bus master ic (not built in VdMot Revamped)
  - 
- ESP32 / WT32-ETH01
  - communication with MQTT broker or JSON interface
  - visualize system status
    - valve state
    - temperature sensors
    - optional: integrated control algorithm 
  - OTA software update (of ESP32 and STM32)

Latest Software can be found here :
https://github.com/SurfGargano/VdMot_Controller/releases/latest

That link is the official upstream firmware. VdMot Revamped is not there, its releases are at
https://github.com/hubertciebiada/VdMot_Controller/releases/latest

Hardware can be found here:
**Hardware for VdMot_Controller is available via mail. Please write to vdmot@storbecknet.de. **

## License
This project is licensed under the terms of the GNU General Public License v3.0 license.
