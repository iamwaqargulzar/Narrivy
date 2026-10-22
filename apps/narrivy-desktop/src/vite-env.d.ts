/// <reference types="vite/client" />
declare module '*.css?raw' {
  const content: string;
  export default content;
}

/** Desktop app version, injected at build time from tauri.conf.json. */
declare const __APP_VERSION__: string;
