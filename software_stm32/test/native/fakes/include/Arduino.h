// Fake of the STM32 Arduino core (framework-arduinoststm32) for the native glue tests: the API the
// STM glue uses, backed by fake::board (fake_board.h, the test side). Pins are numbered
// port * 16 + pin (PA0 = 0, PB0 = 16, PC13 = 45); digitalPinToPinName is the identity.
// Time is simulated: millis() and micros() read fake::board, delay() and delayMicroseconds() advance
// it, and every millisecond passed runs the timer interrupts and the valve sim (fake::advanceUs).
// Glue-facing: every glue translation unit includes it and is compiled for every mutant, so only C
// headers here (no STL).
#pragma once

#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "HardwareSerial.h"
#include "Print.h"
#include "WString.h"
#include "fake_cmsis.h"

typedef uint8_t byte;
typedef bool boolean;

#define LOW 0x0
#define HIGH 0x1
#define CHANGE 0x2
#define FALLING 0x3
#define RISING 0x4

#define INPUT 0x0
#define OUTPUT 0x1
#define INPUT_PULLUP 0x2
#define INPUT_PULLDOWN 0x3
#define INPUT_ANALOG 0x4
#define OUTPUT_OPEN_DRAIN 0x5

#define PA0 0
#define PA1 1
#define PA2 2
#define PA3 3
#define PA4 4
#define PA5 5
#define PA6 6
#define PA7 7
#define PA8 8
#define PA9 9
#define PA10 10
#define PA11 11
#define PA12 12
#define PA13 13
#define PA14 14
#define PA15 15
#define PB0 16
#define PB1 17
#define PB2 18
#define PB3 19
#define PB4 20
#define PB5 21
#define PB6 22
#define PB7 23
#define PB8 24
#define PB9 25
#define PB10 26
#define PB11 27
#define PB12 28
#define PB13 29
#define PB14 30
#define PB15 31
#define PC13 45
#define NUM_DIGITAL_PINS 48

typedef uint32_t PinName;
#define NC 0xFFFFFFFFU
#define digitalPinToPinName(p) (static_cast<PinName>(p))
#define digitalPinToInterrupt(p) (p)
#define STM_PORT(X) ((static_cast<uint32_t>(X) >> 4) & 0xF)
GPIO_TypeDef* set_GPIO_Port_Clock(uint32_t port);

uint32_t millis(void);
uint32_t micros(void);
void delay(uint32_t ms);
void delayMicroseconds(uint32_t us);

void pinMode(uint32_t pin, uint32_t mode);
void digitalWrite(uint32_t pin, uint32_t value);
int digitalRead(uint32_t pin);
uint32_t analogRead(uint32_t pin);
void analogReadResolution(int bits);

void attachInterrupt(uint32_t pin, void (*callback)(void), uint32_t mode);
void detachInterrupt(uint32_t pin);

#define interrupts() __enable_irq()
#define noInterrupts() __disable_irq()
