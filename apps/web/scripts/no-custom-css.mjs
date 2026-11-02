import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const runner = join(here, "../../../veit-framework/scripts/no-custom-css.mjs");
// Tailwind scans the framework modules the app renders (src/tailwind.css lists the same).
const sources = [
  "./**/*.{tsx,ts}",
  "../../../veit-framework/packages/react-controls/src",
  "../../../veit-framework/packages/react-dialog/src",
  "../../../veit-framework/packages/react-auth/src",
  "../../../veit-framework/packages/react-account/src",
  "../../../veit-framework/packages/react-api-keys/src",
].flatMap((glob) => ["--source", glob]);
const result = spawnSync(process.execPath, [runner, join(here, "../src"), ...sources], { stdio: "inherit" });
process.exit(result.status ?? 1);
