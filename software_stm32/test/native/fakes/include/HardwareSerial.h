// Fake of the core's HardwareSerial (framework-arduinoststm32 HardwareSerial.cpp, uart.h): the RX
// side is the core's ring of SERIAL_RX_BUFFER_SIZE bytes (one slot stays free, a byte that finds
// it full is dropped), filled through serial_t::rx_callback like the UART interrupt does, so the
// firmware may replace the callback (serial_t layout: uart and handle first, as in uart.h). The TX
// side is a log for the tests. fake_board.h has the test helpers (inject, takeTx, uartError).
// Glue-facing: no STL.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "Print.h"
#include "fake_cmsis.h"

#if !defined(SERIAL_TX_BUFFER_SIZE)
#define SERIAL_TX_BUFFER_SIZE 64
#endif
#if !defined(SERIAL_RX_BUFFER_SIZE)
#define SERIAL_RX_BUFFER_SIZE 64
#endif

#define SERIAL_8N1 0x06
#define SERIAL_8N2 0x0E
#define SERIAL_7E1 0x24
#define SERIAL_8E1 0x26
#define SERIAL_7E2 0x2C
#define SERIAL_8E2 0x2E
#define SERIAL_7O1 0x34
#define SERIAL_8O1 0x36
#define SERIAL_7O2 0x3C
#define SERIAL_8O2 0x3E

typedef struct serial_s serial_t;
struct serial_s {
  USART_TypeDef* uart;
  UART_HandleTypeDef handle;
  void (*rx_callback)(serial_t*);
  int (*tx_callback)(serial_t*);
  uint8_t recv;  // the received byte the RX callback takes (uart_getc)
  uint8_t* rx_buff;
  uint8_t* tx_buff;
  uint16_t rx_tail;
  uint16_t tx_head;
  volatile uint16_t rx_head;
  volatile uint16_t tx_tail;
};

class Stream : public Print {
 public:
  virtual int available() = 0;
  virtual int read() = 0;
  virtual int peek() = 0;
  void setTimeout(unsigned long timeoutMs) { timeoutMs_ = timeoutMs; }
  // reads until length bytes or the timeout: a missing byte costs the timeout in fake time
  size_t readBytes(char* buffer, size_t length);
  size_t readBytes(uint8_t* buffer, size_t length) { return readBytes(reinterpret_cast<char*>(buffer), length); }

 protected:
  unsigned long timeoutMs_ = 1000;
};

class HardwareSerial : public Stream {
 public:
  explicit HardwareSerial(void* peripheral);
  ~HardwareSerial() override;
  void begin(unsigned long baudRate) { begin(baudRate, SERIAL_8N1); }
  // like the core: (re)attaches HardwareSerial::_rx_complete_irq as the RX callback
  void begin(unsigned long baudRate, uint8_t frame);
  void end();
  int available() override;
  int peek() override;
  int read() override;
  int availableForWrite();
  virtual void flush();
  size_t write(uint8_t c) override;
  size_t write(unsigned long n) { return write(static_cast<uint8_t>(n)); }
  size_t write(long n) { return write(static_cast<uint8_t>(n)); }
  size_t write(unsigned int n) { return write(static_cast<uint8_t>(n)); }
  size_t write(int n) { return write(static_cast<uint8_t>(n)); }
  size_t write(const uint8_t* buffer, size_t size) override;
  using Print::write;
  operator bool() { return ready_; }

  void setRx(uint32_t pin) { rxPin = pin; }
  void setTx(uint32_t pin) { txPin = pin; }

  // the core's receive handler: stores serial_t::recv unless the ring is full
  static void _rx_complete_irq(serial_t* obj);
  UART_HandleTypeDef* getHandle() { return &serial_.handle; }

  // ---- fake state, read by the tests
  static constexpr size_t kTxLogSize = 16384;
  void* peripheral;
  unsigned long baud = 0;
  uint8_t config = 0;
  uint32_t rxPin = 0xFFFFFFFF;
  uint32_t txPin = 0xFFFFFFFF;
  unsigned begins = 0;
  unsigned ends = 0;
  unsigned flushes = 0;
  unsigned readCalls = 0;   // read() calls, also those that found no byte
  unsigned lostBytes = 0;   // bytes injected while the port was not begun
  char txLog[kTxLogSize];   // written bytes since the last fake::takeTx()
  size_t txLength = 0;
  size_t txDropped = 0;     // bytes beyond kTxLogSize
  uint32_t nextError = 0;   // HAL_UART_ERROR_* of the next injected byte (fake::uartError)

  void fakeReset();                 // fake::reset()
  void fakeReceive(uint8_t byte);   // one byte through the RX callback (fake::inject)

 private:
  unsigned char rxBuffer_[SERIAL_RX_BUFFER_SIZE];
  serial_t serial_;
  bool ready_ = false;
};

extern HardwareSerial Serial1;
extern HardwareSerial Serial2;
// defined by the firmware (src/terminal.cpp) or by glue/stubs/stub_terminal.cpp
extern HardwareSerial Serial6;
