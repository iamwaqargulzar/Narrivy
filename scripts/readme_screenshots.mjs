#!/usr/bin/env node
// Take the README screenshots from the desktop UI's showcase mode.
//
//   npm --prefix apps/narrivy-desktop run dev -- --port 1420   (in another terminal)
//   node scripts/readme_screenshots.mjs --chrome /path/to/chrome-headless-shell
//
// The showcase (`?showcase`) holds invented content only, so no one's real
// dictations, meetings or chats can end up in a picture. Pictures are written
// to docs/images/ as PNG at 1440x900 (2x for sharpness).
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((pairs, arg, i, all) => (arg.startsWith('--') ? [...pairs, [arg.slice(2), all[i + 1]]] : pairs), []),
);
const chrome = args.chrome;
if (!chrome) throw new Error('--chrome PATH is required');
const base = args.url ?? 'http://localhost:1420/?showcase';
const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'docs/images');
mkdirSync(out, { recursive: true });

const PORT = 9333;
const profile = mkdtempSync(join(tmpdir(), 'narrivy-shots-'));
const browser = spawn(chrome, [
  `--remote-debugging-port=${PORT}`,
  `--user-data-dir=${profile}`,
  '--no-first-run',
  '--hide-scrollbars',
  '--window-size=1440,900',
  'about:blank',
]);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let target;
for (let i = 0; i < 50 && !target; i += 1) {
  await sleep(200);
  try {
    const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
    target = list.find((t) => t.type === 'page');
  } catch {
    // Not up yet.
  }
}
if (!target) throw new Error('the browser did not start');

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve) => socket.addEventListener('open', resolve, { once: true }));
let id = 0;
const pending = new Map();
socket.addEventListener('message', (message) => {
  const data = JSON.parse(message.data);
  if (data.id && pending.has(data.id)) {
    pending.get(data.id)(data);
    pending.delete(data.id);
  }
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    id += 1;
    pending.set(id, (data) => (data.error ? reject(new Error(`${method}: ${data.error.message}`)) : resolve(data.result)));
    socket.send(JSON.stringify({ id, method, params }));
  });
const run = async (expression) => {
  const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? expression);
  return result.result.value;
};
// Click the first visible button or link whose text or label matches,
// waiting up to five seconds for it to appear.
const click = async (text) => {
  for (let i = 0; ; i += 1) {
    try {
      return await clickNow(text);
    } catch (error) {
      if (i >= 25) throw error;
      await sleep(200);
    }
  }
};
const clickNow = (text) =>
  run(`(() => {
    const want = ${JSON.stringify(text)};
    const all = [...document.querySelectorAll('button, a, [role="tab"], [role="button"]')];
    const hit = all.find((el) => (el.getAttribute('aria-label') === want || el.textContent.trim() === want) && el.offsetParent)
      ?? all.find((el) => el.textContent.trim().startsWith(want) && el.offsetParent);
    if (!hit) throw new Error('nothing to click: ' + want);
    hit.click();
    return true;
  })()`);
const shoot = async (name) => {
  await sleep(900);
  const { data } = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(out, `${name}.png`), Buffer.from(data, 'base64'));
  console.log(`docs/images/${name}.png`);
};

await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 2, mobile: false });
const theme = args.theme ?? 'dark';
await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
await send('Page.navigate', { url: base });
await sleep(1500);
await run(`localStorage.setItem('narrivy.onboarded.v1', '1'); localStorage.setItem('narrivy.theme', '${theme}'); true`);
await send('Page.navigate', { url: base });
// Let the status bar gather a little usage history for its graphs.
await sleep(12_000);

await shoot('home');
await click('History');
await shoot('history');
await click('Pipelines');
await shoot('pipelines');
await click('Models');
await shoot('models');
await click('Store');
await shoot('store');
await click('Assistant');
await sleep(500);
await click('Quiet keyboards under $120');
await shoot('assistant');
await click('Meetings');
await sleep(500);
await click('Weekly product sync');
await shoot('meeting');
await click('Notes');
await shoot('meeting-notes');

socket.close();
browser.kill();
