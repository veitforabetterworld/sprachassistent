-- Tables of the voice assistant. Idempotent; applied at start by apps/api (after the account tables of
-- @veit/auth-server and @veit/api-keys) and by the Rust server.

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Defaults; the admin area changes them. Speech settings take effect after a restart of the voice server.
INSERT INTO app_settings (key, value) VALUES
  ('registration_open', '0'),
  ('smtp_host', ''),
  ('smtp_port', '587'),
  ('smtp_secure', '0'),
  ('smtp_user', ''),
  ('smtp_password', ''),
  ('smtp_from', ''),
  ('whisper_model', 'models/whisper/ggml-small-q5_1.bin'),
  ('whisper_threads', '4'),
  ('whisper_beam_size', '1'),
  ('whisper_language', 'de'),
  ('tts_threads', '2')
ON CONFLICT (key) DO NOTHING;

-- OpenAI-compatible chat providers, tried by ascending priority when a device has none of its own.
CREATE TABLE IF NOT EXISTS llm_providers (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  base_url TEXT NOT NULL,
  api_key TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT true,
  priority INTEGER NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Free tiers prepared once; only the key is missing (console.groq.com, aistudio.google.com).
INSERT INTO llm_providers (name, base_url, model, priority)
SELECT * FROM (VALUES
  ('Groq', 'https://api.groq.com/openai/v1', 'qwen/qwen3.8-27b', 10),
  ('Google Gemini', 'https://generativelanguage.googleapis.com/v1beta/openai', 'gemini-2.5-flash-lite', 20)
) AS seed(name, base_url, model, priority)
WHERE NOT EXISTS (SELECT 1 FROM llm_providers);

-- One ESP each, with its own voice, personality and memory. ESP keys are @veit/api-keys keys with the
-- permission 'device.talk' and the device id as resource.
CREATE TABLE IF NOT EXISTS devices (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  voice TEXT NOT NULL DEFAULT 'vits-piper-de_DE-thorsten-medium',
  speed REAL NOT NULL DEFAULT 1.0,
  persona TEXT NOT NULL DEFAULT '',
  provider_id UUID REFERENCES llm_providers(id) ON DELETE SET NULL,
  history_messages INTEGER NOT NULL DEFAULT 12,
  memory_extraction BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO devices (name, persona)
SELECT 'Kurt', 'Du bist Kurt, ein Sprachassistent mit dem Charakter eines alten, leicht grummeligen, aber herzensguten Butlers.
Du bist trocken-humorvoll und etwas förmlich im Ton, duzt den Nutzer aber freundschaftlich.
Du gibst ehrliche, hilfreiche Antworten und machst gelegentlich eine kleine ironische Bemerkung, ohne gemein zu werden.
Wenn du etwas nicht weißt, sagst du das offen.'
WHERE NOT EXISTS (SELECT 1 FROM devices);

CREATE TABLE IF NOT EXISTS device_messages (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS device_messages_device ON device_messages (device_id, id);

CREATE TABLE IF NOT EXISTS device_facts (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
  content TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS device_facts_device ON device_facts (device_id, id);
