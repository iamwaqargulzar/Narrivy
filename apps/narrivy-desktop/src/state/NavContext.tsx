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
  'applications',
  'history',
  'settings',
  'diagnostics',
  'about',
] as const;
export const PRIMARY_SCREENS = [
  'home',
  'history',
  'pipelines',
  'models',
  'store',
  'assistant',
  'meetings',
  'applications',
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
  applications: 'nav.applications',
  history: 'nav.history',
  settings: 'nav.settings',
  diagnostics: 'nav.diagnostics',
  about: 'nav.about',
};

export const SCREEN_TITLE_KEYS: Record<ScreenId, string> = NAV_LABEL_KEYS;

interface NavContextValue {
  screen: ScreenId;
  navigate: (screen: ScreenId) => void;
}

const NavContext = createContext<NavContextValue | null>(null);

export function NavProvider({ children }: { children: ReactNode }) {
  const [screen, setScreen] = useState<ScreenId>('home');
  const value = useMemo<NavContextValue>(
    () => ({ screen, navigate: setScreen }),
    [screen],
  );
  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useNav(): NavContextValue {
  const ctx = useContext(NavContext);
  if (!ctx) throw new Error('useNav must be used within NavProvider');
  return ctx;
}
