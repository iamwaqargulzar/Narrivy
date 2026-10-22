// Narrivy Writing: service worker.
//
// Owns the one native-messaging connection to Narrivy's host program and
// routes requests from content scripts, the popup and the options page to
// it. Text passes through and is never stored or logged here.

import { load, siteState } from './settings.js';

const HOST = 'com.narrivy.writing';

let port = null;
let nextId = 1;
// Message id -> { resolve, onDelta } for requests in flight.
const pending = new Map();
let lastError = null;

function connect() {
  if (port) return port;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (err) {
    lastError = 'host_missing';
    port = null;
    return null;
  }
  lastError = null;
  port.onMessage.addListener(onHostMessage);
  port.onDisconnect.addListener(() => {
    const reason = chrome.runtime.lastError?.message ?? '';
    lastError = /not found|forbidden/i.test(reason) ? 'host_missing' : 'host_stopped';
    port = null;
    for (const [id, entry] of pending) {
      entry.resolve({ id, type: 'error', code: lastError });
    }
    pending.clear();
  });
  return port;
}

function onHostMessage(message) {
  if (message.type === 'assistant') {
    broadcast({ kind: 'assistant-state', ...message });
    return;
  }
  const entry = pending.get(message.id);
  if (!entry) return;
  if (message.type === 'delta') {
    entry.onDelta?.(message.text);
    return;
  }
  if (message.type === 'accepted' && entry.streaming) {
    // The reply follows as deltas and one done.
    return;
  }
  pending.delete(message.id);
  entry.resolve(message);
}

// Send one message to the host and wait for its reply (or, for a writing
// action, its final `done`).
function ask(message, { onDelta, streaming = false, id = nextId++ } = {}) {
  return new Promise((resolve) => {
    const connected = connect();
    if (!connected) {
      resolve({ id, type: 'error', code: lastError ?? 'host_missing' });
      return;
    }
    pending.set(id, { resolve, onDelta, streaming });
    try {
      connected.postMessage({ ...message, id });
    } catch {
      pending.delete(id);
      resolve({ id, type: 'error', code: 'host_stopped' });
    }
  });
}

function broadcast(message) {
  chrome.tabs.query({}, (tabs) => {
    for (const tab of tabs) {
      if (tab.id !== undefined) chrome.tabs.sendMessage(tab.id, message).catch(() => {});
    }
  });
}

// One-shot requests from content scripts, the popup and options.
chrome.runtime.onMessage.addListener((request, sender, reply) => {
  switch (request.kind) {
    case 'check':
      ask({
        type: 'check',
        text: request.text,
        dialect: request.dialect,
        rules: request.rules,
        words: request.words,
      }).then(reply);
      return true;
    case 'status':
      ask({ type: 'status' }).then(reply);
      return true;
    case 'rules':
      ask({ type: 'rules' }).then(reply);
      return true;
    case 'open-options':
      chrome.runtime.openOptionsPage();
      reply({ ok: true });
      return false;
    case 'site-state':
      load().then((settings) => reply(siteState(settings, request.host)));
      return true;
    default:
      return false;
  }
});

// Writing actions stream, so they use a port per action.
chrome.runtime.onConnect.addListener((client) => {
  if (client.name !== 'narrivy-write') return;
  let id = null;
  client.onMessage.addListener((request) => {
    if (request.kind === 'start') {
      const message = {
        type: 'write',
        action: request.action,
        text: request.text,
        instruction: request.instruction,
      };
      id = nextId++;
      ask(message, {
        id,
        streaming: true,
        onDelta: (text) => safePost(client, { kind: 'delta', text }),
      }).then((done) => safePost(client, { kind: 'done', ...done }));
    } else if (request.kind === 'cancel' && id !== null) {
      ask({ type: 'cancel', target: id });
    }
  });
  client.onDisconnect.addListener(() => {
    if (id !== null && pending.has(id)) ask({ type: 'cancel', target: id });
  });
});

function safePost(client, message) {
  try {
    client.postMessage(message);
  } catch {
    // The page went away.
  }
}

// Right-click menu on selected text in editable fields.
const MENU = [
  ['fix', 'Fix spelling & grammar'],
  ['improve', 'Improve writing'],
  ['rewrite', 'Rewrite'],
  ['shorter', 'Make shorter'],
  ['longer', 'Make longer'],
  ['formal', 'More formal'],
  ['friendly', 'More friendly'],
  ['confident', 'More confident'],
  ['bullets', 'Turn into bullet points'],
  ['summarise', 'Summarise'],
];

chrome.runtime.onInstalled.addListener(() => {
  injectOpenTabs();
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: 'narrivy',
      title: 'Narrivy',
      contexts: ['selection'],
    });
    for (const [id, title] of MENU) {
      chrome.contextMenus.create({
        id: `narrivy-${id}`,
        parentId: 'narrivy',
        title,
        contexts: ['selection'],
      });
    }
    chrome.contextMenus.create({
      id: 'narrivy-custom',
      parentId: 'narrivy',
      title: 'Ask Narrivy…',
      contexts: ['selection'],
    });
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (!tab?.id || !String(info.menuItemId).startsWith('narrivy-')) return;
  const action = String(info.menuItemId).slice('narrivy-'.length);
  deliver(tab.id, info.frameId ?? 0, {
    kind: 'run-action',
    action,
    selectionText: info.selectionText ?? '',
  });
});

chrome.commands.onCommand.addListener((command, tab) => {
  if (command !== 'open-actions' || !tab?.id) return;
  deliver(tab.id, 0, { kind: 'open-actions' });
});

// Send a command to the page's script, adding the script first when the
// page was open before the extension was installed or reloaded.
async function deliver(tabId, frameId, message) {
  try {
    await chrome.tabs.sendMessage(tabId, message, { frameId });
    return;
  } catch {
    // No script in that page yet.
  }
  try {
    await inject(tabId, [frameId]);
    await chrome.tabs.sendMessage(tabId, message, { frameId });
  } catch {
    // A browser page or the store, where extensions cannot run.
    chrome.action.setBadgeBackgroundColor({ color: '#8a8a8a', tabId });
    chrome.action.setBadgeText({ text: '!', tabId });
    chrome.action.setTitle({ title: 'Narrivy Writing cannot run on this page', tabId });
    setTimeout(() => chrome.action.setBadgeText({ text: '', tabId }), 4000);
  }
}

async function inject(tabId, frameIds) {
  const target = frameIds ? { tabId, frameIds } : { tabId, allFrames: true };
  await chrome.scripting.insertCSS({ target, files: ['highlights.css'] });
  await chrome.scripting.executeScript({ target, files: ['content.js'] });
}

// Pages open before an install, update or reload have no script (or one
// that can no longer reach the extension): add it.
async function injectOpenTabs() {
  const tabs = await chrome.tabs.query({ url: ['http://*/*', 'https://*/*', 'file:///*'] });
  for (const tab of tabs) {
    if (tab.id !== undefined && !tab.discarded) inject(tab.id, null).catch(() => {});
  }
}
