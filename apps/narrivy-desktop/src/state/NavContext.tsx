import { createContext, useContext, useMemo, useState, type ReactNode } from 'react';

// Navigation follows the approved design (spec/ui/reference-images):
// primary destinations in the sidebar, About and the version in its footer.
export const SCREENS = [
  'home',
  'pipelines',
  'models',
  'store',
  'assistant',
  'meetings',
  'history',
  'settings',
  'diagnostics',
  'about',
] as const;
export const PRIMARY_SCREENS = [
  'home',
  'history',
  'pipelines',
  'store',
  'assistant',
  'meetings',
  'settings',
  'diagnostics',
] as const;
export const SECONDARY_SCREENS = ['about'] as const;

export type ScreenId = (typeof SCREENS)[number];

export const NAV_LABEL_KEYS: Record<ScreenId, string> = {
  home: 'nav.home',
  pipelines: 'nav.pipelines',
  models: 'nav.models',
  store: 'nav.store',
  assistant: 'nav.assistant',
  meetings: 'nav.meetings',
  history: 'nav.history',
  settings: 'nav.settings',
  diagnostics: 'nav.diagnostics',
  about: 'nav.about',
};

export const SCREEN_TITLE_KEYS: Record<ScreenId, string> = NAV_LABEL_KEYS;

interface NavContextValue {
  screen: ScreenId;
  /** The part of the screen to open first (a Store section, for example). */
  section: string | null;
  navigate: (screen: ScreenId, section?: string) => void;
}

const NavContext = createContext<NavContextValue | null>(null);

export function NavProvider({ children }: { children: ReactNode }) {
  const [place, setPlace] = useState<{ screen: ScreenId; section: string | null }>({
    screen: 'home',
    section: null,
  });
  const value = useMemo<NavContextValue>(
    () => ({
      screen: place.screen,
      section: place.section,
      navigate: (screen, section) => setPlace({ screen, section: section ?? null }),
    }),
    [place],
  );
  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useNav(): NavContextValue {
  const ctx = useContext(NavContext);
  if (!ctx) throw new Error('useNav must be used within NavProvider');
  return ctx;
}
