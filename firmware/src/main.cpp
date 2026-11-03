// Sprachassistent-Client: Taster halten und sprechen, loslassen, Antwort hoeren.
//
// Waehrend der Taster gedrueckt ist, wird das Mikrofon als 16 kHz / 16 Bit mono an POST /v1/talk
// gestreamt (chunked). Der Server antwortet in Rahmen: 1 Byte Art, 4 Byte Laenge (little endian),
// dann der Inhalt. Art 1 ist Audio (16 Bit mono, Rate im Header X-Sample-Rate) und wird direkt
// ueber den MAX98357A abgespielt; Art 2 ist ein Status (thinking, tool:<Name>, speaking), den die
// LED zeigt, bis der Assistent spricht. Was ein Werkzeug tut, weiss der ESP nicht: jeder Chatbot hat
// andere; der Name steht nur im seriellen Protokoll.

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
// Taster gegen GND (rechte Stiftleiste). GPIO 5 ist ein Strapping-Pin: beim Einschalten nicht drücken.
constexpr int BUTTON = 5;
// Freenove ESP32-WROOM: einfarbige LED an GPIO 2. Optional eine WS2812 (RGB) an einem freien Pin,
// z. B. 4; -1 = keine.
constexpr int LED = 2;
constexpr int RGB_LED_PIN = -1;
constexpr int RGB_LEVEL = 80;  // 0..255

constexpr i2s_port_t MIC_PORT = I2S_NUM_0;
constexpr i2s_port_t AMP_PORT = I2S_NUM_1;
constexpr uint32_t MIC_RATE = 16000;
// Der INMP441 liefert 24 Bit im 32-Bit-Wort. >> 16 waere Originalpegel, >> 14 verstaerkt 4-fach.
constexpr int MIC_SHIFT = 14;
// Lautstaerke in Prozent; ueber 100 verstaerkt, Spitzen werden begrenzt statt umzuklappen.
constexpr int VOLUME_PERCENT = 250;
constexpr uint32_t MAX_RECORD_MS = 20000;
// Puffert ca. 1,5 s Audio, waehrend die Verbindung (TLS-Handshake) aufgebaut wird.
constexpr size_t AUDIO_BUFFER_BYTES = 64 * 1024;

static StreamBufferHandle_t audioBuffer;
static volatile bool recording = false;
// Fuer das Protokoll: verworfene Bytes (Puffer voll) und hoechster Pegel der Aufnahme.
static volatile size_t droppedBytes = 0;
static volatile int32_t peakLevel = 0;

#if USE_TLS
static WiFiClientSecure client;
#else
static WiFiClient client;
#endif

// ---------- LED ----------
// Aufnahme: an (RGB rot). KI denkt: pulsiert langsam (blau). Ein Werkzeug laeuft: pulsiert schnell
// (tuerkis). Sprache wird erzeugt: flackert (violett). WLAN-Suche: blinkt (gelb). Waehrend der
// Assistent spricht und sonst: aus.

enum class LedState { Off, On, Recording, Thinking, Tool, Speaking, Connecting };

// Sanftes Pulsieren mit der Periode `periodMs`, Helligkeit 15..255.
static uint8_t pulse(uint32_t periodMs) {
  float phase = (millis() % periodMs) / (float)periodMs;
  return 15 + 240 * (0.5f - 0.5f * cosf(phase * 2 * PI));
}
static volatile LedState ledState = LedState::Off;
constexpr int LED_CHANNEL = 0;

static void showLed(uint8_t level, uint8_t r, uint8_t g, uint8_t b) {
  ledcWrite(LED_CHANNEL, level);
  if (RGB_LED_PIN >= 0) {
    neopixelWrite(RGB_LED_PIN, r * RGB_LEVEL / 255, g * RGB_LEVEL / 255,
                  b * RGB_LEVEL / 255);
  }
}

static void ledTask(void*) {
  for (;;) {
    switch (ledState) {
      case LedState::Recording:
        showLed(255, 255, 0, 0);
        break;
      case LedState::Thinking: {
        uint8_t v = pulse(1200);
        showLed(v, 0, 0, v);
        break;
      }
      case LedState::Tool: {
        uint8_t v = pulse(400);
        showLed(v, 0, v * 7 / 10, v);
        break;
      }
      case LedState::Speaking: {
        bool on = (millis() / 80) % 2;
        showLed(on ? 255 : 40, on ? 160 : 25, 0, on ? 255 : 40);
        break;
      }
      case LedState::Connecting: {
        bool on = (millis() / 300) % 2;
        showLed(on ? 255 : 0, on ? 255 : 0, on ? 160 : 0, 0);
        break;
      }
      case LedState::On:
        showLed(255, 255, 80, 0);
        break;
      default:
        showLed(0, 0, 0, 0);
    }
    vTaskDelay(pdMS_TO_TICKS(20));
  }
}

static void setupLed() {
  ledcSetup(LED_CHANNEL, 5000, 8);
  ledcAttachPin(LED, LED_CHANNEL);
  xTaskCreatePinnedToCore(ledTask, "led", 2048, nullptr, 1, nullptr, 0);
}

// ---------- I2S ----------

static void setupMic() {
  i2s_config_t cfg = {};
  cfg.mode = (i2s_mode_t)(I2S_MODE_MASTER | I2S_MODE_RX);
  cfg.sample_rate = MIC_RATE;
  cfg.bits_per_sample = I2S_BITS_PER_SAMPLE_32BIT;
  // L/R an GND ist der linke Kanal, der Treiber des ESP32 liefert ihn aber unter ONLY_RIGHT
  // (gemessen mit src/hwtest.cpp; mit ONLY_LEFT kommen nur Nullen).
  cfg.channel_format = I2S_CHANNEL_FMT_ONLY_RIGHT;
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
      if (abs(pcm[i]) > peakLevel) peakLevel = abs(pcm[i]);
    }
    size_t bytes = n * sizeof(int16_t);
    droppedBytes += bytes - xStreamBufferSend(audioBuffer, pcm, bytes, 0);  // voll = verwerfen
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
    int16_t v = (int16_t)constrain((int32_t)s * VOLUME_PERCENT / 100, -32768, 32767);
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

// Antwortkoerper, egal ob chunked oder mit Content-Length: readBody liefert die Nutzdaten ohne die
// Chunk-Zeilen. bodyLeft ist der Rest des aktuellen Chunks bzw. des Koerpers (-1 = bis zum Ende
// der Verbindung).
static bool bodyChunked = false;
static long bodyLeft = 0;
static bool bodyDone = false;

static int readBody(uint8_t* buf, size_t len, uint32_t timeoutMs) {
  if (bodyDone) return 0;
  if (bodyChunked && bodyLeft == 0) {
    String line;
    if (!readLine(line, timeoutMs)) {
      bodyDone = true;
      return 0;
    }
    bodyLeft = strtol(line.c_str(), nullptr, 16);
    if (bodyLeft <= 0) {
      bodyDone = true;
      return 0;
    }
  }
  size_t want = bodyLeft >= 0 ? min(len, (size_t)bodyLeft) : len;
  if (!waitAvailable(timeoutMs)) {
    bodyDone = true;
    return 0;
  }
  int n = client.read(buf, want);
  if (n <= 0) {
    bodyDone = true;
    return 0;
  }
  if (bodyLeft > 0) {
    bodyLeft -= n;
    if (bodyLeft == 0) {
      if (bodyChunked) {
        String crlf;
        readLine(crlf, 5000);  // CRLF nach dem Chunk
      } else {
        bodyDone = true;
      }
    }
  }
  return n;
}

static bool readExact(uint8_t* buf, size_t len, uint32_t timeoutMs) {
  size_t got = 0;
  while (got < len) {
    int n = readBody(buf + got, len - got, timeoutMs);
    if (n <= 0) return false;
    got += n;
  }
  return true;
}

constexpr uint8_t FRAME_AUDIO = 1;
constexpr uint8_t FRAME_STATUS = 2;

static void showStatus(const char* status) {
  if (!strcmp(status, "thinking")) ledState = LedState::Thinking;
  else if (!strncmp(status, "tool:", 5)) ledState = LedState::Tool;
  else if (!strcmp(status, "speaking")) ledState = LedState::Speaking;
  Serial.printf("[%lu ms] %s\n", (unsigned long)millis(), status);
}

// Spielt die Rahmen der Antwort ab, bis der Server fertig ist.
static void playFrames() {
  static uint8_t buf[1024];
  uint8_t head[5];
  bool spoke = false;
  // Die erste Antwort kann auf ein langsames Werkzeug warten.
  while (readExact(head, sizeof(head), 60000)) {
    uint32_t len = head[1] | (head[2] << 8) | (head[3] << 16) | ((uint32_t)head[4] << 24);
    if (head[0] == FRAME_STATUS && len < 128) {
      char status[128];
      if (!readExact((uint8_t*)status, len, 10000)) break;
      status[len] = '\0';
      showStatus(status);
      continue;
    }
    if (head[0] == FRAME_AUDIO) {
      if (!spoke) Serial.printf("[%lu ms] spricht\n", (unsigned long)millis());
      spoke = true;
      ledState = LedState::Off;  // der Assistent spricht
    }
    // Audio abspielen, Unbekanntes ueberspringen.
    while (len > 0) {
      int n = readBody(buf, min((uint32_t)sizeof(buf), len), 45000);
      if (n <= 0) return;
      if (head[0] == FRAME_AUDIO) playBytes(buf, n);
      len -= n;
    }
  }
}

static void writeChunk(const uint8_t* data, size_t len) {
  client.printf("%x\r\n", (unsigned)len);
  client.write(data, len);
  client.print("\r\n");
}

static void blink(int times) {
  for (int i = 0; i < times; i++) {
    ledState = LedState::On;
    delay(120);
    ledState = LedState::Off;
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
  bodyChunked = chunked;
  bodyLeft = chunked ? 0 : contentLength;
  bodyDone = !chunked && contentLength == 0;
  playFrames();
  ledState = LedState::Off;
  i2s_zero_dma_buffer(AMP_PORT);
}

static void conversation() {
  ledState = LedState::Recording;
  xStreamBufferReset(audioBuffer);
  droppedBytes = 0;
  peakLevel = 0;
  recording = true;
  uint32_t start = millis();

  bool connected = client.connect(SERVER_HOST, SERVER_PORT);
  uint32_t connectMs = millis() - start;
  if (!connected) {
    recording = false;
    ledState = LedState::Off;
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
  uint32_t pressed = 0;
  for (;;) {
    if (recording && (digitalRead(BUTTON) == HIGH || millis() - start > MAX_RECORD_MS)) {
      recording = false;
      pressed = millis() - start;
    }
    size_t n = xStreamBufferReceive(audioBuffer, buf, sizeof(buf), pdMS_TO_TICKS(40));
    if (n > 0) {
      writeChunk(buf, n);
      sent += n;
    } else if (!recording) {
      break;
    }
  }
  client.print("0\r\n\r\n");
  ledState = LedState::Thinking;
  Serial.printf("[%lu ms] losgelassen\n", (unsigned long)millis());
  Serial.printf(
      "Taster %.1f s gedrueckt, %.1f s Audio gesendet, %.1f s verworfen, Spitzenpegel %d/32767, "
      "Verbindung nach %u ms\n",
      pressed / 1000.0, sent / 2.0 / MIC_RATE, droppedBytes / 2.0 / MIC_RATE, (int)peakLevel,
      (unsigned)connectMs);

  playResponse();
  client.stop();
}

// ---------- Setup / Loop ----------

static void connectWifi() {
  WiFi.mode(WIFI_STA);
  WiFi.setSleep(false);  // sonst deutlich hoehere Latenz
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
  ledState = LedState::Connecting;
  Serial.printf("WLAN %s", WIFI_SSID);
  uint32_t start = millis();
  while (WiFi.status() != WL_CONNECTED) {
    Serial.print('.');
    delay(300);
    if (millis() - start > 15000) {
      // Hilft bei der Fehlersuche: Ist das Netz ueberhaupt sichtbar (nur 2,4 GHz)?
      Serial.printf("\nKeine Verbindung (Status %d). Sichtbare Netze:\n", WiFi.status());
      WiFi.disconnect();
      delay(100);
      int n = WiFi.scanNetworks();
      if (n <= 0) Serial.printf("  keine (scanNetworks = %d)\n", n);
      for (int i = 0; i < n; i++) {
        Serial.printf("  %-32s Kanal %2d  %4d dBm%s\n", WiFi.SSID(i).c_str(), WiFi.channel(i),
                      WiFi.RSSI(i), WiFi.SSID(i) == WIFI_SSID ? "  <- konfiguriert" : "");
      }
      WiFi.disconnect();
      WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
      start = millis();
    }
  }
  ledState = LedState::Off;
  Serial.printf(" verbunden, IP %s\n", WiFi.localIP().toString().c_str());
}

void setup() {
  Serial.begin(115200);
  pinMode(BUTTON, INPUT_PULLUP);
  setupLed();

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
