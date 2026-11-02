import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
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
    if (!name.endsWith(".tsx")) continue;
    const text = readFileSync(path, "utf8");
    const rel = relative(src, path);
    let from = 0;
    while (true) {
      const start = text.indexOf("<VeitSheetDialog", from);
      if (start < 0) break;
      let end = text.length;
      for (let i = start; i < text.length; i++) {
        if (text[i] === "=" && text[i + 1] === ">") {
          i += 1;
          continue;
        }
        if (text[i] === ">") {
          end = i;
          break;
        }
      }
      const tag = text.slice(start, end);
      if (!tag.includes("dismissRef")) {
        errors.push(`${rel}: VeitSheetDialog braucht dismissRef`);
      }
      from = start + 1;
    }
  }
}

walk(src);
if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
