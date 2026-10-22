// Narrivy Writing: settings shared by the service worker, popup and options.
// Stored in chrome.storage.local only (never synced to a cloud account).

export const DEFAULTS = Object.freeze({
  enabled: true,
  // Sites (host names) where checking is off.
  disabledSites: [],
  // Sensitive sites the user turned on anyway.
  allowedSensitiveSites: [],
  dialect: 'american',
  // Pause in typing before a check, in milliseconds.
  delayMs: 450,
  // Rule name -> on/off, over Harper's defaults.
  rules: {},
  // Personal dictionary.
  words: [],
  categories: { spelling: true, grammar: true, punctuation: true, style: true },
  aiActions: true,
  // Show the small Narrivy button next to selected text.
  selectionButton: true,
  // Show the badge with the issue count in the corner of a field.
  fieldBadge: true,
});

// Banking, payments, health records and government log-ins are off until
// the user turns them on for a site.
export const SENSITIVE_PATTERNS = [
  /(^|\.)(bank|banking)[a-z0-9-]*\./i,
  /bank\.(com|co\.uk|de|ca|com\.au)$/i,
  /(^|\.)(paypal|venmo|wise|revolut|stripe|klarna|coinbase|binance|kraken)\./i,
  /(^|\.)(chase|wellsfargo|bankofamerica|citi|capitalone|hsbc|barclays|santander|lloydsbank|natwest|monzo)\./i,
  /(^|\.)(mychart|patient|healthcare)[a-z0-9-]*\./i,
  /\.gov(\.[a-z]{2})?$/i,
  /(^|\.)(login|signin|auth|accounts)\./i,
];

export function isSensitive(host) {
  return SENSITIVE_PATTERNS.some((pattern) => pattern.test(host));
}

// Whether checking runs on `host`, and if not, why.
export function siteState(settings, host) {
  if (!settings.enabled) return { on: false, reason: 'off' };
  if (settings.disabledSites.includes(host)) return { on: false, reason: 'site' };
  if (isSensitive(host) && !settings.allowedSensitiveSites.includes(host)) {
    return { on: false, reason: 'sensitive' };
  }
  return { on: true, reason: null };
}

export async function load() {
  const stored = await chrome.storage.local.get('settings');
  return merge(stored.settings);
}

export function merge(stored) {
  const value = { ...DEFAULTS, ...(stored ?? {}) };
  value.categories = { ...DEFAULTS.categories, ...(stored?.categories ?? {}) };
  return value;
}

export async function save(patch) {
  const current = await load();
  const next = { ...current, ...patch };
  await chrome.storage.local.set({ settings: next });
  return next;
}

// Harper's categories, grouped the way the extension shows them.
export const CATEGORY_OF_KIND = {
  Spelling: 'spelling',
  Typo: 'spelling',
  Agreement: 'grammar',
  BoundaryError: 'grammar',
  Eggcorn: 'grammar',
  Grammar: 'grammar',
  Malapropism: 'grammar',
  Nonstandard: 'grammar',
  Repetition: 'grammar',
  Usage: 'grammar',
  WordChoice: 'grammar',
  WordOrder: 'grammar',
  Capitalization: 'punctuation',
  Formatting: 'punctuation',
  Punctuation: 'punctuation',
  Enhancement: 'style',
  Miscellaneous: 'style',
  Readability: 'style',
  Redundancy: 'style',
  Regionalism: 'style',
  Style: 'style',
};

export const DIALECTS = [
  ['american', 'English (US)'],
  ['british', 'English (UK)'],
  ['canadian', 'English (Canada)'],
  ['australian', 'English (Australia)'],
  ['indian', 'English (India)'],
];
