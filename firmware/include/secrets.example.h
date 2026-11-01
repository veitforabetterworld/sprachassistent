// Nach secrets.h kopieren und ausfuellen. secrets.h ist in .gitignore.
#pragma once

#define WIFI_SSID "MeinWLAN"
#define WIFI_PASSWORD "geheim"

// Ueber die Subdomain (HTTPS) ...
#define SERVER_HOST "sprachassistent.plenivo.de"
#define SERVER_PORT 443
#define USE_TLS 1
// ... oder direkt im Heimnetz (schneller, auf dem Pi dann API_HOST=0.0.0.0 setzen):
// #define SERVER_HOST "192.168.178.50"
// #define SERVER_PORT 3020
// #define USE_TLS 0

// API_KEY aus der .env auf dem Pi
#define API_KEY "..."

// Root-Zertifikat fuer HTTPS: Inhalt von https://letsencrypt.org/certs/isrgrootx1.pem einfuegen.
// Leer = das Zertifikat wird nicht geprueft (funktioniert, ist aber angreifbar).
#define ROOT_CA ""
// #define ROOT_CA R"(-----BEGIN CERTIFICATE-----
// ...
// -----END CERTIFICATE-----
// )"
