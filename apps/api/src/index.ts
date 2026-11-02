import express from "express";
import helmet from "helmet";
import { apiKeys, auth, ensureAccountSecrets } from "./accounts.js";
import { adminRouter } from "./admin.js";
import { ensureSchema, pool } from "./db.js";
import { env } from "./env.js";
import { migratePlaintextSecrets } from "./settings.js";

/**
 * Web API of the admin area. nginx sends /v1/talk and /v1/health to the Rust voice server and everything else
 * under /v1 here.
 */

await ensureSchema();
await ensureAccountSecrets();
await migratePlaintextSecrets();

const app = express();
app.set("trust proxy", "loopback");
app.use(helmet());
app.use(express.json({ limit: "1mb" }));
app.use(auth.attachUser);

app.get("/v1/ready", (_req, res) => {
  res.json({ ok: true });
});
app.use("/v1/admin", adminRouter());
app.use("/v1", auth.router);
app.use("/v1", apiKeys.router);

const server = app.listen(env.port, env.host, () => {
  console.log(`sprachassistent-web hört auf http://${env.host}:${env.port}`);
});

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    server.close();
    void pool.end();
  });
}
