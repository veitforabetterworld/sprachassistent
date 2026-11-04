import { createSecretBox } from "@veit/secret-box";
import "./env.js";

/**
 * Provider keys, tool server tokens and secret settings (@veit/secret-box, `enc1:`). The key is
 * sha256(`${CREDENTIALS_KEY}|sprachassistent-credentials`), the same as in src/secrets.rs, so the voice server
 * reads what this side writes. Plain text is never read or written here; older values were converted once
 * (secretsMigration.ts).
 */

export const CREDENTIALS_CONTEXT = "sprachassistent-credentials";
/** TOTP secrets in users; key material is the generated setting auth_mfa_key. */
export const MFA_TOTP_CONTEXT = "sprachassistent-mfa-totp";

export const credentialsBox = createSecretBox({ key: () => process.env.CREDENTIALS_KEY ?? "", context: CREDENTIALS_CONTEXT });

/** Empty string when the value cannot be opened (logged), so ciphertext is never sent on. */
export function openSecret(stored: string): string {
  try {
    return credentialsBox.decrypt(stored);
  } catch (error) {
    console.error(error);
    return "";
  }
}
