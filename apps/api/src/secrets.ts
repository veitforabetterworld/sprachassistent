import { createCipheriv, createDecipheriv, createHash, randomBytes } from "node:crypto";

/**
 * Same layout as src/secrets.rs: `enc1:` plus base64 of `{"iv","tag","d"}`.
 * The key is sha256(`${CREDENTIALS_KEY}|sprachassistent-credentials`).
 * Values without the prefix stay plain text.
 */

const PREFIX = "enc1:";
const CONTEXT = "sprachassistent-credentials";

let warned = false;

function credentialsKey(): Buffer | null {
  const material = (process.env.CREDENTIALS_KEY ?? "").trim();
  if (!material) return null;
  return createHash("sha256").update(`${material}|${CONTEXT}`).digest();
}

export function credentialsEnabled(): boolean {
  return credentialsKey() !== null;
}

export function encrypt(plain: string): string {
  if (!plain) return "";
  const key = credentialsKey();
  if (!key) throw new Error("CREDENTIALS_KEY fehlt, Geheimnis bleibt unverschlüsselt");
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  const data = Buffer.concat([cipher.update(plain, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag();
  const payload = JSON.stringify({
    iv: iv.toString("base64"),
    tag: tag.toString("base64"),
    d: data.toString("base64"),
  });
  return PREFIX + Buffer.from(payload, "utf8").toString("base64");
}

/** Plain text passes through. `enc1:` without a key throws, so ciphertext is never used as a token. */
export function decrypt(stored: string): string {
  if (!stored || !stored.startsWith(PREFIX)) return stored;
  const key = credentialsKey();
  if (!key) throw new Error("CREDENTIALS_KEY fehlt, gespeichertes Geheimnis nicht lesbar");
  const raw = Buffer.from(stored.slice(PREFIX.length), "base64").toString("utf8");
  const payload = JSON.parse(raw) as { iv: string; tag: string; d: string };
  const iv = Buffer.from(payload.iv, "base64");
  const tag = Buffer.from(payload.tag, "base64");
  const data = Buffer.from(payload.d, "base64");
  if (iv.length !== 12 || tag.length !== 16) throw new Error("Geheimnis nicht lesbar");
  const decipher = createDecipheriv("aes-256-gcm", key, iv);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(data), decipher.final()]).toString("utf8");
}

/** Encrypt when a key is configured. Otherwise keep the plain value and warn once. */
export function seal(plain: string): string {
  if (!plain) return "";
  if (!credentialsEnabled()) {
    if (!warned) {
      warned = true;
      console.warn("CREDENTIALS_KEY fehlt, ein Geheimnis liegt im Klartext");
    }
    return plain;
  }
  return encrypt(plain);
}

/** Empty string when the ciphertext cannot be opened, so it is never sent on. */
export function openSecret(stored: string): string {
  try {
    return decrypt(stored);
  } catch (error) {
    console.error(error);
    return "";
  }
}
