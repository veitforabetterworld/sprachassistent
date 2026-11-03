import { createPool } from "@veit/db";
import { env } from "./env.js";

export const pool = createPool({ connectionString: env.databaseUrl });
