// Errors reach the admin area as codes, which apps/web/src/errors.ts words in every language.
// This check fails on an error text written into apps/api or the voice server instead of a code, and on
// a code the web has no sentence for. Codes of @veit/auth-server (auth.*, api_keys.*) bring their own texts.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const CODE = /^[a-z_]+(\.[a-z_]+)*$/;
const OWN_TEXTS = /^(auth|api_keys)\./;

function files(dir, ext) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return files(path, ext);
    return path.endsWith(ext) ? [path] : [];
  });
}

const errors = [];
const used = new Map();

function use(code, where) {
  if (!CODE.test(code)) errors.push(`${where}: Fehlertext statt Code: "${code}"`);
  else if (!used.has(code)) used.set(code, where);
}

const sources = [
  // apps/api: new HttpError(status, "code"), json({ code: "..." }) and the old json({ error: "..." }).
  ...files(join(root, "apps/api/src"), ".ts").map((path) => ({
    path,
    patterns: [/new HttpError\(\s*\d+\s*,\s*"([^"]*)"/g, /json\(\{\s*(?:code|error):\s*"([^"]*)"/g],
  })),
  // Voice server: failure("code", ...) and ApiError::new(status, "code").
  ...files(join(root, "src"), ".rs").map((path) => ({
    path,
    patterns: [/\bfailure\(\s*"([^"]*)"/g, /ApiError::new\(\s*[\w:]+\s*,\s*"([^"]*)"/g],
  })),
];

for (const { path, patterns } of sources) {
  // Comments out, line breaks kept: rustfmt and prettier put the code on the next line.
  const text = readFileSync(path, "utf8").replace(/^[ \t]*(\/\/|\*).*$/gm, "");
  const rel = relative(root, path).replaceAll("\\", "/");
  for (const pattern of patterns) {
    for (const match of text.matchAll(pattern)) {
      use(match[1], `${rel}:${text.slice(0, match.index).split("\n").length}`);
    }
  }
}

const web = readFileSync(join(root, "apps/web/src/errors.ts"), "utf8");
const worded = new Set([...web.matchAll(/^\s+"?([a-z_.]+)"?: "err\w+"|^\s+"([a-z_.]+)": "\w+"/gm)].map((m) => m[1] ?? m[2]));
for (const [code, where] of used) {
  if (!OWN_TEXTS.test(code) && !worded.has(code)) errors.push(`${where}: Code ${code} fehlt in apps/web/src/errors.ts`);
}

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(`check-error-codes: ${used.size} Codes, alle mit Text.`);
