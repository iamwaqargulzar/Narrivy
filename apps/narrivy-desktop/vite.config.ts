/// <reference types="vitest" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';

// The version the desktop app reports is the Tauri app version, so the UI and
// the host can never disagree about it.
const appVersion = (JSON.parse(readFileSync(new URL('./src-tauri/tauri.conf.json', import.meta.url), 'utf-8')) as {
  version: string;
}).version;

// Tauri expects a fixed dev port; keep Vite quiet and predictable.
export default defineConfig({
  plugins: [react()],
  define: { __APP_VERSION__: JSON.stringify(appVersion) },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_'],
  build: { target: 'es2021' },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    css: false,
  },
});
