import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// No credentials in git: .env files, keys, certificates and the firmware secrets.
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const runner = join(root, "veit-framework/scripts/check-no-tracked-secrets.mjs");
const result = spawnSync(
  process.execPath,
  [
    runner,
    "--name-re",
    "(^|/)\.env($|\.)",
    "--name-re",
    "(^|/)id_rsa($|\.)",
    "--name-re",
    "\.(pem|p12|pfx)$",
    "--name-re",
    "(^|/)credentials\.(json|ya?ml)$",
    "--name-re",
    "(^|/)secrets\.h$",
  ],
  { stdio: "inherit", cwd: root },
);
process.exit(result.status ?? 1);
