// Nach secrets.h kopieren und ausfuellen. secrets.h ist in .gitignore.
#pragma once

#define WIFI_SSID "MeinWLAN"
#define WIFI_PASSWORD "geheim"

// Rechner mit dem Sprachassistenten im Heimnetz (docker compose, Port 80) ...
#define SERVER_HOST "192.168.178.50"
#define SERVER_PORT 80
#define USE_TLS 0
// ... oder ueber eine Domain mit HTTPS (SITE in der .env)
// #define SERVER_HOST "assistent.example.de"
// #define SERVER_PORT 443
// #define USE_TLS 1

// ESP-Schluessel aus dem Adminbereich (Tab ESP-Schluessel, beginnt mit spa_)
#define API_KEY "..."

// Nur bei HTTPS: Root-Zertifikat, Inhalt von https://letsencrypt.org/certs/isrgrootx1.pem einfuegen.
// Leer = das Zertifikat wird nicht geprueft (funktioniert, ist aber angreifbar).
#define ROOT_CA ""
// #define ROOT_CA R"(-----BEGIN CERTIFICATE-----
// ...
// -----END CERTIFICATE-----
// )"
