import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const reviewed = new Set(
  JSON.parse(readFileSync(new URL("./audit-reviewed.json", import.meta.url), "utf8")).map((entry) => String(entry.id)),
);

const result = spawnSync("pnpm", ["audit", "--json"], { encoding: "utf8" });
const data = JSON.parse(result.stdout || "{}");
const blocking = [];

for (const [id, advisory] of Object.entries(data.advisories ?? {})) {
  const line = `${advisory.severity} ${id} ${advisory.module_name}: ${advisory.title}`;
  if (advisory.severity === "low" || advisory.severity === "moderate") {
    console.log(line);
    continue;
  }
  if (reviewed.has(id)) {
    console.log(`reviewed ${line}`);
    continue;
  }
  blocking.push(line);
}

if (blocking.length > 0) {
  console.error(blocking.join("\n"));
  process.exit(1);
}
