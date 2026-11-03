// Aufnahme-Firmware fuer das Wake-Word-Training: pio run -e collect -t upload
// Gegenstueck: training/record.py
//
// Kein WLAN. Das Mikrofon wird genau so gelesen wie in main.cpp (gleicher Pegel, gleiches Format)
// und als Pakete ueber USB-Seriell an den PC geschickt. Der PC kann gleichzeitig Audio schicken,
// das ueber den Lautsprecher laeuft, um die eigene Sprachausgabe mit aufzunehmen.
//
// Paket (beide Richtungen): A5 5A | Typ (1) | Folgenummer (1) | Laenge (2, LE) | Nutzdaten
//   ESP -> PC: 0x01 Audio (int16 LE, 16 kHz mono), 0x02 Text (Status)
//   PC -> ESP: 0x10 Mikrofon an, 0x11 Mikrofon aus, 0x20 Audio abspielen (int16 LE, 16 kHz mono),
//              0x21 Lautstaerke in Prozent (uint16 LE)
// LED: gruen = Mikrofon laeuft, blau = Wiedergabe.

#include <Arduino.h>
#include <driver/i2s.h>

constexpr int MIC_SCK = 32, MIC_WS = 33, MIC_SD = 34;
constexpr int AMP_BCLK = 26, AMP_LRC = 25, AMP_DIN = 27;
constexpr int RGB_LED_PIN = 16;
constexpr i2s_port_t MIC_PORT = I2S_NUM_0;
constexpr i2s_port_t AMP_PORT = I2S_NUM_1;
constexpr uint32_t RATE = 16000;
constexpr int MIC_SHIFT = 14;  // wie main.cpp
constexpr uint32_t BAUD = 921600;

constexpr uint8_t T_AUDIO = 0x01, T_TEXT = 0x02;
constexpr uint8_t T_MIC_ON = 0x10, T_MIC_OFF = 0x11, T_PLAY = 0x20, T_VOLUME = 0x21;

static volatile bool micOn = false;
static volatile int volumePercent = 250;  // wie main.cpp
static volatile uint32_t playingUntil = 0;
static SemaphoreHandle_t txLock;

static void sendPacket(uint8_t type, const void* data, uint16_t len) {
  static uint8_t seq = 0;
  uint8_t header[6] = {0xA5, 0x5A, type, seq++, (uint8_t)(len & 0xFF), (uint8_t)(len >> 8)};
  xSemaphoreTake(txLock, portMAX_DELAY);
  Serial.write(header, sizeof(header));
  Serial.write((const uint8_t*)data, len);
  xSemaphoreGive(txLock);
}

static void sendText(const char* text) { sendPacket(T_TEXT, text, strlen(text)); }

static void setupMic() {
  i2s_config_t cfg = {};
  cfg.mode = (i2s_mode_t)(I2S_MODE_MASTER | I2S_MODE_RX);
  cfg.sample_rate = RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_32BIT;
  cfg.channel_format = I2S_CHANNEL_FMT_ONLY_RIGHT;  // L/R an GND, siehe main.cpp
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
  cfg.dma_buf_len = 512;
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

static void micTask(void*) {
  static int32_t raw[256];
  static int16_t pcm[256];
  for (;;) {
    size_t bytesRead = 0;
    i2s_read(MIC_PORT, raw, sizeof(raw), &bytesRead, portMAX_DELAY);
    if (!micOn) continue;
    size_t n = bytesRead / sizeof(int32_t);
    for (size_t i = 0; i < n; i++) pcm[i] = (int16_t)constrain(raw[i] >> MIC_SHIFT, -32768, 32767);
    sendPacket(T_AUDIO, pcm, n * sizeof(int16_t));
  }
}

static void play(const uint8_t* data, size_t len) {
  static int16_t stereo[512];
  size_t frames = 0;
  for (size_t i = 0; i + 1 < len; i += 2) {
    int16_t s = (int16_t)(data[i] | (data[i + 1] << 8));
    int16_t v = (int16_t)constrain((int32_t)s * volumePercent / 100, -32768, 32767);
    stereo[2 * frames] = v;
    stereo[2 * frames + 1] = v;
    if (++frames == 256 || i + 3 >= len) {
      size_t written;
      i2s_write(AMP_PORT, stereo, frames * 4, &written, portMAX_DELAY);
      frames = 0;
    }
  }
  playingUntil = millis() + 200;
}

static void ledTask(void*) {
  for (;;) {
    bool playing = (int32_t)(playingUntil - millis()) > 0;
    neopixelWrite(RGB_LED_PIN, 0, micOn && !playing ? 40 : 0, playing ? 60 : 0);
    vTaskDelay(pdMS_TO_TICKS(50));
  }
}

// Liest ein Paket vom PC; blockiert hoechstens kurz, wenn ein Paket angefangen ist.
static bool readExact(uint8_t* buf, size_t len) { return Serial.readBytes(buf, len) == len; }

static void handleCommands() {
  static uint8_t payload[4096];
  while (Serial.available() >= 6) {
    if (Serial.read() != 0xA5) continue;
    if (Serial.peek() != 0x5A) continue;
    Serial.read();
    uint8_t rest[4];
    if (!readExact(rest, 4)) return;
    uint8_t type = rest[0];
    uint16_t len = rest[2] | (rest[3] << 8);
    if (len > sizeof(payload) || !readExact(payload, len)) {
      sendText("Paket verworfen");
      return;
    }
    switch (type) {
      case T_MIC_ON:
        micOn = true;
        sendText("Mikrofon an");
        break;
      case T_MIC_OFF:
        micOn = false;
        sendText("Mikrofon aus");
        break;
      case T_PLAY:
        play(payload, len);
        break;
      case T_VOLUME:
        if (len >= 2) volumePercent = payload[0] | (payload[1] << 8);
        break;
    }
  }
}

void setup() {
  Serial.setRxBufferSize(32768);
  Serial.begin(BAUD);
  Serial.setTimeout(200);
  txLock = xSemaphoreCreateMutex();
  setupMic();
  setupAmp();
  xTaskCreatePinnedToCore(micTask, "mic", 4096, nullptr, 5, nullptr, 1);
  xTaskCreatePinnedToCore(ledTask, "led", 2048, nullptr, 1, nullptr, 0);
  sendText("collect bereit");
}

void loop() {
  handleCommands();
  delay(1);
}
