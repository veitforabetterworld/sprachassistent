import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const src = join(dirname(fileURLToPath(import.meta.url)), "../src");
const errors = [];

function walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      walk(path);
      continue;
    }
    if (!name.endsWith(".ts") && !name.endsWith(".tsx")) continue;
    const rel = relative(src, path).replaceAll("\\", "/");
    if (rel.startsWith("i18n/")) continue;
    const text = readFileSync(path, "utf8");
    if (text.includes('"de-DE"') || text.includes("'de-DE'")) {
      errors.push(`${rel}: Locale über @veit/intl, nicht hart de-DE`);
    }
  }
}

walk(src);
const de = readFileSync(join(src, "i18n/de.ts"), "utf8");
const en = readFileSync(join(src, "i18n/en.ts"), "utf8");
const keys = (source) => [...source.matchAll(/^\s{2}(\w+):/gm)].map((match) => match[1]).sort();
const deKeys = keys(de);
const enKeys = keys(en);
if (deKeys.join() !== enKeys.join()) {
  errors.push("i18n: de und en haben unterschiedliche Schlüssel");
}
if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
