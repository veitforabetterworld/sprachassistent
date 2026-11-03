// Wake-Word-Erkennung auf dem ESP (TFLite Micro). Nur in den Envs mit custom_wakeword aktiv; das
// Modell kommt aus firmware/wakewords/<wort>.h (siehe tools/wakeword.py).
#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef WAKEWORD
#define USE_WAKEWORD 1
#else
#define USE_WAKEWORD 0
#endif

// Frontend und Op-Resolver einrichten, Modell laden. false = Wake Word nicht verfuegbar.
bool wakewordBegin();
// Modell freigeben (z. B. waehrend TLS) bzw. wieder laden; Laden dauert ~20 ms.
void wakewordUnload();
bool wakewordLoad();
// 16-kHz-Samples einspeisen; true, wenn das Wake Word gerade erkannt wurde.
bool wakewordFeed(const int16_t* pcm, size_t n);
const char* wakewordName();
