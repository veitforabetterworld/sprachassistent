import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Dev: /v1/talk and /v1/health go to the voice server, the rest of /v1 to apps/api (like nginx on the Pi).
const proxy = {
  "/v1/talk": { target: "http://127.0.0.1:3020" },
  "/v1/health": { target: "http://127.0.0.1:3020" },
  "/v1": { target: "http://127.0.0.1:3022" },
};

export default defineConfig({
  plugins: [tailwindcss(), react()],
  resolve: {
    dedupe: ["react", "react-dom", "react-router-dom"],
  },
  server: { port: 5173, strictPort: true, proxy },
  preview: { port: 4173, strictPort: true, proxy },
});
