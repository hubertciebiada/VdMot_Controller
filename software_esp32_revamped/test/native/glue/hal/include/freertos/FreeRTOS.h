// Fake FreeRTOS (ESP-IDF 4.4 port) for the ESP glue tests: tasks, delays, queues, mutexes and
// critical sections in one header (freertos/task.h, queue.h and semphr.h include this one).
//
// Single-threaded model (fakes::rtos()):
//   - xTaskCreatePinnedToCore() records the task; the fake never runs it (a test calls the task
//     function itself and ends its loop with rtos().stopAfterYields(n) -> fakes::YieldLimit);
//   - vTaskDelay() advances the fake clock by the ticks (1 kHz tick, like CONFIG_FREERTOS_HZ=1000);
//   - a queue is a ring buffer in the caller's static storage (ASan sees a too-small buffer);
//   - misuse that deadlocks or corrupts on the target counts as an RTOS violation, which fails the
//     test case: a second take of a held mutex, a give of a mutex nobody holds, an exit of a
//     critical section that was not entered, a blocking call inside a critical section, a
//     critical section still entered at the end of a case.
#pragma once

#include <stddef.h>
#include <stdint.h>

typedef int BaseType_t;
typedef unsigned int UBaseType_t;
typedef uint32_t TickType_t;

#define pdFALSE ((BaseType_t)0)
#define pdTRUE ((BaseType_t)1)
#define pdPASS (pdTRUE)
#define pdFAIL (pdFALSE)
#define errQUEUE_EMPTY ((BaseType_t)0)
#define errQUEUE_FULL ((BaseType_t)0)
#define portMAX_DELAY ((TickType_t)0xffffffffUL)
#define configTICK_RATE_HZ 1000
#define configMAX_PRIORITIES 25
#define portTICK_PERIOD_MS ((TickType_t)1000 / configTICK_RATE_HZ)
#define pdMS_TO_TICKS(xTimeInMs) \
  ((TickType_t)(((TickType_t)(xTimeInMs) * (TickType_t)configTICK_RATE_HZ) / (TickType_t)1000U))
#define tskNO_AFFINITY ((BaseType_t)0x7FFFFFFF)
#define tskIDLE_PRIORITY ((UBaseType_t)0U)

// ---------------------------------------------------------------- tasks

struct tskTaskControlBlock;
typedef struct tskTaskControlBlock* TaskHandle_t;
typedef void (*TaskFunction_t)(void*);

// Records the task (fakes::rtos().tasks) and returns fakes::rtos().createResult; the handle is a
// unique fake value.
BaseType_t xTaskCreatePinnedToCore(TaskFunction_t pvTaskCode, const char* const pcName,
                                   const uint32_t usStackDepth, void* const pvParameters,
                                   UBaseType_t uxPriority, TaskHandle_t* const pvCreatedTask,
                                   const BaseType_t xCoreID);
BaseType_t xTaskCreate(TaskFunction_t pvTaskCode, const char* const pcName,
                       const uint32_t usStackDepth, void* const pvParameters,
                       UBaseType_t uxPriority, TaskHandle_t* const pvCreatedTask);
// Advances the fake clock; throws fakes::YieldLimit when the budget of stopAfterYields() is used.
void vTaskDelay(const TickType_t xTicksToDelay);
// nullptr (the calling task) does not return: records and throws fakes::TaskDeleted.
void vTaskDelete(TaskHandle_t xTaskToDelete);
// Scripted per task name (fakes::rtos().stackHighWater, bytes like ESP-IDF); 0 when unknown.
UBaseType_t uxTaskGetStackHighWaterMark(TaskHandle_t xTask);
// A recorded task by name, else fakes::rtos().extraHandles[name], else nullptr.
TaskHandle_t xTaskGetHandle(const char* pcNameToQuery);
TaskHandle_t xTaskGetCurrentTaskHandle(void);
TickType_t xTaskGetTickCount(void);

// ---------------------------------------------------------------- queues and semaphores

// The fake keeps its bookkeeping in the static control block of the caller.
typedef struct xSTATIC_QUEUE {
  uint32_t magic;
  uint8_t kind;  // 1 queue, 2 mutex, 3 binary semaphore
  uint8_t* storage;
  UBaseType_t length;
  UBaseType_t itemSize;
  UBaseType_t head;
  UBaseType_t count;
  uint8_t ownsStorage;
} StaticQueue_t;
typedef StaticQueue_t StaticSemaphore_t;
typedef StaticQueue_t* QueueHandle_t;
typedef QueueHandle_t SemaphoreHandle_t;

QueueHandle_t xQueueCreateStatic(UBaseType_t uxQueueLength, UBaseType_t uxItemSize,
                                 uint8_t* pucQueueStorageBuffer, StaticQueue_t* pxQueueBuffer);
QueueHandle_t xQueueCreate(UBaseType_t uxQueueLength, UBaseType_t uxItemSize);
BaseType_t xQueueSend(QueueHandle_t xQueue, const void* pvItemToQueue, TickType_t xTicksToWait);
BaseType_t xQueueSendToBack(QueueHandle_t xQueue, const void* pvItemToQueue,
                            TickType_t xTicksToWait);
BaseType_t xQueueSendToFront(QueueHandle_t xQueue, const void* pvItemToQueue,
                             TickType_t xTicksToWait);
BaseType_t xQueueReceive(QueueHandle_t xQueue, void* pvBuffer, TickType_t xTicksToWait);
BaseType_t xQueuePeek(QueueHandle_t xQueue, void* pvBuffer, TickType_t xTicksToWait);
UBaseType_t uxQueueMessagesWaiting(QueueHandle_t xQueue);
UBaseType_t uxQueueSpacesAvailable(QueueHandle_t xQueue);
BaseType_t xQueueReset(QueueHandle_t xQueue);
void vQueueDelete(QueueHandle_t xQueue);

SemaphoreHandle_t xSemaphoreCreateMutexStatic(StaticSemaphore_t* pxMutexBuffer);
SemaphoreHandle_t xSemaphoreCreateMutex(void);
SemaphoreHandle_t xSemaphoreCreateBinaryStatic(StaticSemaphore_t* pxSemaphoreBuffer);
SemaphoreHandle_t xSemaphoreCreateBinary(void);
// A mutex that is already held cannot be taken (single-threaded: the caller would wait forever):
// an RTOS violation, pdFALSE. A binary semaphore that is not given returns pdFALSE (a timeout).
BaseType_t xSemaphoreTake(SemaphoreHandle_t xSemaphore, TickType_t xBlockTime);
BaseType_t xSemaphoreGive(SemaphoreHandle_t xSemaphore);
void vSemaphoreDelete(SemaphoreHandle_t xSemaphore);

// ---------------------------------------------------------------- critical sections

typedef struct {
  volatile uint32_t owner;
  volatile uint32_t count;
} portMUX_TYPE;
#define portMUX_FREE_VAL 0xB33FFFFFu
#define portMUX_INITIALIZER_UNLOCKED {portMUX_FREE_VAL, 0}

void vPortEnterCritical(portMUX_TYPE* mux);
void vPortExitCritical(portMUX_TYPE* mux);
void vPortCPUInitializeMutex(portMUX_TYPE* mux);
#define portENTER_CRITICAL(mux) vPortEnterCritical(mux)
#define portEXIT_CRITICAL(mux) vPortExitCritical(mux)
#define portENTER_CRITICAL_ISR(mux) vPortEnterCritical(mux)
#define portEXIT_CRITICAL_ISR(mux) vPortExitCritical(mux)
#define taskENTER_CRITICAL(mux) vPortEnterCritical(mux)
#define taskEXIT_CRITICAL(mux) vPortExitCritical(mux)
#define taskENTER_CRITICAL_ISR(mux) vPortEnterCritical(mux)
#define taskEXIT_CRITICAL_ISR(mux) vPortExitCritical(mux)
