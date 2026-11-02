-- Tables of the voice assistant. Idempotent; applied at start by apps/api (after the account tables of
-- @veit/auth-server and @veit/api-keys) and by the Rust server.

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Defaults; the admin area changes them. Speech settings take effect after a restart of the voice server.
INSERT INTO app_settings (key, value) VALUES
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
  ('tts_threads', '2'),
  ('google_oauth_client_id', ''),
  ('google_oauth_client_secret', ''),
  ('account_verification', '0')
ON CONFLICT (key) DO NOTHING;

-- OpenAI-compatible chat providers of one user, tried by ascending priority when a device has none of its own.
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
  max_iterations INTEGER NOT NULL DEFAULT 6,
  busy_phrase TEXT NOT NULL DEFAULT 'Einen Moment.',
  still_busy_phrase TEXT NOT NULL DEFAULT 'Ich bin noch dran.',
  tools_enabled BOOLEAN NOT NULL DEFAULT true,
  -- Owner of the device. Null until the web API assigns the first admin (users may not exist yet).
  user_id BIGINT,
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
  metadata JSONB NOT NULL DEFAULT '{}',
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS device_messages_device ON device_messages (device_id, id);

CREATE TABLE IF NOT EXISTS device_facts (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
  content TEXT NOT NULL,
  origin TEXT NOT NULL DEFAULT 'extraction',
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS device_facts_device ON device_facts (device_id, id);

-- Upgrades of databases created before the agent tables. Fresh installs already have the columns.
ALTER TABLE devices ADD COLUMN IF NOT EXISTS user_id BIGINT;
ALTER TABLE devices ADD COLUMN IF NOT EXISTS max_iterations INTEGER NOT NULL DEFAULT 6;
ALTER TABLE devices ADD COLUMN IF NOT EXISTS busy_phrase TEXT NOT NULL DEFAULT 'Einen Moment.';
ALTER TABLE devices ADD COLUMN IF NOT EXISTS still_busy_phrase TEXT NOT NULL DEFAULT 'Ich bin noch dran.';
ALTER TABLE devices ADD COLUMN IF NOT EXISTS tools_enabled BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE device_messages ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}';
ALTER TABLE device_facts ADD COLUMN IF NOT EXISTS origin TEXT NOT NULL DEFAULT 'extraction';
-- Every model may call tools now; the chatbot's tools_enabled is the only switch.
ALTER TABLE llm_providers DROP COLUMN IF EXISTS supports_tools;
ALTER TABLE llm_providers ADD COLUMN IF NOT EXISTS user_id BIGINT;
CREATE INDEX IF NOT EXISTS devices_user ON devices (user_id);
CREATE INDEX IF NOT EXISTS llm_providers_user ON llm_providers (user_id);

-- MCP servers of one user. stdio stays stored but the voice server refuses to spawn it.
CREATE TABLE IF NOT EXISTS mcp_servers (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id BIGINT NOT NULL,
  name TEXT NOT NULL,
  preset TEXT NOT NULL DEFAULT '',
  transport TEXT NOT NULL DEFAULT 'http',
  url TEXT NOT NULL DEFAULT '',
  command TEXT NOT NULL DEFAULT '',
  args JSONB NOT NULL DEFAULT '[]',
  enabled BOOLEAN NOT NULL DEFAULT true,
  auth_type TEXT NOT NULL DEFAULT 'none',
  secret_enc TEXT NOT NULL DEFAULT '',
  oauth_enc TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'unknown',
  last_error TEXT NOT NULL DEFAULT '',
  last_ok_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS mcp_servers_user ON mcp_servers (user_id);

CREATE TABLE IF NOT EXISTS mcp_tools (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  server_id UUID NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  input_schema JSONB NOT NULL DEFAULT '{}',
  annotations JSONB NOT NULL DEFAULT '{}',
  enabled BOOLEAN NOT NULL DEFAULT true,
  effect TEXT,
  present BOOLEAN NOT NULL DEFAULT true,
  UNIQUE (server_id, name)
);

CREATE TABLE IF NOT EXISTS native_tool_policies (
  user_id BIGINT NOT NULL,
  name TEXT NOT NULL,
  effect TEXT NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT true,
  PRIMARY KEY (user_id, name)
);

CREATE TABLE IF NOT EXISTS agent_runs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id BIGINT NOT NULL,
  device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
  channel TEXT NOT NULL,
  status TEXT NOT NULL,
  user_text TEXT NOT NULL,
  final_text TEXT NOT NULL DEFAULT '',
  iterations INTEGER NOT NULL DEFAULT 0,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT '',
  pending_tools JSONB,
  started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  finished_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS agent_runs_device ON agent_runs (device_id, started_at DESC);

CREATE TABLE IF NOT EXISTS agent_run_steps (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id UUID NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  payload JSONB NOT NULL DEFAULT '{}',
  duration_ms INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS oauth_states (
  state TEXT PRIMARY KEY,
  user_id BIGINT NOT NULL,
  server_id UUID NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL
);
