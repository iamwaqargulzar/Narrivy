// System/Light/Dark theme choice, persisted at localStorage['narrivy.theme'].
// The resolved theme is applied as a class ('light' or 'dark') on <html>.
// The accent (mono, blue or gold) is persisted at localStorage['narrivy.accent']
// and applied as <html data-accent>; tokens.css defines each accent per theme.
// The resolved theme and accent are also reported to the host, which draws the
// dictation overlay in the same colours. Where that overlay sits is persisted at
// localStorage['narrivy.overlay.position'] (bottom centre by default).
// Whether Home shows the greeting is persisted at localStorage['narrivy.greeting']
// ('on' or 'off'; shown by default).
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { t } from '../i18n';
import { bridge } from '../lib/bridge';

export type ThemeChoice = 'system' | 'light' | 'dark';

export const THEME_STORAGE_KEY = 'narrivy.theme';
export const ACCENT_STORAGE_KEY = 'narrivy.accent';
export const GREETING_STORAGE_KEY = 'narrivy.greeting';

export const OVERLAY_POSITION_STORAGE_KEY = 'narrivy.overlay.position';

export const ACCENTS = ['mono', 'blue', 'gold'] as const;
export type AccentChoice = (typeof ACCENTS)[number];

/** The six places the dictation overlay can sit, in reading order. */
export const OVERLAY_POSITIONS = [
  'top-left',
  'top-center',
  'top-right',
  'bottom-left',
  'bottom-center',
  'bottom-right',
] as const;
export type OverlayPosition = (typeof OVERLAY_POSITIONS)[number];
export const DEFAULT_OVERLAY_POSITION: OverlayPosition = 'bottom-center';

interface ThemeContextValue {
  theme: ThemeChoice;
  resolvedTheme: 'light' | 'dark';
  setTheme: (choice: ThemeChoice) => void;
  cycleTheme: () => void;
  accent: AccentChoice;
  setAccent: (choice: AccentChoice) => void;
  showGreeting: boolean;
  setShowGreeting: (show: boolean) => void;
  overlayPosition: OverlayPosition;
  setOverlayPosition: (position: OverlayPosition) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readStoredChoice(): ThemeChoice {
  try {
    const raw = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === 'light' || raw === 'dark' || raw === 'system') return raw;
  } catch {
    // storage unavailable
  }
  return 'dark';
}

function readStoredAccent(): AccentChoice {
  try {
    const raw = window.localStorage.getItem(ACCENT_STORAGE_KEY);
    if ((ACCENTS as readonly string[]).includes(raw ?? '')) return raw as AccentChoice;
  } catch {
    // storage unavailable
  }
  return 'mono';
}

function readStoredOverlayPosition(): OverlayPosition {
  try {
    const raw = window.localStorage.getItem(OVERLAY_POSITION_STORAGE_KEY);
    if ((OVERLAY_POSITIONS as readonly string[]).includes(raw ?? '')) return raw as OverlayPosition;
  } catch {
    // storage unavailable
  }
  return DEFAULT_OVERLAY_POSITION;
}

function readStoredGreeting(): boolean {
  try {
    return window.localStorage.getItem(GREETING_STORAGE_KEY) !== 'off';
  } catch {
    return true;
  }
}

function systemPrefersDark(): boolean {
  try {
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
  } catch {
    return false;
  }
}

function applyResolvedClass(resolved: 'light' | 'dark'): void {
  const el = document.documentElement;
  el.classList.remove('light', 'dark');
  el.classList.add(resolved);
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeChoice>(readStoredChoice);
  const [systemDark, setSystemDark] = useState(systemPrefersDark);
  const [accent, setAccentState] = useState<AccentChoice>(readStoredAccent);
  const [showGreeting, setShowGreetingState] = useState<boolean>(readStoredGreeting);
  const [overlayPosition, setOverlayPositionState] = useState<OverlayPosition>(readStoredOverlayPosition);

  const setOverlayPosition = useCallback((position: OverlayPosition) => {
    setOverlayPositionState(position);
    try {
      window.localStorage.setItem(OVERLAY_POSITION_STORAGE_KEY, position);
    } catch {
      // storage unavailable; keep in-memory choice
    }
  }, []);

  const setShowGreeting = useCallback((show: boolean) => {
    setShowGreetingState(show);
    try {
      window.localStorage.setItem(GREETING_STORAGE_KEY, show ? 'on' : 'off');
    } catch {
      // storage unavailable; keep in-memory choice
    }
  }, []);

  useEffect(() => {
    document.documentElement.dataset.accent = accent;
  }, [accent]);

  const setAccent = useCallback((choice: AccentChoice) => {
    setAccentState(choice);
    try {
      window.localStorage.setItem(ACCENT_STORAGE_KEY, choice);
    } catch {
      // storage unavailable; keep in-memory choice
    }
  }, []);

  useEffect(() => {
    let dispose = () => {};
    try {
      const mq = window.matchMedia('(prefers-color-scheme: dark)');
      const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
      mq.addEventListener('change', onChange);
      dispose = () => mq.removeEventListener('change', onChange);
    } catch {
      // matchMedia unavailable
    }
    return dispose;
  }, []);

  const resolvedTheme: 'light' | 'dark' = theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;

  useEffect(() => {
    applyResolvedClass(resolvedTheme);
  }, [resolvedTheme]);

  // The dictation overlay is drawn by the host, outside this page: tell it
  // which theme and accent to match and where to sit. A host without an
  // overlay ignores this.
  useEffect(() => {
    try {
      void bridge.setOverlayAppearance?.(resolvedTheme, accent, overlayPosition)?.catch(() => {});
    } catch {
      // no host (tests, browser preview)
    }
  }, [resolvedTheme, accent, overlayPosition]);

  const setTheme = useCallback((choice: ThemeChoice) => {
    setThemeState(choice);
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, choice);
    } catch {
      // storage unavailable; keep in-memory choice
    }
  }, []);

  // Cycle order: System → Dark → Light → System.
  const cycleTheme = useCallback(() => {
    setTheme(theme === 'system' ? 'dark' : theme === 'dark' ? 'light' : 'system');
  }, [theme, setTheme]);

  const value = useMemo<ThemeContextValue>(
    () => ({
      theme,
      resolvedTheme,
      setTheme,
      cycleTheme,
      accent,
      setAccent,
      showGreeting,
      setShowGreeting,
      overlayPosition,
      setOverlayPosition,
    }),
    [theme, resolvedTheme, setTheme, cycleTheme, accent, setAccent, showGreeting, setShowGreeting, overlayPosition, setOverlayPosition],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider');
  return ctx;
}

export function ThemeToggleLabel(): string {
  return t('theme.cycleAria');
}
