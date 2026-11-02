import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { execSync } from "node:child_process";
import path from "node:path";

let root;
try {
  root = execSync("git rev-parse --show-toplevel", { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
} catch {
  // A deploy target receives the tree by rsync without .git; there is no hook to install.
  process.exit(0);
}
const hooks = execSync("git rev-parse --git-path hooks", { encoding: "utf8" }).trim();
const destDir = path.resolve(root, hooks);
mkdirSync(destDir, { recursive: true });
const src = path.join(root, ".githooks", "pre-commit");
const dest = path.join(destDir, "pre-commit");
copyFileSync(src, dest);
chmodSync(dest, 0o755);
