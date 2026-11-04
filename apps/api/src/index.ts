import express from "express";
import helmet from "helmet";
import { getAuthUser } from "@veit/auth-server/express";
import { apiKeys, auth, ensureAccountSecrets, isAdmin, verificationRequired } from "./accounts.js";
import { adminRouter } from "./admin.js";
import { pool } from "./db.js";
import { env } from "./env.js";
import { pendingMigrations } from "./migrate.js";

/**
 * Web API of the admin area. nginx sends /v1/talk and /v1/health to the Rust voice server and everything else
 * under /v1 here.
 */

const pending = await pendingMigrations();
if (pending.length) {
  throw new Error(`Datenbank nicht migriert (${pending.join(", ")}): pnpm --filter @sprachassistent/api migrate`);
}
await ensureAccountSecrets();

const app = express();
app.set("trust proxy", "loopback");
app.use(helmet());
app.use(express.json({ limit: "1mb" }));
app.use(auth.attachUser);

app.get("/v1/ready", (_req, res) => {
  res.json({ ok: true });
});

const OPEN_WHILE_UNVERIFIED = new Set([
  "POST /v1/auth/login",
  "POST /v1/auth/login/mfa",
  "POST /v1/auth/login/mfa/email",
  "POST /v1/auth/logout",
  "POST /v1/auth/register",
  "POST /v1/auth/verify-email",
  "POST /v1/auth/verification/resend",
  "POST /v1/auth/password-reset",
  "POST /v1/auth/password-reset/confirm",
  "GET /v1/auth/session",
  "GET /v1/auth/suggest-password",
  "GET /v1/ready",
]);

app.use((req, res, next) => {
  verificationRequired()
    .then(async (required) => {
      if (!required) return next();
      const user = getAuthUser(req);
      if (!user || user.emailVerifiedAt || (await isAdmin(user.id))) return next();
      if (OPEN_WHILE_UNVERIFIED.has(`${req.method} ${req.path}`)) return next();
      res.status(403).json({ error: "auth.email_not_verified", code: "auth.email_not_verified" });
    })
    .catch(next);
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
