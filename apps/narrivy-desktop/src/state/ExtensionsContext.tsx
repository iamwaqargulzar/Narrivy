// Which extensions are switched on (D48): the AI assistant and meeting
// transcription, each with its own sidebar screen, switched in Settings →
// Extensions. Both are on by default. The assistant's switch is kept by the
// service too (`enabled` in its model settings), so its model is not loaded
// at service start while it is off; the meetings switch only hides the
// screen. The writing assistant's switch is the browser connector itself
// (`bridge.browserConnector`), not kept here.
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { bridge } from '../lib/bridge';
import { useNav, type ScreenId } from './NavContext';

export const EXTENSIONS_STORAGE_KEY = 'narrivy.extensions';

export type ExtensionId = 'assistant' | 'meetings';

interface ExtensionsValue {
  assistant: boolean;
  meetings: boolean;
  setEnabled: (id: ExtensionId, on: boolean) => Promise<void>;
  /** Whether a screen is shown: an extension's screen only while it is on. */
  shows: (screen: ScreenId) => boolean;
}

const ExtensionsContext = createContext<ExtensionsValue | null>(null);

function readStored(): { assistant: boolean; meetings: boolean } {
  try {
    const raw = window.localStorage.getItem(EXTENSIONS_STORAGE_KEY);
    const value = raw ? (JSON.parse(raw) as Partial<Record<ExtensionId, boolean>>) : {};
    return { assistant: value.assistant !== false, meetings: value.meetings !== false };
  } catch {
    return { assistant: true, meetings: true };
  }
}

export function ExtensionsProvider({ children }: { children: ReactNode }) {
  const { screen, navigate } = useNav();
  const [state, setState] = useState(readStored);

  const store = useCallback((next: { assistant: boolean; meetings: boolean }) => {
    setState(next);
    try {
      window.localStorage.setItem(EXTENSIONS_STORAGE_KEY, JSON.stringify(next));
    } catch {
      // Not kept across restarts; the switch still applies now.
    }
  }, []);

  // The service is the record for the assistant: follow it when it answers.
  useEffect(() => {
    bridge
      .assistantLibrary('get', 'model')
      .then((json) => {
        const enabled = (JSON.parse(json) as { enabled?: boolean }).enabled;
        if (typeof enabled === 'boolean') {
          setState((old) => (old.assistant === enabled ? old : { ...old, assistant: enabled }));
        }
      })
      .catch(() => {});
  }, []);

  const setEnabled = useCallback(
    async (id: ExtensionId, on: boolean) => {
      if (id === 'assistant') {
        await bridge.assistantLibrary('put', 'model', null, JSON.stringify({ enabled: on }));
      }
      store({ ...state, [id]: on });
    },
    [state, store],
  );

  const shows = useCallback(
    (target: ScreenId) => (target === 'assistant' ? state.assistant : target === 'meetings' ? state.meetings : true),
    [state],
  );

  // A screen switched off while it is open goes back to Home.
  useEffect(() => {
    if (!shows(screen)) navigate('home');
  }, [screen, shows, navigate]);

  const value = useMemo<ExtensionsValue>(
    () => ({ assistant: state.assistant, meetings: state.meetings, setEnabled, shows }),
    [state, setEnabled, shows],
  );
  return <ExtensionsContext.Provider value={value}>{children}</ExtensionsContext.Provider>;
}

const ALL_ON: ExtensionsValue = {
  assistant: true,
  meetings: true,
  setEnabled: () => Promise.resolve(),
  shows: () => true,
};

export function useExtensions(): ExtensionsValue {
  return useContext(ExtensionsContext) ?? ALL_ON;
}
