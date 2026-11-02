import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const src = join(dirname(fileURLToPath(import.meta.url)), "../src/pages");
const warnAt = 600;
const failAt = 1000;
const failures = [];

function walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      walk(path);
      continue;
    }
    if (!name.endsWith(".tsx")) continue;
    const lines = readFileSync(path, "utf8").split("\n").length;
    const rel = relative(src, path);
    if (lines >= failAt) failures.push(`${rel}: ${lines} Zeilen (Limit ${failAt})`);
    else if (lines >= warnAt) console.warn(`page-loc warn ${rel}: ${lines}`);
  }
}

walk(src);
if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
