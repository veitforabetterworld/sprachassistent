import express from "express";
import helmet from "helmet";
import { adminExists, apiKeys, auth, ensureAccountSecrets } from "./accounts.js";
import { adminRouter } from "./admin.js";
import { ensureSchema, pool } from "./db.js";
import { env } from "./env.js";
import { getSetting } from "./settings.js";

/**
 * Web API of the admin area. nginx sends /v1/talk and /v1/health to the Rust voice server and everything else
 * under /v1 here.
 */

await ensureSchema();
await ensureAccountSecrets();

const app = express();
app.set("trust proxy", "loopback");
app.use(helmet());
app.use(express.json({ limit: "1mb" }));
app.use(auth.attachUser);

// Only the very first account (the admin) may register, unless the admin area opens registration.
app.post("/v1/auth/register", async (_req, res, next) => {
  try {
    if ((await adminExists()) && (await getSetting("registration_open")) !== "1") {
      res.status(403).json({ error: "auth.registration_closed", message: "Registrierung ist geschlossen" });
      return;
    }
    next();
  } catch (error) {
    next(error);
  }
});

app.get("/v1/setup", async (_req, res) => {
  res.json({ admin_exists: await adminExists(), registration_open: (await getSetting("registration_open")) === "1" });
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
