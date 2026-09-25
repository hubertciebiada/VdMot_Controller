/**HEADER*******************************************************************
  project : VdMot Controller
  Comments: I2C bus (EEPROM) recovery
***************************************************************************/

#ifndef _I2C_BUS_H
	#define _I2C_BUS_H

// frees the bus from a slave that holds SDA low after a reset in the middle of a transfer;
// call before Wire.begin()
void i2c_bus_recover (void);
// stops Wire, recovers the bus and starts Wire again (before a retry of a failed EEPROM access)
void i2c_bus_restart (void);

#endif //_I2C_BUS_H
