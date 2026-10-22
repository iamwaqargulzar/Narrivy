// Top-level chrome, following the approved design: a full-width top bar
// (brand, local/private note, live status, search, theme switch), the
// navigation sidebar, the main landmark hosting the active screen, a live
// status strip along the bottom, and the command palette.
import { useEffect, useState } from 'react';
import { CommandPalette } from './CommandPalette';
import { HomeScreen } from './screens/Home';
import { DictationScreen } from './screens/Dictation';
import { PipelinesScreen } from './screens/Pipelines';
import { ModelsScreen } from './screens/Models';
import { RefineScreen } from './screens/Refine';
import { AssistantScreen } from './screens/Assistant';
import { MeetingsScreen } from './screens/Meetings';
import { StoreScreen } from './screens/Store';
import { ApplicationsScreen } from './screens/Applications';
import { HistoryScreen } from './screens/History';
import { SettingsScreen } from './screens/Settings';
import { DiagnosticsScreen } from './screens/Diagnostics';
import { AboutScreen } from './screens/About';
import {
  NAV_LABEL_KEYS,
  PRIMARY_SCREENS,
  SCREEN_TITLE_KEYS,
  SECONDARY_SCREENS,
  useNav,
  type ScreenId,
} from '../state/NavContext';
import { useTheme } from '../theme/ThemeProvider';
import { BrandMark, Icon, type IconName } from '../ui/icons';
import { StatusStrip } from './StatusStrip';
import { TipsPopover } from './TipsPopover';
import { useSession } from '../state/SessionContext';
import { useDaemonStatus } from '../state/StatusContext';
import { usePipelines } from '../state/PipelinesContext';
import { t } from '../i18n';

const NAV_ICONS: Record<ScreenId, IconName> = {
  home: 'home',
  dictation: 'mic',
  pipelines: 'pipeline',
  models: 'models',
  store: 'store',
  refine: 'sliders',
  assistant: 'spark',
  meetings: 'people',
  applications: 'apps',
  history: 'history',
  settings: 'gear',
  diagnostics: 'pulse',
  about: 'about',
};

const LIVE = new Set(['preparing', 'recording', 'listening']);
const BUSY = new Set([
  'finalizing_audio',
  'transcribing',
  'transforming',
  'delivering',
  'processing',
]);

function ActiveScreen() {
  const { screen } = useNav();
  switch (screen) {
    case 'home':
      return <HomeScreen />;
    case 'dictation':
      return <DictationScreen />;
    case 'pipelines':
      return <PipelinesScreen />;
    case 'models':
      return <ModelsScreen />;
    case 'store':
      return <StoreScreen />;
    case 'refine':
      return <RefineScreen />;
    case 'assistant':
      return <AssistantScreen />;
    case 'meetings':
      return <MeetingsScreen />;
    case 'applications':
      return <ApplicationsScreen />;
    case 'history':
      return <HistoryScreen />;
    case 'settings':
      return <SettingsScreen />;
    case 'diagnostics':
      return <DiagnosticsScreen />;
    case 'about':
      return <AboutScreen />;
  }
}

function NavButton({ id, live }: { id: ScreenId; live?: boolean }) {
  const { screen, navigate } = useNav();
  const label = t(NAV_LABEL_KEYS[id]);
  return (
    <button
      type="button"
      className="nav-item"
      aria-current={screen === id ? 'true' : 'false'}
      aria-label={label}
      title={label}
      onClick={() => navigate(id)}
    >
      <Icon name={NAV_ICONS[id]} className="nav-icon" />
      <span>{label}</span>
      {live ? <span className="nav-live-dot" aria-hidden="true" /> : null}
    </button>
  );
}

/** The installed version with its update state; opens About. The state is
 * only ever what was actually checked: until the opt-in update check exists
 * it says "Not checked", never "Up to date". */
function VersionButton() {
  const { navigate } = useNav();
  const state = t('about.updatesNotChecked');
  return (
    <button
      type="button"
      className="version-button"
      aria-label={t('version.button', { version: __APP_VERSION__, state })}
      title={t('version.button', { version: __APP_VERSION__, state })}
      onClick={() => navigate('about')}
    >
      <span className="status-dot" data-state="unchecked" aria-hidden="true" />
      <span className="version-button-name">v{__APP_VERSION__}</span>
    </button>
  );
}

/** One live status word for the top bar, from session events and readiness. */
function useShellStatus(): { label: string; tone: 'ok' | 'live' | 'busy' | 'warn' | 'idle' } {
  const session = useSession();
  const { status, error } = useDaemonStatus();
  const { catalog } = usePipelines();
  if (LIVE.has(session.phase)) return { label: t('shell.status.listening'), tone: 'live' };
  if (BUSY.has(session.phase)) return { label: t('shell.status.processing'), tone: 'busy' };
  if (!status && error) return { label: t('shell.offline'), tone: 'warn' };
  if (!status) return { label: t('shell.status.connecting'), tone: 'idle' };
  if (catalog.availability === 'ready') return { label: t('shell.status.ready'), tone: 'ok' };
  if (catalog.availability === 'loading') return { label: t('shell.status.connecting'), tone: 'idle' };
  return { label: t('shell.status.attention'), tone: 'warn' };
}

export function AppShell() {
  const { screen } = useNav();
  const { resolvedTheme, setTheme } = useTheme();
  const session = useSession();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const shellStatus = useShellStatus();
  const daemonStatus = useDaemonStatus().status;
  const statusReady = Boolean(daemonStatus);
  const live = LIVE.has(session.phase);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (e.key === 'Escape') {
        setPaletteOpen(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        {t('a11y.skip')}
      </a>
      <header className="topbar">
        <div className="brand">
          <BrandMark />
          <span className="brand-name">{t('app.name')}</span>
        </div>
        <div className="topbar-main">
          <span className="topbar-item" aria-label="Processing mode: Local, private">
            <Icon name="lock" />
            {t('shell.localPrivate')}
          </span>
          <span className="topbar-sep" aria-hidden="true" />
          <span
            className="topbar-item topbar-status"
            data-tone={shellStatus.tone}
            data-phase={session.phase || (statusReady ? 'idle' : '')}
            role="status"
          >
            <span className="status-dot" aria-hidden="true" />
            {shellStatus.label}
          </span>
          <span className="topbar-spacer" />
          <button
            type="button"
            className="topbar-search"
            aria-label={t('palette.open')}
            aria-keyshortcuts="Control+K"
            onClick={() => setPaletteOpen(true)}
          >
            <Icon name="search" />
            <span>{t('palette.search')}</span>
          </button>
          <TipsPopover />
          <div className="theme-switch" role="group" aria-label={t('shell.theme')}>
            <button
              type="button"
              aria-pressed={resolvedTheme === 'light'}
              aria-label={t('shell.lightTheme')}
              onClick={() => setTheme('light')}
            >
              <Icon name="sun" />
              <span>{t('shell.light')}</span>
            </button>
            <button
              type="button"
              aria-pressed={resolvedTheme === 'dark'}
              aria-label={t('shell.darkTheme')}
              onClick={() => setTheme('dark')}
            >
              <Icon name="moon" />
              <span>{t('shell.dark')}</span>
            </button>
          </div>
        </div>
      </header>
      <aside className="app-rail">
        <nav className="app-nav" aria-label={t('nav.landmark')} role="navigation">
          <ul>
            {PRIMARY_SCREENS.map((id) => (
              <li key={id}>
                <NavButton id={id} live={(id === 'home' || id === 'dictation') && live && screen !== id} />
              </li>
            ))}
          </ul>
          <div className="nav-footer">
            {SECONDARY_SCREENS.map((id) => (
              <NavButton key={id} id={id} />
            ))}
            <VersionButton />
          </div>
        </nav>
      </aside>
      <main className={`main-pane${screen === 'assistant' ? ' is-flush' : ''}`} id="main-content" role="main" tabIndex={-1}>
        <h1 className="sr-only">{t(SCREEN_TITLE_KEYS[screen])}</h1>
        <ActiveScreen />
      </main>
      <StatusStrip />
      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} />
    </div>
  );
}
