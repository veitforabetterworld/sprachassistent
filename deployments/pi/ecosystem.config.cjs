// PM2 process on the Raspberry Pi (sprachassistent.plenivo.de). The binary reads .env from cwd.
const REPO = process.env.SPRACHASSISTENT_REPO || "/root/sprachassistent";

module.exports = {
  apps: [
    {
      name: "sprachassistent-api",
      cwd: REPO,
      script: `${REPO}/bin/sprachassistent`,
      interpreter: "none",
      max_memory_restart: "1500M",
      restart_delay: 5000,
      time: true,
    },
  ],
};
