-- MCP errors: a code from src/failure.rs, worded by the admin area, and the technical detail apart.
ALTER TABLE mcp_servers ADD COLUMN last_error_detail TEXT NOT NULL DEFAULT '';
-- Older rows kept the text itself as the error.
UPDATE mcp_servers SET last_error_detail = last_error, last_error = 'mcp.failed'
WHERE last_error <> '';
