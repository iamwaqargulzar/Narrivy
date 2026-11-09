// Narrivy Writing: settings page.
import { DEFAULTS, DIALECTS, load, save } from './settings.js';

const $ = (id) => document.getElementById(id);
const esc = (text) =>
  String(text).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

let rules = [];

async function render() {
  const s = await load();
  for (const key of ['enabled', 'fieldBadge', 'aiActions', 'selectionButton']) $(key).checked = s[key];
  $('dialect').innerHTML = DIALECTS.map(
    ([value, label]) => `<option value="${value}" ${value === s.dialect ? 'selected' : ''}>${label}</option>`,
  ).join('');
  $('delayMs').value = s.delayMs;
  document.querySelectorAll('[data-category]').forEach((box) => {
    box.checked = s.categories[box.dataset.category] !== false;
  });
  tags($('words'), s.words, (word) => save({ words: s.words.filter((w) => w !== word) }).then(render));
  tags($('sites'), s.disabledSites, (site) =>
    save({ disabledSites: s.disabledSites.filter((w) => w !== site) }).then(render),
  );
  tags($('sensitive'), s.allowedSensitiveSites, (site) =>
    save({ allowedSensitiveSites: s.allowedSensitiveSites.filter((w) => w !== site) }).then(render),
  );
  renderRules(s);
}

function tags(container, values, remove) {
  container.innerHTML = values.length
    ? values
        .map((value, i) => `<span class="tag">${esc(value)}<button data-i="${i}" aria-label="Remove ${esc(value)}">×</button></span>`)
        .join('')
    : '<span class="muted">None</span>';
  container.querySelectorAll('button').forEach((button) =>
    button.addEventListener('click', () => remove(values[Number(button.dataset.i)])),
  );
}

function renderRules(s) {
  const box = $('rules');
  if (!rules.length) return;
  const query = $('rule-search').value.trim().toLowerCase();
  const shown = rules.filter(
    (rule) => !query || rule.name.toLowerCase().includes(query) || rule.description.toLowerCase().includes(query),
  );
  box.innerHTML = shown
    .map((rule) => {
      const on = s.rules[rule.name] ?? rule.enabled_by_default;
      const changed = rule.name in s.rules && s.rules[rule.name] !== rule.enabled_by_default;
      return `<div class="rule">
        <div class="rule-body"><div class="rule-name">${esc(spaced(rule.name))}${changed ? ' <span class="muted">· changed</span>' : ''}</div>
        <div class="rule-desc">${described(rule.description)}</div></div>
        <label class="switch"><input type="checkbox" data-rule="${esc(rule.name)}" ${on ? 'checked' : ''} aria-label="${esc(spaced(rule.name))}" /><span></span></label>
      </div>`;
    })
    .join('');
  box.querySelectorAll('[data-rule]').forEach((input) =>
    input.addEventListener('change', async () => {
      const current = await load();
      const name = input.dataset.rule;
      const rule = rules.find((r) => r.name === name);
      const next = { ...current.rules };
      if (input.checked === rule.enabled_by_default) delete next[name];
      else next[name] = input.checked;
      await save({ rules: next });
      renderRules(await load());
    }),
  );
}

const spaced = (name) =>
  name.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/([A-Z])([A-Z][a-z])/g, '$1 $2');
// Harper marks words in descriptions with backticks.
const described = (text) => esc(text).replace(/`([^`]+)`/g, '“$1”');

async function loadRules() {
  const reply = await chrome.runtime.sendMessage({ kind: 'rules' });
  if (reply?.type === 'rules') {
    rules = reply.rules;
    renderRules(await load());
  } else {
    $('rules').innerHTML = '<p class="muted">Start Narrivy to see and change the rules.</p>';
  }
}

async function status() {
  const reply = await chrome.runtime.sendMessage({ kind: 'status' });
  if (reply?.type === 'status') {
    $('light').className = 'light good';
    $('status').textContent = 'Connected to Narrivy';
    $('model').textContent = reply.assistant
      ? `AI actions use ${reply.assistant.name}.`
      : 'AI actions need an assistant model: Narrivy → Models → Language models.';
  } else {
    $('light').className = 'light bad';
    $('status').textContent =
      reply?.code === 'host_missing' ? 'Browser connector not installed' : 'Narrivy is not running';
    $('model').textContent = '';
  }
}

for (const key of ['enabled', 'fieldBadge', 'aiActions', 'selectionButton']) {
  $(key).addEventListener('change', (event) => save({ [key]: event.target.checked }));
}
$('dialect').addEventListener('change', (event) => save({ dialect: event.target.value }));
$('delayMs').addEventListener('change', (event) => {
  const value = Math.min(3000, Math.max(150, Number(event.target.value) || DEFAULTS.delayMs));
  save({ delayMs: value });
});
document.querySelectorAll('[data-category]').forEach((box) =>
  box.addEventListener('change', async () => {
    const current = await load();
    save({ categories: { ...current.categories, [box.dataset.category]: box.checked } });
  }),
);
$('word-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const word = $('word').value.trim();
  if (!word) return;
  const current = await load();
  await save({ words: Array.from(new Set([...current.words, word])) });
  $('word').value = '';
  render();
});
$('rule-search').addEventListener('input', async () => renderRules(await load()));
$('rules-reset').addEventListener('click', async () => {
  await save({ rules: {} });
  renderRules(await load());
});
$('shortcuts').addEventListener('click', () => chrome.tabs.create({ url: 'chrome://extensions/shortcuts' }));

render();
loadRules();
status();
