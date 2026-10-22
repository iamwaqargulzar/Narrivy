// Narrivy Writing: toolbar popup.
import { DIALECTS, isSensitive, load, save } from './settings.js';

const $ = (id) => document.getElementById(id);

async function currentHost() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  try {
    const url = new URL(tab?.url ?? '');
    return url.protocol.startsWith('http') ? url.hostname : null;
  } catch {
    return null;
  }
}

async function render() {
  const settings = await load();
  const host = await currentHost();
  $('enabled').checked = settings.enabled;
  $('dialect').innerHTML = DIALECTS.map(
    ([value, label]) => `<option value="${value}" ${value === settings.dialect ? 'selected' : ''}>${label}</option>`,
  ).join('');
  const site = $('site');
  if (!host) {
    $('site-label').textContent = 'This page';
    $('site-note').textContent = 'Narrivy does not run on browser pages.';
    site.disabled = true;
  } else {
    $('site-label').textContent = host;
    const sensitive = isSensitive(host);
    const on = sensitive
      ? settings.allowedSensitiveSites.includes(host)
      : !settings.disabledSites.includes(host);
    site.checked = on && settings.enabled;
    site.disabled = !settings.enabled;
    $('site-note').textContent = sensitive
      ? 'Off by default: banking, payments, health and sign-in pages.'
      : on
        ? 'Checking while you type'
        : 'Off on this site';
  }
  site.onchange = async () => {
    const current = await load();
    if (isSensitive(host)) {
      const allowed = new Set(current.allowedSensitiveSites);
      site.checked ? allowed.add(host) : allowed.delete(host);
      await save({ allowedSensitiveSites: [...allowed] });
    } else {
      const disabled = new Set(current.disabledSites);
      site.checked ? disabled.delete(host) : disabled.add(host);
      await save({ disabledSites: [...disabled] });
    }
    render();
  };
}

async function status() {
  const reply = await chrome.runtime.sendMessage({ kind: 'status' });
  const light = $('light');
  if (reply?.type === 'status') {
    light.className = 'light good';
    $('status').textContent = 'Connected to Narrivy';
    const assistant = reply.assistant;
    $('model').textContent = assistant
      ? `AI actions: ${assistant.name}${assistant.state?.startsWith('loaded:') ? ` (on ${assistant.state.slice(7).toUpperCase()})` : ''}`
      : 'AI actions need an assistant model: Narrivy → Models → Language models.';
  } else {
    light.className = 'light bad';
    const code = reply?.code;
    $('status').textContent =
      code === 'host_missing' ? 'Browser connector not installed' : 'Narrivy is not running';
    $('model').textContent =
      code === 'host_missing'
        ? 'Install it from Narrivy, then restart the browser.'
        : 'Start Narrivy to check your writing.';
  }
}

$('enabled').onchange = async (event) => {
  await save({ enabled: event.target.checked });
  render();
};
$('dialect').onchange = (event) => save({ dialect: event.target.value });
$('options').onclick = () => chrome.runtime.openOptionsPage();

render();
status();
