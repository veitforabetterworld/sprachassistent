// Sprachassistent-Client: Taster halten und sprechen, loslassen, Antwort hoeren.
// Mit Wake Word (Env giraffe/computer, siehe wakeword.h) auch: Wort sagen, sprechen, nach einer
// kurzen Pause endet die Aufnahme von selbst.
//
// Waehrend der Taster gedrueckt ist, wird das Mikrofon als 16 kHz / 16 Bit mono an POST /v1/talk
// gestreamt (chunked). Der Server antwortet in Rahmen: 1 Byte Art, 4 Byte Laenge (little endian),
// dann der Inhalt. Art 1 ist Audio (16 Bit mono, Rate im Header X-Sample-Rate) und wird direkt
// ueber den MAX98357A abgespielt; Art 2 ist ein Status (thinking, tool:<Name>, speaking), den die
// LED zeigt, bis der Assistent spricht. Was ein Werkzeug tut, weiss der ESP nicht: jeder Chatbot hat
// andere; der Name steht nur im seriellen Protokoll.
//
// Der Motor (z. B. der Kopf einer Laber-Giraffe) laeuft, solange der Server arbeitet: vom Loslassen
// bis zum ersten Ton und wieder nach jedem Status, der nach Audio kommt ("Einen Moment bitte" und
// dann das Werkzeug). Er steht, bis der I2S-Puffer leer gespielt ist: nie sprechen und bewegen.

#include <Arduino.h>
#include <WiFi.h>
#include <WiFiClientSecure.h>
#include <driver/i2s.h>
#include <freertos/stream_buffer.h>

#include "secrets.h"
#include "wakeword.h"

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
// Freenove ESP32-WROOM: einfarbige LED an GPIO 2, WS2812 (RGB) an GPIO 16; -1 = keine RGB-LED.
// neopixelWrite bindet sich beim ersten Aufruf fest an einen Pin, also nur eine RGB-LED.
constexpr int LED = 2;
constexpr int RGB_LED_PIN = 16;
constexpr int RGB_LEVEL = 80;  // 0..255
// Motor ueber einen Logic-Level-MOSFET (Gate an diesem Pin, HIGH = laeuft); -1 = kein Motor.
constexpr int MOTOR_PIN = 4;

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

// Nach dem Wake Word: Aufnahme endet nach so viel Stille, oder wenn gar nichts gesagt wird.
constexpr uint32_t SILENCE_END_MS = 800;
constexpr uint32_t NO_SPEECH_MS = 5000;
// Nach der eigenen Antwort so lange nicht auf das Wake Word hoeren.
constexpr uint32_t WAKE_LOCKOUT_MS = 500;

static StreamBufferHandle_t audioBuffer;
static volatile bool recording = false;
// Wake Word: micTask hoert nur, solange wakeListening gesetzt ist; wakeBusy waehrend einer Inferenz.
static volatile bool wakeListening = false;
static volatile bool wakeBusy = false;
static volatile bool wakeTriggered = false;
// Sprache in der laufenden Aufnahme (fuer das Ende nach dem Wake Word).
static volatile bool voiceSeen = false;
static volatile uint32_t lastVoiceMs = 0;
static volatile float noiseLevel = 50;

// Blockpegel (RMS), ab dem in der Aufnahme Sprache angenommen wird
static float voiceThreshold() { return max(noiseLevel * 4, 200.0f); }
// Fuer das Protokoll: verworfene Bytes (Puffer voll) und hoechster Pegel der Aufnahme.
static volatile size_t droppedBytes = 0;
static volatile int32_t peakLevel = 0;

#if USE_TLS
static WiFiClientSecure client;
#else
static WiFiClient client;
#endif

// ---------- LED ----------
// Aufnahme: an (RGB rot, nach dem Wake Word gruen). KI denkt: pulsiert langsam (blau). Ein Werkzeug laeuft: pulsiert schnell
// (tuerkis). Sprache wird erzeugt: flackert (violett). WLAN-Suche: blinkt (gelb). Waehrend der
// Assistent spricht und sonst: aus.
// Fehler, auch an der einfarbigen LED zu unterscheiden: Server nicht erreichbar oder keine Antwort:
// 5x kurz (rot). Schluessel abgelehnt (401/403/404): 3x lang (orange). Anderer Serverfehler: 3x kurz (rot).
// Spracherkennung aus oder nichts verstanden sagt der Server selbst an.

enum class LedState { Off, On, Recording, Listening, Thinking, Tool, Speaking, Connecting };

// Sanftes Pulsieren mit der Periode `periodMs`, Helligkeit 15..255.
static uint8_t pulse(uint32_t periodMs) {
  float phase = (millis() % periodMs) / (float)periodMs;
  return 15 + 240 * (0.5f - 0.5f * cosf(phase * 2 * PI));
}
static volatile LedState ledState = LedState::Off;
// Farbe von LedState::On an der RGB-LED, siehe blink().
static volatile uint8_t onRed = 255, onGreen = 80;
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
      case LedState::Listening:
        showLed(255, 0, 255, 0);
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
        showLed(255, onRed, onGreen, 0);
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

// ---------- Motor ----------

static volatile bool motorWanted = false;
// Bis dahin spielt der I2S-Puffer noch Audio, siehe playBytes().
static volatile uint32_t audioUntil = 0;
// Bytes im DMA-Puffer des Verstaerkers (8 x 512 Frames, 4 Byte pro Frame).
constexpr uint32_t AMP_DMA_FRAMES = 8 * 512;
static uint32_t ampRate = 22050;

static void motorTask(void*) {
  for (;;) {
    bool run = motorWanted && (int32_t)(millis() - audioUntil) >= 0;
    digitalWrite(MOTOR_PIN, run ? HIGH : LOW);
    vTaskDelay(pdMS_TO_TICKS(10));
  }
}

static void setupMotor() {
  if (MOTOR_PIN < 0) return;
  pinMode(MOTOR_PIN, OUTPUT);
  digitalWrite(MOTOR_PIN, LOW);
  xTaskCreatePinnedToCore(motorTask, "motor", 2048, nullptr, 1, nullptr, 0);
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

// Liest das Mikrofon dauerhaft; nur waehrend der Aufnahme landet das Audio im Puffer. Sonst hoert
// es auf das Wake Word.
static void micTask(void*) {
  static int32_t raw[256];
  static int16_t pcm[256];
  for (;;) {
    size_t bytesRead = 0;
    i2s_read(MIC_PORT, raw, sizeof(raw), &bytesRead, portMAX_DELAY);
    size_t n = bytesRead / sizeof(int32_t);
    for (size_t i = 0; i < n; i++) pcm[i] = (int16_t)constrain(raw[i] >> MIC_SHIFT, -32768, 32767);

    // Pegel des Blocks (16 ms): Grundrauschen im Leerlauf mitfuehren, in der Aufnahme Sprache erkennen
    float sum = 0;
    for (size_t i = 0; i < n; i++) sum += (float)pcm[i] * pcm[i];
    float rms = n ? sqrtf(sum / n) : 0;

    if (!recording) {
      // Grundrauschen: faellt schnell, steigt nur langsam (~30 s). Sonst treiben das Wake Word selbst
      // und die Antwort aus dem Lautsprecher den Wert hoch, und normale Sprache gilt danach als Stille.
      if ((int32_t)(millis() - audioUntil) >= 0) {
        noiseLevel += (rms - noiseLevel) * (rms < noiseLevel ? 0.05f : 0.0005f);
        noiseLevel = max((float)noiseLevel, 10.0f);
      }
      if (USE_WAKEWORD && wakeListening) {
        wakeBusy = true;
        if (wakeListening && wakewordFeed(pcm, n)) {
          wakeTriggered = true;
          wakeListening = false;
        }
        wakeBusy = false;
      }
      continue;
    }
    if (rms > voiceThreshold()) {
      voiceSeen = true;
      lastVoiceMs = millis();
    }
    for (size_t i = 0; i < n; i++) {
      if (abs(pcm[i]) > peakLevel) peakLevel = abs(pcm[i]);
    }
    size_t bytes = n * sizeof(int16_t);
    droppedBytes += bytes - xStreamBufferSend(audioBuffer, pcm, bytes, 0);  // voll = verwerfen
  }
}

static bool wakeReady = false;
static uint32_t listenAfter = 0;

// Haelt die Wake-Word-Erkennung an und gibt ihren Speicher fuer TLS frei.
static void stopListening() {
  wakeListening = false;
  while (wakeBusy) delay(1);
  if (USE_WAKEWORD && wakeReady) wakewordUnload();
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
    // Nach dem Schreiben ist der DMA-Puffer hoechstens voll; 50 ms Reserve.
    audioUntil = millis() + AMP_DMA_FRAMES * 1000 / ampRate + 50;
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

// Jeder Status heisst: der Server arbeitet (auch "speaking", dann erzeugt er die Sprache).
static void showStatus(const char* status) {
  if (!strcmp(status, "thinking")) ledState = LedState::Thinking;
  else if (!strncmp(status, "tool:", 5)) ledState = LedState::Tool;
  else if (!strcmp(status, "speaking")) ledState = LedState::Speaking;
  motorWanted = true;
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
      motorWanted = false;
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

static void blink(int times, uint32_t ms, uint8_t red, uint8_t green) {
  onRed = red;
  onGreen = green;
  for (int i = 0; i < times; i++) {
    ledState = LedState::On;
    delay(ms);
    ledState = LedState::Off;
    delay(ms);
  }
}

static void blinkUnreachable() { blink(5, 120, 255, 0); }
static void blinkKeyRejected() { blink(3, 400, 255, 80); }
static void blinkServerError() { blink(3, 120, 255, 0); }

static void playResponse() {
  String line;
  // Wartet auf Spracherkennung und den ersten Satz der Antwort.
  if (!readLine(line, 60000)) {
    motorWanted = false;
    Serial.println("Keine Antwort vom Server");
    blinkUnreachable();
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
    motorWanted = false;
    Serial.printf("HTTP %d\n", status);
    // Der Koerper ist { "code": ..., "detail": ... }, siehe src/failure.rs.
    while (readLine(line, 2000)) Serial.println(line);
    if (status == 401 || status == 403 || status == 404) blinkKeyRejected();
    else blinkServerError();
    return;
  }

  i2s_set_clk(AMP_PORT, rate, I2S_BITS_PER_SAMPLE_16BIT, I2S_CHANNEL_STEREO);
  ampRate = rate > 0 ? rate : 22050;
  hasCarry = false;
  bodyChunked = chunked;
  bodyLeft = chunked ? 0 : contentLength;
  bodyDone = !chunked && contentLength == 0;
  playFrames();
  motorWanted = false;
  ledState = LedState::Off;
  i2s_zero_dma_buffer(AMP_PORT);
}

// byWake: nach dem Wake Word gestartet, endet bei Stille statt beim Loslassen des Tasters.
static void conversation(bool byWake) {
  stopListening();
  ledState = byWake ? LedState::Listening : LedState::Recording;
  xStreamBufferReset(audioBuffer);
  droppedBytes = 0;
  peakLevel = 0;
  voiceSeen = false;
  recording = true;
  uint32_t start = millis();

  bool connected = client.connect(SERVER_HOST, SERVER_PORT);
  uint32_t connectMs = millis() - start;
  if (!connected) {
    recording = false;
    ledState = LedState::Off;
    Serial.println("Verbindung zum Server fehlgeschlagen");
    blinkUnreachable();
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
    bool done = byWake ? (voiceSeen && millis() - lastVoiceMs > SILENCE_END_MS) ||
                             (!voiceSeen && millis() - start > NO_SPEECH_MS)
                       : digitalRead(BUTTON) == HIGH;
    if (recording && (done || millis() - start > MAX_RECORD_MS)) {
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
  motorWanted = true;
  Serial.printf("[%lu ms] %s\n", (unsigned long)millis(), byWake ? "Aufnahme beendet (Stille)" : "losgelassen");
  Serial.printf(
      "Aufnahme %.1f s, %.1f s Audio gesendet, %.1f s verworfen, Spitzenpegel %d/32767, "
      "Verbindung nach %u ms\n",
      pressed / 1000.0, sent / 2.0 / MIC_RATE, droppedBytes / 2.0 / MIC_RATE, (int)peakLevel,
      (unsigned)connectMs);
  if (byWake) {
    Serial.printf("Rauschen %.0f, Sprachschwelle %.0f, letzte Sprache nach %.1f s\n", (float)noiseLevel,
                  voiceThreshold(), voiceSeen ? (lastVoiceMs - start) / 1000.0 : -1.0);
  }

  playResponse();
  client.stop();
}

// Nach einer Unterhaltung wieder auf das Wake Word hoeren, sobald die Antwort verklungen ist.
static void resumeListening() {
  if (!USE_WAKEWORD || !wakeReady) return;
  uint32_t until = (int32_t)(audioUntil - millis()) > 0 ? audioUntil : millis();
  listenAfter = until + WAKE_LOCKOUT_MS;
  if (!wakewordLoad()) Serial.println("Wake Word: Laden fehlgeschlagen");
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
  setupMotor();

  audioBuffer = xStreamBufferCreate(AUDIO_BUFFER_BYTES, 1);
  setupMic();
  setupAmp();
  // Die Wake-Word-Inferenz laeuft im Mikrofon-Task und braucht mehr Stack
  xTaskCreatePinnedToCore(micTask, "mic", USE_WAKEWORD ? 8192 : 4096, nullptr, 5, nullptr, 1);
  if (USE_WAKEWORD) wakeReady = wakewordBegin();

#if USE_TLS
  if (strlen(ROOT_CA) > 0) {
    client.setCACert(ROOT_CA);
  } else {
    client.setInsecure();
  }
#endif
  connectWifi();
  if (wakeReady) {
    Serial.printf("Bereit: \"%s\" sagen oder Taster halten und sprechen\n", wakewordName());
  } else {
    Serial.println("Bereit: Taster halten und sprechen");
  }
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) connectWifi();
  if (wakeReady && !wakeListening && !wakeTriggered && (int32_t)(millis() - listenAfter) >= 0) {
    wakeListening = true;
  }
  if (wakeTriggered) {
    wakeTriggered = false;
    conversation(true);
    resumeListening();
  }
  if (digitalRead(BUTTON) == LOW) {
    delay(30);  // Entprellen
    if (digitalRead(BUTTON) == LOW) {
      conversation(false);
      resumeListening();
      while (digitalRead(BUTTON) == LOW) delay(10);
    }
  }
  delay(5);
}
