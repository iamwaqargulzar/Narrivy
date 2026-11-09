// Narrivy Writing: content script.
//
// Finds the editable field being typed in, sends its text to Narrivy for a
// rule check after a pause in typing, underlines what was found, and offers
// fixes and AI writing actions. Everything the extension draws lives in one
// closed shadow root; only the ::highlight() rules in highlights.css touch
// the page. Text goes to the service worker and Narrivy's native host only.
(() => {
  // A copy left by an earlier version of the extension (after an update or
  // reload) can no longer reach it: take its place.
  // An old copy may live in another script world, so also drop its UI.
  for (const old of document.querySelectorAll('narrivy-writing')) old.remove();
  const previous = window.__narrivyWriting;
  if (previous?.alive()) return;
  previous?.retire();
  window.__narrivyWriting = {
    alive: () => Boolean(chrome.runtime?.id),
    retire: () => {
      siteOn = false;
      host.remove();
      if (typeof Highlight === 'function' && CSS.highlights) {
        for (const name of [...CSS.highlights.keys()]) {
          if (name.startsWith('narrivy-')) CSS.highlights.delete(name);
        }
      }
    },
  };

  // ---------------------------------------------------------------- settings

  const DEFAULTS = {
    enabled: true,
    disabledSites: [],
    allowedSensitiveSites: [],
    dialect: 'american',
    delayMs: 450,
    rules: {},
    words: [],
    categories: { spelling: true, grammar: true, punctuation: true, style: true },
    aiActions: true,
    selectionButton: true,
    fieldBadge: true,
  };

  const CATEGORY_OF_KIND = {
    Spelling: 'spelling', Typo: 'spelling',
    Agreement: 'grammar', BoundaryError: 'grammar', Eggcorn: 'grammar', Grammar: 'grammar',
    Malapropism: 'grammar', Nonstandard: 'grammar', Repetition: 'grammar', Usage: 'grammar',
    WordChoice: 'grammar', WordOrder: 'grammar',
    Capitalization: 'punctuation', Formatting: 'punctuation', Punctuation: 'punctuation',
    Enhancement: 'style', Miscellaneous: 'style', Readability: 'style', Redundancy: 'style',
    Regionalism: 'style', Style: 'style',
  };
  const CATEGORY_LABEL = {
    spelling: 'Spelling',
    grammar: 'Grammar',
    punctuation: 'Punctuation',
    style: 'Clarity',
  };
  const CATEGORY_COLOR = {
    spelling: '#e5484d',
    grammar: '#f08c00',
    punctuation: '#f08c00',
    style: '#3e7bfa',
  };

  const ACTIONS = [
    ['fix', 'Fix'],
    ['improve', 'Improve'],
    ['rewrite', 'Rewrite'],
    ['shorter', 'Shorter'],
    ['longer', 'Longer'],
    ['formal', 'Formal'],
    ['friendly', 'Friendly'],
    ['confident', 'Confident'],
    ['bullets', 'Bullets'],
    ['summarise', 'Summarise'],
  ];
  const ACTION_TITLE = Object.fromEntries(ACTIONS);
  ACTION_TITLE.custom = 'Custom';

  const MAX_TEXT = 60000;

  let settings = { ...DEFAULTS };
  let siteOn = false;
  // `${rule}|${text}` ignored for this page.
  const ignored = new Set();

  function mergeSettings(stored) {
    const value = { ...DEFAULTS, ...(stored ?? {}) };
    value.categories = { ...DEFAULTS.categories, ...(stored?.categories ?? {}) };
    return value;
  }

  async function refreshSettings() {
    try {
      const stored = await chrome.storage.local.get('settings');
      settings = mergeSettings(stored.settings);
      const state = await chrome.runtime.sendMessage({ kind: 'site-state', host: location.hostname });
      siteOn = Boolean(state?.on);
    } catch {
      siteOn = false;
    }
    if (!siteOn) {
      for (const field of fields) field.detach();
      hideAllUi();
    } else {
      for (const field of fields) field.check(true);
    }
  }

  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.settings) refreshSettings();
  });

  // -------------------------------------------------------------- eligibility

  const SECRET_AUTOCOMPLETE = /cc-|one-time-code|password/;
  const SECRET_NAME =
    /pass(word|wd)?\b|\bpwd|\botp\b|one.?time|cvv|cvc|card.?(number|no)|iban|\bssn\b|security.?code|\bpin\b/i;
  const CODE_EDITOR = '.cm-editor, .CodeMirror, .monaco-editor, .ace_editor, [role="code"]';
  const OPTED_OUT =
    '[data-narrivy="off"], [data-gramm="false"], [data-gramm_editor="false"], [data-enable-grammarly="false"]';

  function isSecret(el) {
    const autocomplete = (el.getAttribute('autocomplete') || '').toLowerCase();
    if (SECRET_AUTOCOMPLETE.test(autocomplete)) return true;
    const label = [
      el.getAttribute('name'),
      el.id,
      el.getAttribute('aria-label'),
      el.getAttribute('placeholder'),
    ]
      .filter(Boolean)
      .join(' ');
    return SECRET_NAME.test(label);
  }

  // The field to check for `target`, or null when it must not be read.
  function eligibleField(target) {
    if (!target || target.nodeType !== 1) return null;
    if (target.closest(OPTED_OUT) || target.closest(CODE_EDITOR)) return null;
    if (target instanceof HTMLTextAreaElement) {
      if (target.readOnly || target.disabled || isSecret(target)) return null;
      return target;
    }
    if (target instanceof HTMLInputElement) {
      const type = (target.getAttribute('type') || 'text').toLowerCase();
      if (type !== 'text') return null;
      const role = target.getAttribute('role');
      if (role === 'combobox' || role === 'searchbox') return null;
      if (target.readOnly || target.disabled || isSecret(target)) return null;
      return target;
    }
    if (target.isContentEditable) {
      let root = target;
      while (root.parentElement && root.parentElement.isContentEditable) root = root.parentElement;
      if (root.closest(CODE_EDITOR) || root.closest(OPTED_OUT) || isSecret(root)) return null;
      return root;
    }
    return null;
  }

  const isControl = (el) => el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement;

  // ------------------------------------------------------- rich text mapping

  const BLOCK = /^(P|DIV|LI|H[1-6]|BLOCKQUOTE|PRE|TR|SECTION|ARTICLE|UL|OL|TABLE|HEADER|FOOTER|FIGCAPTION|DD|DT)$/;

  // The text of a contenteditable root as the user sees it, with the text
  // node behind every stretch, so offsets map back to DOM positions.
  function readRich(root) {
    const parts = [];
    let text = '';
    const newline = () => {
      if (text.length && !text.endsWith('\n')) text += '\n';
    };
    const walk = (node) => {
      for (const child of node.childNodes) {
        if (child.nodeType === Node.TEXT_NODE) {
          const value = child.nodeValue;
          if (!value) continue;
          parts.push({ node: child, start: text.length, end: text.length + value.length });
          text += value;
        } else if (child.nodeType === Node.ELEMENT_NODE) {
          const tag = child.tagName;
          if (tag === 'BR') {
            text += '\n';
            continue;
          }
          if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'TEMPLATE' || child.hidden) continue;
          if (child.getAttribute('contenteditable') === 'false') {
            // A mention or chip: keep words apart without reading it.
            text += ' ';
            continue;
          }
          const block = BLOCK.test(tag);
          if (block) newline();
          walk(child);
          if (block) newline();
        }
      }
    };
    walk(root);
    return { text, parts };
  }

  function domPoint(parts, offset, preferNext) {
    let lo = 0;
    let hi = parts.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const part = parts[mid];
      if (offset < part.start) hi = mid - 1;
      else if (offset > part.end) lo = mid + 1;
      else {
        if (offset === part.end && preferNext && parts[mid + 1]?.start === offset) {
          return { node: parts[mid + 1].node, offset: 0 };
        }
        return { node: part.node, offset: offset - part.start };
      }
    }
    // Inside a block break: snap to the next text.
    const next = parts.find((part) => part.start >= offset);
    if (next) return { node: next.node, offset: 0 };
    const last = parts[parts.length - 1];
    return last ? { node: last.node, offset: last.node.nodeValue.length } : null;
  }

  function richRange(parts, start, end) {
    const from = domPoint(parts, start, true);
    const to = domPoint(parts, end, false);
    if (!from || !to) return null;
    try {
      const range = document.createRange();
      range.setStart(from.node, from.offset);
      range.setEnd(to.node, to.offset);
      return range;
    } catch {
      return null;
    }
  }

  // ------------------------------------------------- text control geometry

  const MIRRORED = [
    'boxSizing', 'width', 'height', 'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft',
    'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth', 'borderTopStyle',
    'borderRightStyle', 'borderBottomStyle', 'borderLeftStyle', 'fontFamily', 'fontSize',
    'fontWeight', 'fontStyle', 'fontVariant', 'fontStretch', 'fontFeatureSettings', 'letterSpacing',
    'wordSpacing', 'lineHeight', 'textTransform', 'textIndent', 'textAlign', 'tabSize', 'direction',
    'overflowWrap', 'wordBreak', 'hyphens',
  ];

  // Rectangles (viewport coordinates) of [start, end) inside a textarea or
  // input, measured on an invisible copy laid out exactly like it.
  function controlRects(el, mirror, spans) {
    const style = getComputedStyle(el);
    const box = el.getBoundingClientRect();
    for (const key of MIRRORED) mirror.style[key] = style[key];
    mirror.style.borderColor = 'transparent';
    mirror.style.left = `${box.left}px`;
    mirror.style.top = `${box.top}px`;
    mirror.style.width = `${box.width}px`;
    mirror.style.height = `${box.height}px`;
    mirror.style.boxSizing = 'border-box';
    const textarea = el instanceof HTMLTextAreaElement;
    mirror.style.whiteSpace = textarea ? 'pre-wrap' : 'pre';
    mirror.style.overflow = 'hidden';
    const value = el.value;
    mirror.textContent = textarea ? `${value}​` : value;
    mirror.scrollTop = el.scrollTop;
    mirror.scrollLeft = el.scrollLeft;
    const node = mirror.firstChild;
    const out = [];
    for (const [start, end] of spans) {
      if (!node || end > value.length) {
        out.push([]);
        continue;
      }
      const range = document.createRange();
      range.setStart(node, start);
      range.setEnd(node, Math.max(start, end));
      out.push(Array.from(range.getClientRects()).filter((r) => r.width > 0 || end === start));
    }
    return out;
  }

  // The inner box text is drawn in, for clipping underlines.
  function contentBox(el) {
    const box = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const left = box.left + parseFloat(style.borderLeftWidth);
    const top = box.top + parseFloat(style.borderTopWidth);
    return {
      left,
      top,
      right: box.right - parseFloat(style.borderRightWidth),
      bottom: box.bottom - parseFloat(style.borderBottomWidth),
    };
  }

  // ------------------------------------------------------------------ the UI

  const host = document.createElement('narrivy-writing');
  host.style.cssText = 'all: initial; position: fixed; inset: 0 auto auto 0; z-index: 2147483647;';
  const shadow = host.attachShadow({ mode: 'closed' });
  shadow.innerHTML = `<style>${uiCss()}</style>
    <div class="mirror" aria-hidden="true"></div>
    <div class="layer" aria-hidden="true"></div>
    <div class="card" role="dialog" aria-label="Narrivy suggestion" hidden></div>
    <div class="panel" role="dialog" aria-label="Narrivy" hidden></div>
    <div class="menu" role="menu" aria-label="Narrivy writing actions" hidden></div>
    <div class="preview" role="dialog" aria-label="Narrivy result" hidden></div>
    <div class="toast" role="status" hidden></div>`;
  const mirror = shadow.querySelector('.mirror');
  const layer = shadow.querySelector('.layer');
  const card = shadow.querySelector('.card');
  const panel = shadow.querySelector('.panel');
  const menu = shadow.querySelector('.menu');
  const preview = shadow.querySelector('.preview');
  const toast = shadow.querySelector('.toast');
  let mounted = false;

  function mount() {
    if (mounted) return;
    document.documentElement.appendChild(host);
    mounted = true;
  }

  function hideAllUi() {
    card.hidden = true;
    panel.hidden = true;
    menu.hidden = true;
    if (!preview.dataset.busy) preview.hidden = true;
  }

  // Harper marks words in its messages with backticks.
  const message = (text) => esc(text).replace(/`([^`]+)`/g, '<b>“$1”</b>');

  const esc = (text) =>
    String(text).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

  function place(el, anchor, { below = true, gap = 6 } = {}) {
    el.style.visibility = 'hidden';
    el.hidden = false;
    const width = el.offsetWidth;
    const height = el.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left = Math.min(Math.max(8, anchor.left), vw - width - 8);
    let top = below ? anchor.bottom + gap : anchor.top - height - gap;
    if (below && top + height > vh - 8) top = anchor.top - height - gap;
    if (!below && top < 8) top = anchor.bottom + gap;
    top = Math.min(Math.max(8, top), vh - height - 8);
    left = Math.max(8, left);
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
    el.style.visibility = '';
  }

  // ------------------------------------------------------------------ fields

  const fields = new Set();
  const fieldOf = new WeakMap();
  const highlights = {};
  const highlightSupported = typeof Highlight === 'function' && CSS.highlights;

  class Field {
    constructor(el) {
      this.el = el;
      this.control = isControl(el);
      this.findings = [];
      this.text = '';
      this.parts = [];
      this.version = 0;
      this.timer = 0;
      this.status = 'idle';
      this.rects = [];
      this.badge = document.createElement('button');
      this.badge.className = 'badge';
      this.badge.type = 'button';
      this.badge.setAttribute('aria-label', 'Narrivy writing suggestions');
      this.badge.addEventListener('mousedown', (e) => e.preventDefault());
      this.badge.addEventListener('click', () => openPanel(this));
      this.lines = document.createElement('div');
      this.lines.className = 'lines';
      layer.append(this.lines, this.badge);
      this.observer = new ResizeObserver(() => scheduleLayout());
      this.observer.observe(el);
    }

    read() {
      if (this.control) {
        this.text = this.el.value;
        this.parts = [];
      } else {
        const { text, parts } = readRich(this.el);
        this.text = text;
        this.parts = parts;
      }
      return this.text;
    }

    schedule() {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.check(), settings.delayMs);
    }

    async check(force = false) {
      clearTimeout(this.timer);
      if (!siteOn || !this.el.isConnected) return;
      const before = this.text;
      const text = this.read();
      if (!force && text === before && this.status === 'ok') {
        this.render();
        return;
      }
      if (!text.trim() || text.length > MAX_TEXT) {
        this.findings = [];
        this.status = 'ok';
        this.render();
        return;
      }
      const version = ++this.version;
      this.status = 'checking';
      this.renderBadge();
      let reply;
      try {
        reply = await chrome.runtime.sendMessage({
          kind: 'check',
          text,
          dialect: settings.dialect,
          rules: settings.rules,
          words: settings.words,
        });
      } catch {
        reply = { type: 'error', code: 'extension_reloaded' };
      }
      if (version !== this.version) return;
      if (reply?.type !== 'findings') {
        this.status = `error:${reply?.code ?? 'unknown'}`;
        this.findings = [];
        this.render();
        return;
      }
      // The text may have changed while the check ran; findings for an
      // older text would point at the wrong words.
      if (this.read() !== text) {
        this.schedule();
        return;
      }
      this.status = 'ok';
      this.findings = reply.findings
        .map((finding) => ({ ...finding, category: CATEGORY_OF_KIND[finding.kind] ?? 'style' }))
        .filter((finding) => settings.categories[finding.category] !== false)
        .filter((finding) => !ignored.has(ignoreKey(finding, text)));
      this.render();
    }

    rangeFor(start, end) {
      return richRange(this.parts, start, end);
    }

    // Viewport rectangles of every finding, in findings order.
    measure() {
      if (this.control) {
        this.rects = controlRects(
          this.el,
          mirror,
          this.findings.map((f) => [f.start, f.end]),
        );
      } else {
        this.rects = this.findings.map((f) => {
          const range = this.rangeFor(f.start, f.end);
          return range ? Array.from(range.getClientRects()) : [];
        });
      }
      return this.rects;
    }

    render() {
      if (!this.control) refreshHighlights();
      this.layout();
      if (panel.dataset.field === this.id() && !panel.hidden) openPanel(this);
    }

    id() {
      if (!this.uid) this.uid = String(Math.random()).slice(2);
      return this.uid;
    }

    layout() {
      this.measure();
      if (this.control) this.drawLines();
      this.renderBadge();
    }

    drawLines() {
      const clip = contentBox(this.el);
      const pieces = [];
      this.findings.forEach((finding, i) => {
        for (const rect of this.rects[i] ?? []) {
          const left = Math.max(rect.left, clip.left);
          const right = Math.min(rect.right, clip.right);
          const bottom = rect.bottom;
          if (right <= left || bottom <= clip.top || bottom - 2 > clip.bottom) continue;
          pieces.push(
            `<div class="line ${finding.category}" style="left:${left}px;top:${bottom - 3}px;width:${right - left}px"></div>`,
          );
        }
      });
      this.lines.innerHTML = pieces.join('');
    }

    renderBadge() {
      const badge = this.badge;
      const focused = activeField === this || panel.dataset.field === this.id();
      if (!settings.fieldBadge || !focused || !this.el.isConnected) {
        badge.hidden = true;
        return;
      }
      const box = this.el.getBoundingClientRect();
      if (box.width < 60 || box.height < 18 || box.bottom < 0 || box.top > window.innerHeight) {
        badge.hidden = true;
        return;
      }
      badge.hidden = false;
      const size = box.height < 40 ? 18 : 22;
      badge.style.width = `${size}px`;
      badge.style.height = `${size}px`;
      badge.style.left = `${box.right - size - 6}px`;
      badge.style.top = `${box.bottom - size - 6}px`;
      const count = this.findings.length;
      const serious = this.findings.some((f) => f.category !== 'style');
      badge.className = `badge ${this.status === 'checking' ? 'busy' : ''} ${
        this.status.startsWith('error') ? 'error' : count ? (serious ? 'issues' : 'style') : 'clean'
      }`;
      badge.textContent = this.status.startsWith('error') ? '!' : count ? String(Math.min(count, 99)) : '';
      badge.title = this.status.startsWith('error')
        ? errorText(this.status.slice(6))
        : count
          ? `${count} suggestion${count === 1 ? '' : 's'}`
          : 'No issues found';
    }

    findingAt(x, y) {
      for (let i = 0; i < this.findings.length; i++) {
        for (const rect of this.rects[i] ?? []) {
          if (x >= rect.left - 1 && x <= rect.right + 1 && y >= rect.top - 2 && y <= rect.bottom + 3) {
            return { finding: this.findings[i], rect };
          }
        }
      }
      return null;
    }

    detach() {
      clearTimeout(this.timer);
      this.findings = [];
      this.lines.innerHTML = '';
      this.badge.hidden = true;
      if (!this.control) refreshHighlights();
    }

    destroy() {
      this.detach();
      this.observer.disconnect();
      this.lines.remove();
      this.badge.remove();
      fields.delete(this);
    }
  }

  function ignoreKey(finding, text) {
    return `${finding.rule}|${text.slice(finding.start, finding.end).toLowerCase()}`;
  }

  function refreshHighlights() {
    if (!highlightSupported) return;
    const byCategory = { spelling: [], grammar: [], punctuation: [], style: [] };
    for (const field of fields) {
      if (field.control || !field.el.isConnected) continue;
      for (const finding of field.findings) {
        const range = field.rangeFor(finding.start, finding.end);
        if (range) byCategory[finding.category].push(range);
      }
    }
    for (const [category, ranges] of Object.entries(byCategory)) {
      const name = `narrivy-${category}`;
      if (!highlights[name]) {
        highlights[name] = new Highlight();
        CSS.highlights.set(name, highlights[name]);
      }
      highlights[name].clear();
      for (const range of ranges) highlights[name].add(range);
    }
  }

  function setActiveHighlight(range) {
    if (!highlightSupported) return;
    if (!highlights['narrivy-active']) {
      highlights['narrivy-active'] = new Highlight();
      CSS.highlights.set('narrivy-active', highlights['narrivy-active']);
    }
    highlights['narrivy-active'].clear();
    if (range) highlights['narrivy-active'].add(range);
  }

  function fieldFor(el) {
    let field = fieldOf.get(el);
    if (!field || !fields.has(field)) {
      mount();
      field = new Field(el);
      fieldOf.set(el, field);
      fields.add(field);
    }
    return field;
  }

  let layoutQueued = false;
  function scheduleLayout() {
    if (layoutQueued) return;
    layoutQueued = true;
    requestAnimationFrame(() => {
      layoutQueued = false;
      for (const field of fields) {
        if (!field.el.isConnected) {
          field.destroy();
          continue;
        }
        if (field.findings.length || field === activeField) field.layout();
      }
      if (!card.hidden && cardState) hideCard();
    });
  }

  // ------------------------------------------------------------- page events

  let activeField = null;

  function targetOf(event) {
    const path = event.composedPath?.();
    return (path && path[0]) || event.target;
  }

  document.addEventListener(
    'focusin',
    (event) => {
      if (!siteOn) return;
      const el = eligibleField(targetOf(event));
      const previous = activeField;
      activeField = el ? fieldFor(el) : null;
      if (previous && previous !== activeField) previous.renderBadge();
      if (activeField) {
        if (activeField.status === 'idle') activeField.check();
        else activeField.layout();
      }
    },
    true,
  );

  document.addEventListener(
    'focusout',
    () => {
      setTimeout(() => {
        const el = eligibleField(document.activeElement);
        if (!el && activeField && panel.hidden && menu.hidden) {
          const leaving = activeField;
          activeField = null;
          leaving.renderBadge();
        }
      }, 120);
    },
    true,
  );

  document.addEventListener(
    'input',
    (event) => {
      if (!siteOn) return;
      const el = eligibleField(targetOf(event));
      if (!el) return;
      const field = fieldFor(el);
      activeField = field;
      // Old underlines no longer match the text: hide them until the check.
      field.status = 'stale';
      if (field.findings.length) {
        field.findings = [];
        field.render();
      }
      hideCard();
      field.schedule();
    },
    true,
  );

  window.addEventListener('scroll', scheduleLayout, { capture: true, passive: true });
  window.addEventListener('resize', scheduleLayout, { passive: true });

  // --------------------------------------------------------- suggestion card

  let cardState = null;
  let hoverTimer = 0;
  let cardHideTimer = 0;
  let pointer = { x: 0, y: 0 };
  let moveQueued = false;

  document.addEventListener(
    'mousemove',
    (event) => {
      pointer = { x: event.clientX, y: event.clientY };
      if (moveQueued) return;
      moveQueued = true;
      requestAnimationFrame(() => {
        moveQueued = false;
        onPointer();
      });
    },
    { passive: true },
  );

  function onPointer() {
    if (!siteOn) return;
    for (const field of fields) {
      if (!field.findings.length) continue;
      const box = field.el.getBoundingClientRect();
      if (pointer.x < box.left || pointer.x > box.right || pointer.y < box.top || pointer.y > box.bottom) {
        continue;
      }
      if (!field.control) field.measure();
      const hit = field.findingAt(pointer.x, pointer.y);
      if (hit) {
        clearTimeout(cardHideTimer);
        if (cardState?.finding === hit.finding) return;
        clearTimeout(hoverTimer);
        hoverTimer = setTimeout(() => showCard(field, hit.finding, hit.rect), 220);
        return;
      }
    }
    clearTimeout(hoverTimer);
    if (cardState && !card.matches(':hover')) {
      clearTimeout(cardHideTimer);
      cardHideTimer = setTimeout(hideCard, 350);
    }
  }

  card.addEventListener('mouseenter', () => clearTimeout(cardHideTimer));
  card.addEventListener('mouseleave', () => {
    cardHideTimer = setTimeout(hideCard, 350);
  });
  card.addEventListener('mousedown', (e) => e.preventDefault());

  function hideCard() {
    card.hidden = true;
    cardState = null;
    setActiveHighlight(null);
  }

  function fixLabel(fix, flagged) {
    if (fix.op === 'remove') return `Remove “${flagged}”`;
    if (fix.op === 'insert_after') return `Add “${fix.text}”`;
    return fix.text === '' ? 'Remove' : fix.text;
  }

  function showCard(field, finding, rect) {
    cardState = { field, finding };
    const flagged = field.text.slice(finding.start, finding.end);
    const fixes = finding.suggestions.slice(0, 5);
    const spelling = finding.category === 'spelling';
    card.innerHTML = `
      <div class="card-head">
        <span class="dot" style="background:${CATEGORY_COLOR[finding.category]}"></span>
        <span class="card-kind">${esc(CATEGORY_LABEL[finding.category])}</span>
      </div>
      <p class="card-message">${message(finding.message)}</p>
      ${
        fixes.length
          ? `<div class="fixes">${fixes
              .map((fix, i) => `<button class="fix ${i ? 'alt' : ''}" data-fix="${i}">${esc(fixLabel(fix, flagged))}</button>`)
              .join('')}</div>`
          : ''
      }
      <div class="card-actions">
        <button class="link" data-act="ignore">Ignore</button>
        ${spelling ? '<button class="link" data-act="word">Add to dictionary</button>' : ''}
        <button class="link" data-act="rule" title="${esc(finding.rule)}">Turn off rule</button>
      </div>`;
    card.querySelectorAll('[data-fix]').forEach((button) =>
      button.addEventListener('click', () => {
        applyFix(field, finding, fixes[Number(button.dataset.fix)]);
        hideCard();
      }),
    );
    card.querySelectorAll('[data-act]').forEach((button) =>
      button.addEventListener('click', () => {
        const act = button.dataset.act;
        if (act === 'ignore') {
          ignored.add(ignoreKey(finding, field.text));
          field.findings = field.findings.filter((f) => f !== finding);
          field.render();
        } else if (act === 'word') {
          const words = Array.from(new Set([...settings.words, flagged]));
          saveSettings({ words });
        } else if (act === 'rule') {
          saveSettings({ rules: { ...settings.rules, [finding.rule]: false } });
          showToast('Rule turned off. Turn it back on in Narrivy Writing settings.');
        }
        hideCard();
      }),
    );
    if (!field.control) setActiveHighlight(field.rangeFor(finding.start, finding.end));
    place(card, rect);
  }

  async function saveSettings(patch) {
    const stored = await chrome.storage.local.get('settings');
    const next = { ...mergeSettings(stored.settings), ...patch };
    await chrome.storage.local.set({ settings: next });
  }

  // ------------------------------------------------------------------- edits

  // Replace [start, end) of the field's current text through the editor, so
  // it sees an ordinary edit and Ctrl+Z undoes it.
  function replaceText(field, start, end, replacement) {
    const el = field.el;
    if (field.control) {
      el.focus({ preventScroll: true });
      el.setSelectionRange(start, end);
      const done = replacement
        ? document.execCommand('insertText', false, replacement)
        : document.execCommand('delete', false);
      if (!done) {
        el.setRangeText(replacement, start, end, 'end');
        el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertReplacementText', data: replacement }));
      }
      return true;
    }
    field.read();
    const range = field.rangeFor(start, end);
    if (!range) return false;
    el.focus({ preventScroll: true });
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    const done = replacement
      ? document.execCommand('insertText', false, replacement)
      : document.execCommand('delete', false);
    if (!done) {
      const before = new InputEvent('beforeinput', {
        bubbles: true,
        cancelable: true,
        inputType: 'insertReplacementText',
        data: replacement,
      });
      if (el.dispatchEvent(before)) {
        range.deleteContents();
        if (replacement) range.insertNode(document.createTextNode(replacement));
        el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertReplacementText', data: replacement }));
      }
    }
    return true;
  }

  function applyFix(field, finding, fix) {
    if (!fix) return;
    if (fix.op === 'insert_after') replaceText(field, finding.end, finding.end, fix.text);
    else replaceText(field, finding.start, finding.end, fix.op === 'remove' ? '' : fix.text);
    field.check(true);
  }

  function fixAll(field) {
    const chosen = field.findings
      .filter((f) => f.category !== 'style' && f.suggestions.length)
      .sort((a, b) => b.start - a.start);
    let lastStart = Infinity;
    for (const finding of chosen) {
      if (finding.end > lastStart) continue;
      const fix = finding.suggestions[0];
      if (fix.op === 'insert_after') replaceText(field, finding.end, finding.end, fix.text);
      else replaceText(field, finding.start, finding.end, fix.op === 'remove' ? '' : fix.text);
      lastStart = finding.start;
    }
    field.check(true);
  }

  // ------------------------------------------------------------------- panel

  function openPanel(field) {
    hideCard();
    menu.hidden = true;
    panel.dataset.field = field.id();
    const findings = field.findings;
    const error = field.status.startsWith('error') ? field.status.slice(6) : null;
    const fixable = findings.filter((f) => f.category !== 'style' && f.suggestions.length).length;
    panel.innerHTML = `
      <div class="panel-head">
        <span class="brand">Narrivy</span>
        <button class="icon" data-act="close" aria-label="Close">×</button>
      </div>
      ${
        error
          ? `<p class="notice">${esc(errorText(error))}</p>`
          : findings.length
            ? `<div class="list">${findings
                .slice(0, 40)
                .map((f, i) => {
                  const flagged = field.text.slice(f.start, f.end);
                  const fix = f.suggestions[0];
                  return `<div class="row">
                    <span class="dot" style="background:${CATEGORY_COLOR[f.category]}"></span>
                    <div class="row-body">
                      <div class="row-text">${
                        fix
                          ? `<s>${esc(flagged)}</s> → <b>${esc(fixLabel(fix, flagged))}</b>`
                          : `<b>${esc(flagged)}</b>`
                      }</div>
                      <div class="row-message">${message(f.message)}</div>
                    </div>
                    ${fix ? `<button class="small" data-apply="${i}">Apply</button>` : ''}
                  </div>`;
                })
                .join('')}</div>`
            : '<p class="notice good">No issues found.</p>'
      }
      ${fixable > 1 ? `<button class="primary wide" data-act="fixall">Fix ${fixable} spelling and grammar issues</button>` : ''}
      ${
        settings.aiActions
          ? `<div class="section">Rewrite the whole text</div>
             <div class="chips">${ACTIONS.map(([id, label]) => `<button class="chip" data-ai="${id}">${label}</button>`).join('')}</div>`
          : ''
      }
      <div class="panel-foot">
        <button class="link" data-act="site">Turn off on ${esc(location.hostname || 'this page')}</button>
        <button class="link" data-act="options">Settings</button>
      </div>`;
    panel.querySelectorAll('[data-apply]').forEach((button) =>
      button.addEventListener('click', () => {
        const finding = findings[Number(button.dataset.apply)];
        applyFix(field, finding, finding.suggestions[0]);
      }),
    );
    panel.querySelectorAll('[data-ai]').forEach((button) =>
      button.addEventListener('click', () => {
        panel.hidden = true;
        field.read();
        runAction(button.dataset.ai, {
          field,
          start: 0,
          end: field.text.length,
          text: field.text,
          anchor: field.el.getBoundingClientRect(),
        });
      }),
    );
    panel.querySelector('[data-act="close"]').addEventListener('click', closePanel);
    panel.querySelector('[data-act="fixall"]')?.addEventListener('click', () => fixAll(field));
    panel.querySelector('[data-act="site"]').addEventListener('click', async () => {
      const stored = await chrome.storage.local.get('settings');
      const current = mergeSettings(stored.settings);
      await saveSettings({ disabledSites: Array.from(new Set([...current.disabledSites, location.hostname])) });
      closePanel();
      showToast(`Narrivy Writing is off on ${location.hostname}. Turn it on again from the toolbar button.`);
    });
    panel.querySelector('[data-act="options"]').addEventListener('click', () => {
      chrome.runtime.sendMessage({ kind: 'open-options' }).catch(() => {});
      closePanel();
    });
    const box = field.el.getBoundingClientRect();
    place(panel, { left: box.right - 340, right: box.right, top: box.top, bottom: box.bottom });
  }

  function closePanel() {
    panel.hidden = true;
    delete panel.dataset.field;
    for (const field of fields) field.renderBadge();
  }
  panel.addEventListener('mousedown', (e) => {
    if (!(e.target instanceof HTMLInputElement)) e.preventDefault();
  });

  // ------------------------------------------------------ selection actions

  const pill = document.createElement('button');
  pill.className = 'pill';
  pill.type = 'button';
  pill.innerHTML = '<span class="spark">✦</span>';
  pill.title = 'Narrivy writing actions (Alt+Shift+G)';
  pill.hidden = true;
  layer.appendChild(pill);
  pill.addEventListener('mousedown', (e) => e.preventDefault());
  pill.addEventListener('click', () => {
    const target = currentSelection();
    pill.hidden = true;
    if (target) openMenu(target);
  });

  // The selected text in an eligible field: { field, start, end, text, anchor }.
  function currentSelection() {
    const active = document.activeElement;
    const el = eligibleField(active);
    if (el && isControl(el)) {
      const start = el.selectionStart ?? 0;
      const end = el.selectionEnd ?? 0;
      if (end <= start) return null;
      const field = fieldFor(el);
      field.read();
      const rects = controlRects(el, mirror, [[start, end]])[0];
      const anchor = rects.length ? rects[rects.length - 1] : el.getBoundingClientRect();
      return { field, start, end, text: el.value.slice(start, end), anchor };
    }
    const selection = getSelection();
    if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
    const range = selection.getRangeAt(0);
    const node = range.commonAncestorContainer;
    const element = node.nodeType === 1 ? node : node.parentElement;
    const root = eligibleField(element);
    const rects = range.getClientRects();
    const anchor = rects.length ? rects[rects.length - 1] : range.getBoundingClientRect();
    if (root && !isControl(root)) {
      const field = fieldFor(root);
      field.read();
      const start = offsetOf(field, range.startContainer, range.startOffset, false);
      const end = offsetOf(field, range.endContainer, range.endOffset, true);
      if (start === null || end === null || end <= start) return null;
      return { field, start, end, text: field.text.slice(start, end), anchor };
    }
    // Text that is not editable: the result can only be copied.
    const text = selection.toString();
    return text.trim() ? { field: null, text, anchor } : null;
  }

  // The text offset of a DOM boundary. At an element boundary a start
  // snaps forward to the next text and an end back to the previous text, so
  // a selected paragraph does not take the line break before or after it.
  function offsetOf(field, node, offset, isEnd) {
    if (node.nodeType === Node.TEXT_NODE) {
      const part = field.parts.find((p) => p.node === node);
      return part ? part.start + offset : null;
    }
    const boundary = document.createRange();
    boundary.setStart(node, offset);
    boundary.collapse(true);
    if (isEnd) {
      let found = null;
      for (const part of field.parts) {
        if (boundary.comparePoint(part.node, part.node.nodeValue.length) <= 0) found = part;
        else break;
      }
      return found ? found.end : 0;
    }
    for (const part of field.parts) {
      if (boundary.comparePoint(part.node, 0) >= 0) return part.start;
    }
    const last = field.parts[field.parts.length - 1];
    return last ? last.end : 0;
  }

  let selectionTimer = 0;
  document.addEventListener('selectionchange', () => {
    clearTimeout(selectionTimer);
    selectionTimer = setTimeout(updatePill, 250);
  });

  function updatePill() {
    if (!siteOn || !settings.aiActions || !settings.selectionButton || !menu.hidden || !preview.hidden) {
      pill.hidden = true;
      return;
    }
    const target = currentSelection();
    if (!target || !target.field || target.text.trim().length < 3) {
      pill.hidden = true;
      return;
    }
    mount();
    pill.hidden = false;
    pill.style.left = `${Math.min(target.anchor.right + 6, window.innerWidth - 34)}px`;
    pill.style.top = `${Math.max(4, target.anchor.top - 2)}px`;
  }

  function openMenu(target) {
    mount();
    hideCard();
    panel.hidden = true;
    menu.innerHTML = `
      <div class="chips">${ACTIONS.map(([id, label]) => `<button class="chip" data-ai="${id}">${label}</button>`).join('')}</div>
      <form class="ask"><input type="text" placeholder="Ask Narrivy to…" maxlength="500" aria-label="Instruction"/><button class="primary" type="submit">Go</button></form>`;
    menu.querySelectorAll('[data-ai]').forEach((button) =>
      button.addEventListener('click', () => {
        menu.hidden = true;
        runAction(button.dataset.ai, target);
      }),
    );
    const form = menu.querySelector('form');
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      const instruction = form.querySelector('input').value.trim();
      if (!instruction) return;
      menu.hidden = true;
      runAction('custom', target, instruction);
    });
    menu.addEventListener('mousedown', (e) => {
      if (!(e.target instanceof HTMLInputElement)) e.preventDefault();
    });
    place(menu, target.anchor);
  }

  // ------------------------------------------------------------ AI preview

  let writing = null;

  function runAction(action, target, instruction) {
    if (!target?.text?.trim()) {
      showToast('Select some text first.');
      return;
    }
    if (target.text.length > MAX_TEXT) {
      showToast('That is too much text for one action.');
      return;
    }
    mount();
    pill.hidden = true;
    writing?.port?.disconnect();
    const state = {
      action,
      instruction,
      target,
      original: target.text,
      text: '',
      done: false,
      warning: null,
      error: null,
      port: chrome.runtime.connect({ name: 'narrivy-write' }),
    };
    // Remember where the text was, to check nothing moved before replacing.
    if (target.field && !target.field.control) {
      state.range = target.field.rangeFor(target.start, target.end);
    }
    writing = state;
    preview.dataset.busy = '1';
    state.port.onMessage.addListener((message) => {
      if (writing !== state) return;
      if (message.kind === 'delta') {
        state.text += message.text;
      } else if (message.kind === 'done') {
        state.done = true;
        delete preview.dataset.busy;
        if (message.type === 'error') state.error = message.code;
        else if (message.outcome?.startsWith('failed:')) state.error = message.outcome.slice(7);
        else if (message.outcome === 'cancelled') state.error = 'cancelled';
        else {
          state.text = message.text || state.text;
          state.warning = message.warning ?? null;
        }
        state.port.disconnect();
      }
      renderPreview();
    });
    state.port.onDisconnect.addListener(() => {
      if (writing === state && !state.done) {
        state.done = true;
        state.error = 'host_stopped';
        delete preview.dataset.busy;
        renderPreview();
      }
    });
    state.port.postMessage({ kind: 'start', action, text: target.text, instruction });
    renderPreview(true);
  }

  function renderPreview(first = false) {
    const state = writing;
    if (!state) return;
    const title = state.action === 'custom' ? state.instruction : ACTION_TITLE[state.action];
    const editable = Boolean(state.target.field);
    const status = state.error
      ? `<p class="notice">${esc(errorText(state.error))}</p>`
      : state.done
        ? state.warning
          ? `<p class="notice warn">${esc(warningText(state.warning))}</p>`
          : ''
        : `<div class="status"><span class="spinner"></span>${state.text ? 'Writing…' : 'Starting the model…'}</div>`;
    preview.innerHTML = `
      <div class="panel-head">
        <span class="brand">Narrivy · ${esc(title)}</span>
        <button class="icon" data-act="close" aria-label="Close">×</button>
      </div>
      ${state.text ? `<div class="result" tabindex="0">${esc(state.text)}</div>` : ''}
      ${status}
      <div class="preview-actions">
        ${
          state.done && !state.error
            ? `${editable ? '<button class="primary" data-act="replace">Replace</button><button class="secondary" data-act="below">Insert below</button>' : ''}
               <button class="secondary" data-act="copy">Copy</button>
               <button class="secondary" data-act="again">Try again</button>`
            : state.done
              ? '<button class="secondary" data-act="again">Try again</button>'
              : '<button class="secondary" data-act="stop">Stop</button>'
        }
      </div>`;
    const on = (act, handler) => preview.querySelector(`[data-act="${act}"]`)?.addEventListener('click', handler);
    on('close', closePreview);
    on('stop', () => state.port.postMessage({ kind: 'cancel' }));
    on('again', () => runAction(state.action, state.target, state.instruction));
    on('copy', async () => {
      try {
        await navigator.clipboard.writeText(state.text);
        showToast('Copied.');
      } catch {
        showToast('Could not copy. Select the text and copy it instead.');
      }
    });
    on('replace', () => commit(state, false));
    on('below', () => commit(state, true));
    if (first || preview.hidden) {
      const anchor = state.target.anchor;
      place(preview, anchor);
    } else {
      // Keep it on screen as it grows.
      const rect = preview.getBoundingClientRect();
      if (rect.bottom > window.innerHeight - 8) {
        preview.style.top = `${Math.max(8, window.innerHeight - rect.height - 8)}px`;
      }
    }
  }

  function closePreview() {
    if (writing && !writing.done) writing.port.postMessage({ kind: 'cancel' });
    writing = null;
    delete preview.dataset.busy;
    preview.hidden = true;
  }
  preview.addEventListener('mousedown', (e) => {
    if (!(e.target instanceof HTMLElement && e.target.classList.contains('result'))) e.preventDefault();
  });

  function commit(state, below) {
    const { field } = state.target;
    if (!field) return;
    field.read();
    let { start, end } = state.target;
    // The text may have moved since the action started.
    if (field.text.slice(start, end) !== state.original) {
      const found = field.text.indexOf(state.original);
      if (found < 0) {
        showToast('The text changed while Narrivy was writing. Use Copy instead.');
        return;
      }
      start = found;
      end = found + state.original.length;
    }
    if (below) replaceText(field, end, end, `\n\n${state.text}`);
    else replaceText(field, start, end, state.text);
    closePreview();
    field.check(true);
    showToast(below ? 'Inserted.' : 'Replaced.', {
      label: 'Undo',
      run: () => {
        field.el.focus({ preventScroll: true });
        document.execCommand('undo');
      },
    });
  }

  // ------------------------------------------------------------------- toast

  let toastTimer = 0;
  function showToast(message, action) {
    mount();
    toast.innerHTML = `<span>${esc(message)}</span>${action ? `<button class="link" data-act="toast">${esc(action.label)}</button>` : ''}`;
    toast.querySelector('[data-act="toast"]')?.addEventListener('click', () => {
      action.run();
      toast.hidden = true;
    });
    toast.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toast.hidden = true;
    }, action ? 6000 : 3500);
  }
  toast.addEventListener('mousedown', (e) => e.preventDefault());

  // ---------------------------------------------------------------- messages

  function errorText(code) {
    switch (code) {
      case 'host_missing':
        return "Narrivy's browser connector is not installed. Install it from Narrivy (scripts/install-browser-extension.sh in development).";
      case 'host_stopped':
      case 'daemon_unavailable':
      case 'daemon_restarted':
        return 'Narrivy is not running. Start Narrivy and try again.';
      case 'cancelled':
        return 'Stopped.';
      case 'assistant_not_configured':
      case 'assistant_model_missing':
        return 'Choose an assistant model in Narrivy → Models → Language models.';
      case 'assistant_timeout':
        return 'The model took too long. Try again or pick a smaller assistant model.';
      case 'assistant_unavailable':
      case 'assistant_worker_lost':
      case 'assistant_failed':
        return "Narrivy's assistant model could not run. Check Narrivy → Models → Language models.";
      case 'writing_text_too_long':
        return 'That is too much text for one action.';
      case 'extension_reloaded':
        return 'Narrivy Writing was updated. Reload this page.';
      default:
        return `Narrivy could not do that (${code}).`;
    }
  }

  function warningText(code) {
    if (code === 'not_faithful') return 'This changed more than a fix should. Check it before replacing.';
    if (code === 'lost_details') return 'Some numbers, links or addresses from your text are missing. Check before replacing.';
    return 'Check this before replacing.';
  }

  chrome.runtime.onMessage.addListener((message, _sender, reply) => {
    if (message.kind !== 'run-action' && message.kind !== 'open-actions') return false;
    // Settings load once at start; a script added just now waits for them.
    ready.then(() => {
      handleCommand(message);
      reply({ ok: true });
    });
    return true;
  });

  function handleCommand(message) {
    if (!siteOn) {
      showToast('Narrivy Writing is off on this site. Turn it on from the toolbar button.');
      return;
    }
    if (message.kind === 'run-action') {
      let target = currentSelection();
      // The page may have dropped its selection when the menu opened.
      if (!target && message.selectionText?.trim()) {
        const box = document.activeElement?.getBoundingClientRect?.();
        target = {
          field: null,
          text: message.selectionText,
          anchor: box ?? { left: 16, right: 16, top: 16, bottom: 16 },
        };
      }
      if (!target) {
        showToast('Select some text first.');
        return;
      }
      if (message.action === 'custom') openMenu(target);
      else runAction(message.action, target);
    } else if (message.kind === 'open-actions') {
      if (!document.hasFocus()) return;
      const target = currentSelection();
      if (target) openMenu(target);
      else if (activeField) openPanel(activeField);
    }
  }

  document.addEventListener(
    'keydown',
    (event) => {
      if (event.key !== 'Escape') return;
      if (!card.hidden || !panel.hidden || !menu.hidden || !preview.hidden) {
        hideCard();
        closePanel();
        menu.hidden = true;
        if (!preview.hidden) closePreview();
      }
    },
    true,
  );

  document.addEventListener(
    'mousedown',
    (event) => {
      if (event.composedPath().includes(host)) return;
      menu.hidden = true;
      if (!panel.hidden) closePanel();
    },
    true,
  );

  // -------------------------------------------------------------------- CSS

  function uiCss() {
    const wave = (color) =>
      `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5Q1.5 0 3 2.5T6 2.5' fill='none' stroke='${encodeURIComponent(color)}' stroke-width='1.2'/%3E%3C/svg%3E")`;
    return `
      :host { all: initial; }
      * { box-sizing: border-box; }
      [hidden] { display: none !important; }
      .mirror { position: fixed; visibility: hidden; pointer-events: none; overflow: hidden; }
      .layer { position: fixed; inset: 0; pointer-events: none; }
      .line { position: absolute; height: 3px; pointer-events: none; }
      .line.spelling { background: ${wave('#e5484d')} repeat-x left center; }
      .line.grammar { background: ${wave('#f08c00')} repeat-x left center; }
      .line.punctuation { border-bottom: 2px solid #f08c00; height: 2px; }
      .line.style { border-bottom: 2px dotted #3e7bfa; height: 2px; }
      :host, .card, .panel, .menu, .preview, .toast, .badge, .pill {
        --bg: #ffffff; --text: #1c1c1c; --muted: #6b6b6b; --border: rgba(10,10,10,.1);
        --hover: #f0efeb; --accent: #111111; --accent-text: #ffffff; --shadow: 0 10px 30px rgba(0,0,0,.14), 0 2px 6px rgba(0,0,0,.08);
        font: 400 13px/1.45 "Inter Variable", Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
        color: var(--text);
      }
      @media (prefers-color-scheme: dark) {
        :host, .card, .panel, .menu, .preview, .toast, .badge, .pill {
          --bg: #1a1a1a; --text: #ededed; --muted: #9a9a9a; --border: rgba(255,255,255,.1);
          --hover: #242424; --accent: #f2f2f2; --accent-text: #0a0a0a; --shadow: 0 10px 30px rgba(0,0,0,.5);
        }
      }
      .card, .panel, .menu, .preview, .toast {
        position: fixed; pointer-events: auto; background: var(--bg); border: 1px solid var(--border);
        border-radius: 14px; box-shadow: var(--shadow);
      }
      .card { width: 300px; padding: 12px 14px; }
      .card-head { display: flex; align-items: center; gap: 8px; font-weight: 600; font-size: 12px; color: var(--muted); }
      .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
      .card-message { margin: 6px 0 10px; }
      .fixes { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
      button { font: inherit; color: inherit; cursor: pointer; }
      .fix { background: var(--accent); color: var(--accent-text); border: 0; border-radius: 8px; padding: 5px 10px; font-weight: 600; }
      .fix.alt { background: none; color: var(--text); border: 1px solid var(--border); font-weight: 500; }
      .fix:hover, .primary:hover { opacity: .88; }
      .fix.alt:hover { background: var(--hover); opacity: 1; }
      .card-message b, .row-message b { font-weight: 600; }
      .card-actions, .panel-foot { display: flex; gap: 14px; flex-wrap: wrap; }
      .link { background: none; border: 0; padding: 0; color: var(--muted); text-decoration: underline; text-underline-offset: 2px; }
      .link:hover { color: var(--text); }
      .badge {
        position: fixed; pointer-events: auto; border-radius: 50%; border: 0; display: grid; place-items: center;
        font-size: 11px; font-weight: 700; color: #fff; background: #3a3a3a; box-shadow: 0 1px 4px rgba(0,0,0,.25); padding: 0;
      }
      .badge.clean { background: #2f9e44; }
      .badge.clean::after { content: ""; width: 6px; height: 3px; border: solid #fff; border-width: 0 0 2px 2px; transform: rotate(-45deg) translate(1px,-1px); }
      .badge.issues { background: #e5484d; }
      .badge.style { background: #3e7bfa; }
      .badge.error { background: #8a8a8a; }
      .badge.busy { background: #3a3a3a; }
      .badge.busy::before { content: ""; position: absolute; inset: 2px; border-radius: 50%; border: 2px solid rgba(255,255,255,.35); border-top-color: #fff; animation: spin .8s linear infinite; }
      @keyframes spin { to { transform: rotate(360deg); } }
      .panel, .preview { width: 340px; max-height: min(480px, calc(100vh - 16px)); overflow: auto; padding: 12px 14px; scrollbar-width: thin; scrollbar-color: var(--border) transparent; }
      .preview { width: 380px; }
      .panel-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
      .brand { font-weight: 700; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .icon { background: none; border: 0; font-size: 18px; line-height: 1; color: var(--muted); padding: 2px 6px; border-radius: 6px; }
      .icon:hover { background: var(--hover); color: var(--text); }
      .list { display: flex; flex-direction: column; gap: 2px; margin-bottom: 10px; }
      .row { display: flex; gap: 8px; align-items: flex-start; padding: 7px 6px; border-radius: 8px; }
      .row:hover { background: var(--hover); }
      .row .dot { margin-top: 6px; }
      .row-body { flex: 1; min-width: 0; }
      .row-text { overflow-wrap: anywhere; }
      .row-text s { color: var(--muted); }
      .row-message { color: var(--muted); font-size: 12px; }
      .small { background: none; border: 1px solid var(--border); border-radius: 7px; padding: 2px 8px; font-size: 12px; }
      .small:hover, .secondary:hover, .chip:hover { background: var(--hover); }
      .primary { background: var(--accent); color: var(--accent-text); border: 0; border-radius: 9px; padding: 7px 12px; font-weight: 600; }
      .secondary { background: none; border: 1px solid var(--border); border-radius: 9px; padding: 6px 11px; }
      .wide { width: 100%; margin-bottom: 10px; }
      .section { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .04em; color: var(--muted); margin: 4px 0 6px; }
      .chips { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 10px; }
      .chip { background: none; border: 1px solid var(--border); border-radius: 999px; padding: 4px 10px; font-size: 12.5px; }
      .notice { margin: 4px 0 10px; color: var(--muted); }
      .notice.good { color: #2f9e44; }
      .notice.warn { color: #b26a00; }
      .menu { width: 320px; padding: 10px; }
      .menu .chips { margin-bottom: 8px; }
      .ask { display: flex; gap: 6px; }
      .ask input { flex: 1; min-width: 0; font: inherit; color: var(--text); background: transparent; border: 1px solid var(--border); border-radius: 9px; padding: 6px 9px; outline: none; }
      .ask input:focus { border-color: var(--muted); }
      .result { white-space: pre-wrap; overflow-wrap: anywhere; background: var(--hover); border-radius: 10px; padding: 10px 12px; margin-bottom: 10px; max-height: 300px; overflow: auto; user-select: text; }
      .status { display: flex; align-items: center; gap: 8px; color: var(--muted); margin-bottom: 10px; }
      .spinner { width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--border); border-top-color: var(--text); animation: spin .8s linear infinite; }
      .preview-actions { display: flex; gap: 8px; flex-wrap: wrap; }
      .toast { left: 50%; bottom: 20px; transform: translateX(-50%); padding: 9px 14px; display: flex; gap: 12px; align-items: center; }
      .pill {
        position: fixed; pointer-events: auto; width: 26px; height: 26px; border-radius: 50%; border: 1px solid var(--border);
        background: var(--bg); box-shadow: var(--shadow); display: grid; place-items: center; padding: 0;
      }
      .pill .spark { font-size: 13px; color: var(--text); }
      .pill:hover { background: var(--hover); }
      button:focus-visible, input:focus-visible { outline: 2px solid #3e7bfa; outline-offset: 2px; }
    `;
  }

  const ready = refreshSettings();
})();
