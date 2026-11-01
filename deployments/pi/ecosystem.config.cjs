// PM2 processes on the Raspberry Pi (sprachassistent.plenivo.de). Both read .env (DATABASE_URL) from the repo root.
const REPO = process.env.SPRACHASSISTENT_REPO || "/root/sprachassistent";

module.exports = {
  apps: [
    {
      // Rust: speech recognition, AI, speech output; /v1/talk and /v1/health.
      name: "sprachassistent-voice",
      cwd: REPO,
      script: `${REPO}/bin/sprachassistent`,
      interpreter: "none",
      max_memory_restart: "1500M",
      restart_delay: 5000,
      time: true,
    },
    {
      // Node: accounts, ESP keys and admin area; the rest of /v1.
      name: "sprachassistent-web",
      cwd: REPO,
      script: `${REPO}/apps/api/dist/index.js`,
      env: { NODE_ENV: "production" },
      max_memory_restart: "400M",
      restart_delay: 5000,
      time: true,
    },
  ],
};
