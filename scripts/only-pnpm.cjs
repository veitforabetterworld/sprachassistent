const ua = process.env.npm_config_user_agent ?? "";
if (!ua.includes("pnpm")) {
  console.error("This repo uses pnpm. Run: pnpm install");
  process.exit(1);
}
