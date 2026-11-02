// Hardware-Test ohne WLAN: pio run -e hwtest -t upload && pio device monitor -e hwtest
//
// Alle 8 Sekunden: Ruhepegel des Mikrofons messen, dann einen Ton ueber den Lautsprecher spielen
// und gleichzeitig das Mikrofon messen. Hoert das Mikrofon den Ton, funktionieren beide.
// Dazwischen wird jeder Tastendruck gemeldet. Pins wie in main.cpp.

#include <Arduino.h>
#include <driver/i2s.h>
#include <math.h>

#include <vector>

constexpr int MIC_SCK = 32, MIC_WS = 33, MIC_SD = 34;
constexpr int AMP_BCLK = 26, AMP_LRC = 25, AMP_DIN = 27;
constexpr int BUTTON = 5;
constexpr int LED = 2;
constexpr int MIC_SHIFT = 14;
constexpr uint32_t RATE = 16000;

constexpr i2s_port_t MIC_PORT = I2S_NUM_0;
constexpr i2s_port_t AMP_PORT = I2S_NUM_1;

static void setupMic(i2s_channel_fmt_t fmt) {
  i2s_driver_uninstall(MIC_PORT);
  i2s_config_t cfg = {};
  cfg.mode = (i2s_mode_t)(I2S_MODE_MASTER | I2S_MODE_RX);
  cfg.sample_rate = RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_32BIT;
  cfg.channel_format = fmt;
  cfg.communication_format = I2S_COMM_FORMAT_STAND_I2S;
  cfg.intr_alloc_flags = ESP_INTR_FLAG_LEVEL1;
  cfg.dma_buf_count = 8;
  cfg.dma_buf_len = 256;
  ESP_ERROR_CHECK(i2s_driver_install(MIC_PORT, &cfg, 0, nullptr));
  i2s_pin_config_t pins = {};
  pins.mck_io_num = I2S_PIN_NO_CHANGE;
  pins.bck_io_num = MIC_SCK;
  pins.ws_io_num = MIC_WS;
  pins.data_out_num = I2S_PIN_NO_CHANGE;
  pins.data_in_num = MIC_SD;
  ESP_ERROR_CHECK(i2s_set_pin(MIC_PORT, &pins));
}

static void setupAmp() {
  i2s_config_t cfg = {};
  cfg.mode = (i2s_mode_t)(I2S_MODE_MASTER | I2S_MODE_TX);
  cfg.sample_rate = RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_16BIT;
  cfg.channel_format = I2S_CHANNEL_FMT_RIGHT_LEFT;
  cfg.communication_format = I2S_COMM_FORMAT_STAND_I2S;
  cfg.intr_alloc_flags = ESP_INTR_FLAG_LEVEL1;
  cfg.dma_buf_count = 8;
  cfg.dma_buf_len = 256;
  cfg.tx_desc_auto_clear = true;
  ESP_ERROR_CHECK(i2s_driver_install(AMP_PORT, &cfg, 0, nullptr));
  i2s_pin_config_t pins = {};
  pins.mck_io_num = I2S_PIN_NO_CHANGE;
  pins.bck_io_num = AMP_BCLK;
  pins.ws_io_num = AMP_LRC;
  pins.data_out_num = AMP_DIN;
  pins.data_in_num = I2S_PIN_NO_CHANGE;
  ESP_ERROR_CHECK(i2s_set_pin(AMP_PORT, &pins));
}

struct Level {
  double rms = 0;
  int32_t peak = 0;
  int32_t minRaw = INT32_MAX, maxRaw = INT32_MIN;
  size_t samples = 0;
};

static int lastButton = HIGH;

static void pollButton() {
  int b = digitalRead(BUTTON);
  if (b != lastButton) {
    delay(20);
    b = digitalRead(BUTTON);
    if (b != lastButton) {
      lastButton = b;
      Serial.println(b == LOW ? "TASTER gedrueckt" : "TASTER losgelassen");
      digitalWrite(LED, b == LOW);
    }
  }
}

// Misst `ms` Millisekunden; spielt dabei optional einen Sinuston.
static Level measure(uint32_t ms, float toneHz, float toneAmp) {
  static int32_t raw[256];
  static int16_t out[512];
  Level lv;
  double sum = 0, sumSq = 0;
  int32_t lo = INT32_MAX, hi = INT32_MIN;
  float phase = 0;
  // Erste Puffer verwerfen (Einschwingen, alte DMA-Daten)
  for (int i = 0; i < 4; i++) {
    size_t r;
    i2s_read(MIC_PORT, raw, sizeof(raw), &r, portMAX_DELAY);
  }
  uint32_t start = millis();
  while (millis() - start < ms) {
    if (toneHz > 0) {
      for (int i = 0; i < 256; i++) {
        int16_t s = (int16_t)(sinf(phase) * toneAmp * 32767);
        phase += 2 * PI * toneHz / RATE;
        if (phase > 2 * PI) phase -= 2 * PI;
        out[2 * i] = s;
        out[2 * i + 1] = s;
      }
      size_t w;
      i2s_write(AMP_PORT, out, sizeof(out), &w, portMAX_DELAY);
    }
    size_t r = 0;
    i2s_read(MIC_PORT, raw, sizeof(raw), &r, portMAX_DELAY);
    for (size_t i = 0; i < r / 4; i++) {
      lv.minRaw = min(lv.minRaw, raw[i]);
      lv.maxRaw = max(lv.maxRaw, raw[i]);
      int32_t s = raw[i] >> MIC_SHIFT;
      sum += s;
      sumSq += (double)s * s;
      lo = min(lo, s);
      hi = max(hi, s);
      lv.samples++;
    }
    pollButton();
  }
  i2s_zero_dma_buffer(AMP_PORT);
  if (lv.samples) {
    double mean = sum / lv.samples;  // Gleichanteil des Mikrofons abziehen
    lv.rms = sqrt(max(0.0, sumSq / lv.samples - mean * mean));
    lv.peak = (hi - lo) / 2;
  }
  return lv;
}

void setup() {
  Serial.begin(115200);
  delay(500);
  pinMode(BUTTON, INPUT_PULLUP);
  pinMode(LED, OUTPUT);
  lastButton = digitalRead(BUTTON);
  Serial.println();
  Serial.println("=== HWTEST Sprachassistent ===");
  Serial.printf("Chip %s Rev %d, %d Kerne, Heap frei %u, PSRAM %u\n", ESP.getChipModel(),
                ESP.getChipRevision(), ESP.getChipCores(), ESP.getFreeHeap(), ESP.getPsramSize());
  Serial.printf("Taster an GPIO %d: %s\n", BUTTON, lastButton == LOW ? "GEDRUECKT (oder Kurzschluss nach GND)" : "offen");
  setupAmp();
}

static void testChannel(i2s_channel_fmt_t fmt, const char* name) {
  setupMic(fmt);
  measure(800, 0, 0);  // Einschwingen nach dem Start des Mikrofons (Gleichanteil driftet)
  Level idle = measure(1000, 0, 0);
  Level tone = measure(1500, 1000, 0.25f);
  double ratio = idle.rms > 0 ? tone.rms / idle.rms : 0;
  Serial.printf("[%s] MIC ruhe rms=%.0f raw=[%08x..%08x] | mit Ton rms=%.0f peak=%d | Faktor %.1f -> %s\n", name,
                idle.rms, (unsigned)idle.minRaw, (unsigned)idle.maxRaw, tone.rms, tone.peak, ratio,
                idle.minRaw == idle.maxRaw ? "KEINE DATEN" : ratio > 3 ? "LOOPBACK OK" : "Daten, Ton kaum gehoert");
}

void loop() {
  testChannel(I2S_CHANNEL_FMT_ONLY_LEFT, "ONLY_LEFT ");
  testChannel(I2S_CHANNEL_FMT_ONLY_RIGHT, "ONLY_RIGHT");
  uint32_t until = millis() + 3000;
  while (millis() < until) {
    pollButton();
    delay(10);
  }
}
