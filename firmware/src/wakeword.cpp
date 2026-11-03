// Wake-Word-Erkennung: Frontend und Streaming-Modell wie ESPHome micro_wake_word, siehe docs/wakeword.md.

#include "wakeword.h"

#if USE_WAKEWORD

#include <Arduino.h>
#include <esp_heap_caps.h>
#include <frontend.h>
#include <frontend_util.h>

#include "tensorflow/lite/micro/micro_allocator.h"
#include "tensorflow/lite/micro/micro_interpreter.h"
#include "tensorflow/lite/micro/micro_mutable_op_resolver.h"
#include "tensorflow/lite/micro/micro_resource_variable.h"
#include "tensorflow/lite/schema/schema_generated.h"

#include "wakeword_model.h"

// Muss zum Training passen (microWakeWord, pymicro-features)
constexpr int FEATURE_SIZE = 40;
constexpr size_t VAR_ARENA_SIZE = 1024;
// Nach einer Erkennung so viele Inferenzen (je 30 ms) ignorieren
constexpr int COOLDOWN_INFERENCES = 30;

static FrontendConfig frontendConfig;
static FrontendState frontendState;
static tflite::MicroMutableOpResolver<20> resolver;
static uint8_t* varArena = nullptr;
static tflite::MicroAllocator* varAllocator = nullptr;
static tflite::MicroResourceVariables* resourceVars = nullptr;
static tflite::MicroInterpreter* interpreter = nullptr;
static uint8_t* arena = nullptr;
static size_t arenaSize = WAKEWORD_ARENA;

static int8_t features[FEATURE_SIZE];
static uint8_t recent[WAKEWORD_WINDOW];
static int recentIndex = 0;
static int strideStep = 0;
static int cooldown = 0;

static bool registerOps() {
  return resolver.AddCallOnce() == kTfLiteOk && resolver.AddVarHandle() == kTfLiteOk &&
         resolver.AddReshape() == kTfLiteOk && resolver.AddReadVariable() == kTfLiteOk &&
         resolver.AddStridedSlice() == kTfLiteOk && resolver.AddConcatenation() == kTfLiteOk &&
         resolver.AddAssignVariable() == kTfLiteOk && resolver.AddConv2D() == kTfLiteOk &&
         resolver.AddMul() == kTfLiteOk && resolver.AddAdd() == kTfLiteOk && resolver.AddMean() == kTfLiteOk &&
         resolver.AddFullyConnected() == kTfLiteOk && resolver.AddLogistic() == kTfLiteOk &&
         resolver.AddQuantize() == kTfLiteOk && resolver.AddDepthwiseConv2D() == kTfLiteOk &&
         resolver.AddAveragePool2D() == kTfLiteOk && resolver.AddMaxPool2D() == kTfLiteOk &&
         resolver.AddPad() == kTfLiteOk && resolver.AddPack() == kTfLiteOk && resolver.AddSplitV() == kTfLiteOk;
}

static bool setupFrontend() {
  frontendConfig.window.size_ms = 30;
  frontendConfig.window.step_size_ms = 10;
  frontendConfig.filterbank.num_channels = FEATURE_SIZE;
  frontendConfig.filterbank.lower_band_limit = 125.0;
  frontendConfig.filterbank.upper_band_limit = 7500.0;
  frontendConfig.noise_reduction.smoothing_bits = 10;
  frontendConfig.noise_reduction.even_smoothing = 0.025;
  frontendConfig.noise_reduction.odd_smoothing = 0.06;
  frontendConfig.noise_reduction.min_signal_remaining = 0.05;
  frontendConfig.pcan_gain_control.enable_pcan = 1;
  frontendConfig.pcan_gain_control.strength = 0.95;
  frontendConfig.pcan_gain_control.offset = 80.0;
  frontendConfig.pcan_gain_control.gain_bits = 21;
  frontendConfig.log_scale.enable_log = 1;
  frontendConfig.log_scale.scale_shift = 6;
  return FrontendPopulateState(&frontendConfig, &frontendState, 16000);
}

bool wakewordLoad() {
  if (interpreter) return true;
  // Die Variablen-Arena muss bei jedem Laden frisch sein, sonst haengen alte Zustaende drin
  varAllocator = tflite::MicroAllocator::Create(varArena, VAR_ARENA_SIZE);
  resourceVars = tflite::MicroResourceVariables::Create(varAllocator, 20);
  arena = (uint8_t*)heap_caps_aligned_alloc(16, arenaSize, MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT);
  if (!arena) {
    Serial.println("Wake Word: zu wenig Speicher fuer die Arena");
    return false;
  }
  interpreter = new tflite::MicroInterpreter(tflite::GetModel(WAKEWORD_MODEL), resolver, arena, arenaSize, resourceVars);
  if (interpreter->AllocateTensors() != kTfLiteOk) {
    Serial.println("Wake Word: AllocateTensors fehlgeschlagen");
    wakewordUnload();
    return false;
  }
  FrontendReset(&frontendState);
  memset(recent, 0, sizeof(recent));
  strideStep = 0;
  cooldown = 0;
  return true;
}

void wakewordUnload() {
  delete interpreter;
  interpreter = nullptr;
  heap_caps_free(arena);
  arena = nullptr;
}

bool wakewordBegin() {
  if (tflite::GetModel(WAKEWORD_MODEL)->version() != TFLITE_SCHEMA_VERSION) {
    Serial.println("Wake Word: Modell-Schema passt nicht");
    return false;
  }
  varArena = (uint8_t*)heap_caps_malloc(VAR_ARENA_SIZE, MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT);
  if (!varArena || !registerOps() || !setupFrontend() || !wakewordLoad()) return false;
  // Arena auf den tatsaechlichen Bedarf verkleinern (spart einige KB fuer TLS). Der Speicherplaner
  // braucht etwas Luft ueber arena_used_bytes, daher in Schritten probieren.
  size_t used = interpreter->arena_used_bytes();
  for (size_t extra : {1024, 2048, 4096}) {
    size_t size = (used + extra + 15) & ~15;
    if (size >= WAKEWORD_ARENA) break;
    wakewordUnload();
    arenaSize = size;
    if (wakewordLoad()) break;
    arenaSize = WAKEWORD_ARENA;
  }
  if (!interpreter && !wakewordLoad()) return false;
  Serial.printf("Wake Word \"%s\" bereit (Arena %u Bytes, Schwelle %.2f)\n", WAKEWORD_NAME, arenaSize,
                WAKEWORD_CUTOFF / 255.0f);
  return true;
}

const char* wakewordName() { return WAKEWORD_NAME; }

// Ein Feature-Frame ins Modell; true bei Erkennung.
static bool step() {
  TfLiteTensor* input = interpreter->input(0);
  uint8_t stride = input->dims->data[1];
  memcpy(input->data.int8 + FEATURE_SIZE * strideStep, features, FEATURE_SIZE);
  if (++strideStep < stride) return false;
  strideStep = 0;
  if (interpreter->Invoke() != kTfLiteOk) return false;
  recentIndex = (recentIndex + 1) % WAKEWORD_WINDOW;
  recent[recentIndex] = interpreter->output(0)->data.uint8[0];
  if (cooldown > 0) {
    cooldown--;
    return false;
  }
  uint32_t sum = 0;
  for (int i = 0; i < WAKEWORD_WINDOW; i++) sum += recent[i];
  if (sum / WAKEWORD_WINDOW <= WAKEWORD_CUTOFF) return false;
  Serial.printf("Wake Word erkannt (Mittel %.2f)\n", sum / WAKEWORD_WINDOW / 255.0f);
  memset(recent, 0, sizeof(recent));
  cooldown = COOLDOWN_INFERENCES;
  return true;
}

bool wakewordFeed(const int16_t* pcm, size_t n) {
  if (!interpreter) return false;
  bool detected = false;
  size_t offset = 0;
  while (offset < n) {
    size_t processed = 0;
    FrontendOutput fo = FrontendProcessSamples(&frontendState, pcm + offset, n - offset, &processed);
    offset += processed;
    if (fo.size == 0) continue;
    for (size_t i = 0; i < fo.size; i++) {
      // Skalierung wie ESPHome: (Wert / 25.6 / 26) * 256 - 128
      int32_t v = ((fo.values[i] * 256) + 333) / 666 - 128;
      features[i] = (int8_t)constrain(v, -128, 127);
    }
    if (step()) detected = true;
  }
  return detected;
}

#else

bool wakewordBegin() { return false; }
void wakewordUnload() {}
bool wakewordLoad() { return false; }
bool wakewordFeed(const int16_t*, size_t) { return false; }
const char* wakewordName() { return ""; }

#endif
