// Sprachassistent-Client: Taster halten und sprechen, loslassen, Antwort hoeren.
//
// Waehrend der Taster gedrueckt ist, wird das Mikrofon als 16 kHz / 16 Bit mono an POST /v1/talk
// gestreamt (chunked). Der Server antwortet mit rohem PCM (16 Bit mono, Rate im Header
// X-Sample-Rate), das direkt ueber den MAX98357A abgespielt wird.

#include <Arduino.h>
#include <WiFi.h>
#include <WiFiClientSecure.h>
#include <driver/i2s.h>
#include <freertos/stream_buffer.h>

#include "secrets.h"

// INMP441: VDD 3.3V, GND, L/R an GND (linker Kanal)
constexpr int MIC_SCK = 32;
constexpr int MIC_WS = 33;
constexpr int MIC_SD = 34;
// MAX98357A: VIN 5V, GND, SD und GAIN offen
constexpr int AMP_BCLK = 26;
constexpr int AMP_LRC = 25;
constexpr int AMP_DIN = 27;
// Taster gegen GND
constexpr int BUTTON = 13;
constexpr int LED = 2;

constexpr i2s_port_t MIC_PORT = I2S_NUM_0;
constexpr i2s_port_t AMP_PORT = I2S_NUM_1;
constexpr uint32_t MIC_RATE = 16000;
// Der INMP441 liefert 24 Bit im 32-Bit-Wort. >> 16 waere Originalpegel, >> 14 verstaerkt 4-fach.
constexpr int MIC_SHIFT = 14;
constexpr int VOLUME_PERCENT = 70;
constexpr uint32_t MAX_RECORD_MS = 20000;
// Puffert ca. 1,5 s Audio, waehrend die Verbindung (TLS-Handshake) aufgebaut wird.
constexpr size_t AUDIO_BUFFER_BYTES = 48 * 1024;

static StreamBufferHandle_t audioBuffer;
static volatile bool recording = false;

#if USE_TLS
static WiFiClientSecure client;
#else
static WiFiClient client;
#endif

// ---------- I2S ----------

static void setupMic() {
  i2s_config_t cfg = {};
  cfg.mode = (i2s_mode_t)(I2S_MODE_MASTER | I2S_MODE_RX);
  cfg.sample_rate = MIC_RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_32BIT;
  // Bleibt es stumm, hier I2S_CHANNEL_FMT_ONLY_RIGHT probieren (je nach ESP32-Revision vertauscht).
  cfg.channel_format = I2S_CHANNEL_FMT_ONLY_LEFT;
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
  cfg.sample_rate = 22050;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_16BIT;
  cfg.channel_format = I2S_CHANNEL_FMT_RIGHT_LEFT;
  cfg.communication_format = I2S_COMM_FORMAT_STAND_I2S;
  cfg.intr_alloc_flags = ESP_INTR_FLAG_LEVEL1;
  cfg.dma_buf_count = 8;
  cfg.dma_buf_len = 512;
  cfg.tx_desc_auto_clear = true;  // Stille statt Knacken, wenn Daten fehlen
  ESP_ERROR_CHECK(i2s_driver_install(AMP_PORT, &cfg, 0, nullptr));

  i2s_pin_config_t pins = {};
  pins.mck_io_num = I2S_PIN_NO_CHANGE;
  pins.bck_io_num = AMP_BCLK;
  pins.ws_io_num = AMP_LRC;
  pins.data_out_num = AMP_DIN;
  pins.data_in_num = I2S_PIN_NO_CHANGE;
  ESP_ERROR_CHECK(i2s_set_pin(AMP_PORT, &pins));
}

// Liest das Mikrofon dauerhaft; nur waehrend der Aufnahme landet das Audio im Puffer.
static void micTask(void*) {
  static int32_t raw[256];
  static int16_t pcm[256];
  for (;;) {
    size_t bytesRead = 0;
    i2s_read(MIC_PORT, raw, sizeof(raw), &bytesRead, portMAX_DELAY);
    if (!recording) continue;
    size_t n = bytesRead / sizeof(int32_t);
    for (size_t i = 0; i < n; i++) {
      pcm[i] = (int16_t)constrain(raw[i] >> MIC_SHIFT, -32768, 32767);
    }
    xStreamBufferSend(audioBuffer, pcm, n * sizeof(int16_t), 0);  // voll = verwerfen
  }
}

// ---------- Wiedergabe ----------

static uint8_t carryByte;
static bool hasCarry = false;

// Mono-PCM (little endian) auf beide Kanaele; ein ungerades Byte wartet auf das naechste Paket.
static void playBytes(const uint8_t* data, size_t len) {
  static int16_t stereo[512];
  size_t frames = 0;
  auto flush = [&]() {
    size_t written;
    i2s_write(AMP_PORT, stereo, frames * 4, &written, portMAX_DELAY);
    frames = 0;
  };
  auto push = [&](int16_t s) {
    int16_t v = (int32_t)s * VOLUME_PERCENT / 100;
    stereo[2 * frames] = v;
    stereo[2 * frames + 1] = v;
    if (++frames == 256) flush();
  };
  size_t i = 0;
  if (hasCarry && len > 0) {
    push((int16_t)(carryByte | (data[0] << 8)));
    hasCarry = false;
    i = 1;
  }
  for (; i + 1 < len; i += 2) push((int16_t)(data[i] | (data[i + 1] << 8)));
  if (i < len) {
    carryByte = data[i];
    hasCarry = true;
  }
  if (frames) flush();
}

// ---------- HTTP ----------

static bool waitAvailable(uint32_t timeoutMs) {
  uint32_t start = millis();
  while (!client.available()) {
    if (!client.connected() || millis() - start > timeoutMs) return false;
    delay(2);
  }
  return true;
}

static bool readLine(String& line, uint32_t timeoutMs) {
  line = "";
  for (;;) {
    if (!waitAvailable(timeoutMs)) return false;
    char c = client.read();
    if (c == '\n') break;
    if (c != '\r') line += c;
  }
  return true;
}

// Liest bis zu `len` Bytes und spielt sie ab; liefert die Anzahl gelesener Bytes.
static size_t pumpAudio(size_t len, uint32_t timeoutMs) {
  static uint8_t buf[1024];
  size_t done = 0;
  while (done < len) {
    if (!waitAvailable(timeoutMs)) break;
    int n = client.read(buf, min(sizeof(buf), len - done));
    if (n <= 0) break;
    playBytes(buf, n);
    done += n;
  }
  return done;
}

static void writeChunk(const uint8_t* data, size_t len) {
  client.printf("%x\r\n", (unsigned)len);
  client.write(data, len);
  client.print("\r\n");
}

static void blink(int times) {
  for (int i = 0; i < times; i++) {
    digitalWrite(LED, HIGH);
    delay(120);
    digitalWrite(LED, LOW);
    delay(120);
  }
}

static void playResponse() {
  String line;
  // Wartet auf Spracherkennung und den ersten Satz der Antwort.
  if (!readLine(line, 60000)) {
    Serial.println("Keine Antwort vom Server");
    blink(3);
    return;
  }
  int status = line.length() >= 12 ? line.substring(9, 12).toInt() : 0;
  bool chunked = false;
  long contentLength = -1;
  uint32_t rate = 22050;
  while (readLine(line, 10000) && line.length() > 0) {
    String lower = line;
    lower.toLowerCase();
    if (lower.startsWith("transfer-encoding:") && lower.indexOf("chunked") > 0) chunked = true;
    if (lower.startsWith("content-length:")) contentLength = lower.substring(15).toInt();
    if (lower.startsWith("x-sample-rate:")) rate = lower.substring(14).toInt();
    if (lower.startsWith("x-transcript:")) Serial.printf("Erkannt (URL-kodiert): %s\n", line.substring(13).c_str());
  }
  if (status != 200) {
    Serial.printf("HTTP %d\n", status);
    while (readLine(line, 2000)) Serial.println(line);
    blink(3);
    return;
  }

  i2s_set_clk(AMP_PORT, rate, I2S_BITS_PER_SAMPLE_16BIT, I2S_CHANNEL_STEREO);
  hasCarry = false;
  if (chunked) {
    for (;;) {
      if (!readLine(line, 30000)) break;
      long size = strtol(line.c_str(), nullptr, 16);
      if (size <= 0) break;
      if (pumpAudio(size, 30000) < (size_t)size) break;
      readLine(line, 5000);  // CRLF nach dem Chunk
    }
  } else {
    pumpAudio(contentLength >= 0 ? contentLength : SIZE_MAX, 30000);
  }
  i2s_zero_dma_buffer(AMP_PORT);
}

static void conversation() {
  digitalWrite(LED, HIGH);
  xStreamBufferReset(audioBuffer);
  recording = true;
  uint32_t start = millis();

  if (!client.connect(SERVER_HOST, SERVER_PORT)) {
    recording = false;
    digitalWrite(LED, LOW);
    Serial.println("Verbindung zum Server fehlgeschlagen");
    blink(5);
    return;
  }
  client.printf(
      "POST /v1/talk HTTP/1.1\r\n"
      "Host: %s\r\n"
      "Authorization: Bearer %s\r\n"
      "Content-Type: application/octet-stream\r\n"
      "Transfer-Encoding: chunked\r\n"
      "Connection: close\r\n\r\n",
      SERVER_HOST, API_KEY);

  static uint8_t buf[2048];
  size_t sent = 0;
  for (;;) {
    if (digitalRead(BUTTON) == HIGH || millis() - start > MAX_RECORD_MS) recording = false;
    size_t n = xStreamBufferReceive(audioBuffer, buf, sizeof(buf), pdMS_TO_TICKS(40));
    if (n > 0) {
      writeChunk(buf, n);
      sent += n;
    } else if (!recording) {
      break;
    }
  }
  client.print("0\r\n\r\n");
  digitalWrite(LED, LOW);
  Serial.printf("%.1f s Audio gesendet, warte auf Antwort\n", sent / 2.0 / MIC_RATE);

  playResponse();
  client.stop();
}

// ---------- Setup / Loop ----------

static void connectWifi() {
  WiFi.mode(WIFI_STA);
  WiFi.setSleep(false);  // sonst deutlich hoehere Latenz
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
  Serial.print("WLAN");
  while (WiFi.status() != WL_CONNECTED) {
    Serial.print('.');
    digitalWrite(LED, !digitalRead(LED));
    delay(300);
  }
  digitalWrite(LED, LOW);
  Serial.printf(" verbunden, IP %s\n", WiFi.localIP().toString().c_str());
}

void setup() {
  Serial.begin(115200);
  pinMode(BUTTON, INPUT_PULLUP);
  pinMode(LED, OUTPUT);

  audioBuffer = xStreamBufferCreate(AUDIO_BUFFER_BYTES, 1);
  setupMic();
  setupAmp();
  xTaskCreatePinnedToCore(micTask, "mic", 4096, nullptr, 5, nullptr, 1);

#if USE_TLS
  if (strlen(ROOT_CA) > 0) {
    client.setCACert(ROOT_CA);
  } else {
    client.setInsecure();
  }
#endif
  connectWifi();
  Serial.println("Bereit: Taster halten und sprechen");
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) connectWifi();
  if (digitalRead(BUTTON) == LOW) {
    delay(30);  // Entprellen
    if (digitalRead(BUTTON) == LOW) {
      conversation();
      while (digitalRead(BUTTON) == LOW) delay(10);
    }
  }
  delay(5);
}
