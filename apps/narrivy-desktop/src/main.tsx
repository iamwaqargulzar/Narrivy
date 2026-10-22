import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import '@fontsource-variable/inter';
import '@fontsource/instrument-serif/400.css';
import './theme/tokens.css';
import './theme/app.css';

async function boot() {
  // `npm run dev` in a plain browser has no Tauri host: use the preview
  // bridge. Production builds drop this branch entirely.
  if (import.meta.env.DEV && !('__TAURI_INTERNALS__' in window)) {
    const { setPreviewBridge } = await import('./lib/bridge');
    // `?showcase`: invented content for the README screenshots.
    if (new URLSearchParams(window.location.search).has('showcase')) {
      const [{ createShowcaseBridge, usageHistory }, { seedUsageHistory }] = await Promise.all([
        import('./test/showcaseBridge'),
        import('./app/StatusStrip'),
      ]);
      seedUsageHistory(usageHistory());
      setPreviewBridge(createShowcaseBridge());
    } else {
      const { createPreviewBridge } = await import('./lib/previewBridge');
      setPreviewBridge(createPreviewBridge());
    }
  }
  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void boot();
