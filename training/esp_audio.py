"""Verbindung zur Aufnahme-Firmware (firmware/src/collect.cpp) ueber USB-Seriell.

Paket (beide Richtungen): A5 5A | Typ | Folgenummer | Laenge (2, LE) | Nutzdaten.
Audio ist int16 LE, 16 kHz mono, mit genau dem Pegel, den auch die Hauptfirmware an den Server schickt.
"""

import threading
import time

import numpy as np
import serial

RATE = 16000
BAUD = 921600

T_AUDIO, T_TEXT = 0x01, 0x02
T_MIC_ON, T_MIC_OFF, T_PLAY, T_VOLUME = 0x10, 0x11, 0x20, 0x21


class EspAudio:
    def __init__(self, port: str):
        # Kein Reset beim Oeffnen: DTR/RTS haengen am Boot-Schaltkreis, also vor open() abschalten
        self.ser = serial.Serial()
        self.ser.port = port
        self.ser.baudrate = BAUD
        self.ser.timeout = 0.1
        self.ser.dtr = False
        self.ser.rts = False
        self.ser.open()
        self._lock = threading.Lock()
        self._chunks: list[np.ndarray] = []
        self._recording = False
        self.dropped_packets = 0
        self.messages: list[str] = []
        self._tx_seq = 0
        self._last_seq = None
        self._stop = False
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()
        self._audio_seen = False
        # Falls der ESP doch neu startet, verpasst er den ersten Befehl: bis zu 3 s wiederholen
        for _ in range(15):
            self._send(T_MIC_ON)
            time.sleep(0.2)
            if self._audio_seen:
                break
        else:
            raise RuntimeError(f"Keine Audiodaten von {port}. Ist die collect-Firmware geflasht?")

    def close(self):
        self._send(T_MIC_OFF)
        self._stop = True
        self._reader.join(timeout=1)
        self.ser.close()

    def _send(self, ptype: int, payload: bytes = b""):
        header = bytes([0xA5, 0x5A, ptype, self._tx_seq & 0xFF, len(payload) & 0xFF, len(payload) >> 8])
        self._tx_seq += 1
        self.ser.write(header + payload)

    def _read_loop(self):
        buf = bytearray()
        while not self._stop:
            buf += self.ser.read(4096)
            while True:
                start = buf.find(b"\xa5\x5a")
                if start < 0:
                    del buf[:-1]
                    break
                if start > 0:
                    del buf[:start]
                if len(buf) < 6:
                    break
                ptype, seq, length = buf[2], buf[3], buf[4] | (buf[5] << 8)
                if length > 4096:
                    del buf[:2]  # kein gueltiger Kopf, weitersuchen
                    continue
                if len(buf) < 6 + length:
                    break
                payload = bytes(buf[6 : 6 + length])
                del buf[: 6 + length]
                if self._last_seq is not None and seq != (self._last_seq + 1) & 0xFF:
                    self.dropped_packets += 1
                self._last_seq = seq
                if ptype == T_AUDIO:
                    self._audio_seen = True
                    if self._recording:
                        with self._lock:
                            self._chunks.append(np.frombuffer(payload, dtype="<i2").copy())
                elif ptype == T_TEXT:
                    self.messages.append(payload.decode("utf-8", "replace"))

    def start(self):
        with self._lock:
            self._chunks = []
        self._recording = True

    def stop(self) -> np.ndarray:
        self._recording = False
        with self._lock:
            chunks, self._chunks = self._chunks, []
        return np.concatenate(chunks) if chunks else np.zeros(0, dtype=np.int16)

    def record(self, seconds: float) -> np.ndarray:
        self.start()
        time.sleep(seconds)
        return self.stop()

    def set_volume(self, percent: int):
        self._send(T_VOLUME, int(percent).to_bytes(2, "little"))

    def play(self, samples: np.ndarray):
        """Spielt int16-Audio (16 kHz) ueber den Lautsprecher, ungefaehr in Echtzeit."""
        data = samples.astype("<i2").tobytes()
        chunk = 2048  # 64 ms
        t0 = time.monotonic()
        for i in range(0, len(data), chunk):
            self._send(T_PLAY, data[i : i + chunk])
            # Hoechstens 0,3 s vorauseilen, damit der Empfangspuffer des ESP nicht ueberlaeuft
            ahead = (i + chunk) / 2 / RATE - (time.monotonic() - t0)
            if ahead > 0.3:
                time.sleep(ahead - 0.3)
