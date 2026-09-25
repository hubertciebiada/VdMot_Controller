// Fake of the CMSIS and STM32Cube HAL parts the STM glue uses (STM32F401, framework-arduinoststm32):
// registers are plain RAM, the interrupt mask is a variable, a system reset throws.
// Glue-facing: C headers only (every glue translation unit is compiled for every mutant).
#pragma once

#include <stdint.h>

#define RCC_CSR_RMVF (1UL << 24)
#define RCC_CSR_BORRSTF (1UL << 25)
#define RCC_CSR_PINRSTF (1UL << 26)
#define RCC_CSR_PORRSTF (1UL << 27)
#define RCC_CSR_SFTRSTF (1UL << 28)
#define RCC_CSR_IWDGRSTF (1UL << 29)
#define RCC_CSR_WWDGRSTF (1UL << 30)
#define RCC_CSR_LPWRRSTF (1UL << 31)
#define RCC_CSR_RESET_FLAGS 0xFE000000UL

// RCC_CSR (reference manual RM0368): the reset flags accumulate over resets until the firmware writes
// RMVF, which clears them (and reads back as 0). The runner hooks (glue/runner_hooks.cpp) add the
// flags of every boot.
struct fake_csr_register {
  uint32_t value;
  uint32_t clears;  // RMVF writes
  operator uint32_t() const { return value; }
  fake_csr_register& operator=(uint32_t v) {
    value = v;
    clearOnRmvf();
    return *this;
  }
  fake_csr_register& operator|=(uint32_t bits) { return *this = value | bits; }
  fake_csr_register& operator&=(uint32_t bits) { return *this = value & bits; }

 private:
  void clearOnRmvf() {
    if (value & RCC_CSR_RMVF) {
      value &= ~(RCC_CSR_RESET_FLAGS | RCC_CSR_RMVF);
      clears++;
    }
  }
};

typedef struct {
  fake_csr_register CSR;
} RCC_TypeDef;
extern RCC_TypeDef fake_RCC;
#define RCC (&fake_RCC)

// Peripherals are told apart by their address only.
typedef struct {
  uint32_t index;
} TIM_TypeDef;
extern TIM_TypeDef fake_TIM1, fake_TIM2, fake_TIM3;
#define TIM1 (&fake_TIM1)
#define TIM2 (&fake_TIM2)
#define TIM3 (&fake_TIM3)

typedef struct {
  uint32_t index;
} USART_TypeDef;
extern USART_TypeDef fake_USART1, fake_USART2, fake_USART6;
#define USART1 (&fake_USART1)
#define USART2 (&fake_USART2)
#define USART6 (&fake_USART6)

typedef struct {
  uint32_t index;
} GPIO_TypeDef;

typedef struct {
  USART_TypeDef* Instance;
  volatile uint32_t ErrorCode;  // HAL_UART_ERROR_*, set before the receive callback runs
} UART_HandleTypeDef;

#define HAL_UART_ERROR_NONE 0x00000000U
#define HAL_UART_ERROR_PE 0x00000001U
#define HAL_UART_ERROR_NE 0x00000002U
#define HAL_UART_ERROR_FE 0x00000004U
#define HAL_UART_ERROR_ORE 0x00000008U

// PRIMASK: 1 = interrupts disabled. Timers, EXTI and the valve sim do not interrupt while it is set.
uint32_t __get_PRIMASK(void);
void __set_PRIMASK(uint32_t priMask);
void __disable_irq(void);
void __enable_irq(void);

// Does not return on the device: throws fake::SystemReset (glue::run() turns it into a software reset).
void HAL_NVIC_SystemReset(void);
// DBGMCU_IDCODE device id, fake::board.devId (0x423: STM32F401xB/C)
uint32_t HAL_GetDEVID(void);
