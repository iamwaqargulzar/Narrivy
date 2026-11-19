// Turning a key press into the combination the service understands, and a
// combination into words. The service decides which combinations are
// allowed; this module only translates.
import type { ShortcutBinding } from './bridge';

export const MODIFIER_ORDER = ['Super', 'Ctrl', 'Alt', 'Shift'] as const;

const NAMED_CODES: Record<string, string> = {
  Space: 'space',
  Insert: 'Insert',
  Home: 'Home',
  End: 'End',
  PageUp: 'Prior',
  PageDown: 'Next',
  Pause: 'Pause',
  ScrollLock: 'Scroll_Lock',
  PrintScreen: 'Print',
  ContextMenu: 'Menu',
  Comma: 'comma',
  Period: 'period',
  Slash: 'slash',
  Semicolon: 'semicolon',
  Quote: 'apostrophe',
  Backquote: 'grave',
  Minus: 'minus',
  Equal: 'equal',
  BracketLeft: 'bracketleft',
  BracketRight: 'bracketright',
  Backslash: 'backslash',
};

const KEY_LABELS: Record<string, string> = {
  space: 'Space',
  Prior: 'Page Up',
  Scroll_Lock: 'Scroll Lock',
  Print: 'Print Screen',
  Next: 'Page Down',
  comma: ',',
  period: '.',
  slash: '/',
  semicolon: ';',
  apostrophe: "'",
  grave: '`',
  minus: '-',
  equal: '=',
  bracketleft: '[',
  bracketright: ']',
  backslash: '\\',
};

export interface PressedKeys {
  modifiers: string[];
  /** The non-modifier key, or null while only modifiers are held. A saved
   * combination uses an empty key for modifiers alone. */
  key: string | null;
}

interface KeyLike {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

/** The physical key as a key name, or null for a key that cannot be bound. */
export function keyNameFromCode(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1].toLowerCase();
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit) return digit[1];
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return NAMED_CODES[code] ?? null;
}

export function pressedKeys(event: KeyLike): PressedKeys {
  const held: Record<string, boolean> = {
    Super: event.metaKey,
    Ctrl: event.ctrlKey,
    Alt: event.altKey,
    Shift: event.shiftKey,
  };
  return {
    modifiers: MODIFIER_ORDER.filter((name) => held[name]),
    key: keyNameFromCode(event.code),
  };
}

export function keyLabel(key: string): string {
  return KEY_LABELS[key] ?? (key.length === 1 ? key.toUpperCase() : key);
}

/** The keys of a combination as separate words, for key chips. */
export function comboParts(combo: Pick<ShortcutBinding, 'modifiers' | 'key'>): string[] {
  const modifiers = MODIFIER_ORDER.filter((name) => combo.modifiers.includes(name));
  return combo.key ? [...modifiers, keyLabel(combo.key)] : [...modifiers];
}

export function comboLabel(combo: Pick<ShortcutBinding, 'modifiers' | 'key'>): string {
  return comboParts(combo).join(' + ');
}
