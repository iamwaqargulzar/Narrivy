// Ctrl/Cmd+K overlay: substring-filtered actions, Enter runs, Esc closes,
// arrow keys move the selection.
import { useEffect, useMemo, useRef, useState } from 'react';
import { t } from '../i18n';
import { NAV_LABEL_KEYS, SCREENS, useNav, type ScreenId } from '../state/NavContext';
import { useDiagnostics } from '../state/DiagnosticsContext';
import { useTheme } from '../theme/ThemeProvider';
import { useDictationActions } from '../hooks/useDictationActions';

interface PaletteAction {
  id: string;
  label: string;
  keywords?: string;
  run: () => void;
}

export function CommandPalette({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { navigate } = useNav();
  const { cycleTheme } = useTheme();
  const { runAll } = useDiagnostics();
  const dictation = useDictationActions();
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setSelected(0);
    // Focus after mount; give it back to where it was when the palette closes.
    const previous = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => inputRef.current?.focus());
    return () => {
      previous?.focus?.();
    };
  }, [open]);

  const actions = useMemo<PaletteAction[]>(() => {
    const navActions: PaletteAction[] = SCREENS.map((screen: ScreenId) => ({
      id: `go:${screen}`,
      label: t('palette.goTo', { screen: t(NAV_LABEL_KEYS[screen]) }),
      keywords: screen,
      run: () => navigate(screen),
    }));
    return [
      ...navActions,
      { id: 'theme', label: t('palette.toggleTheme'), keywords: 'dark light system appearance theme', run: cycleTheme },
      // Start/cancel use the same advertised pipeline selection as Home and
      // Dictation; the command never substitutes a default lane.
      { id: 'start', label: t('palette.startDictation'), keywords: 'dictation record start begin', run: () => void dictation.start() },
      { id: 'cancel', label: t('palette.cancelDictation'), keywords: 'dictation stop abort cancel', run: () => void dictation.cancel() },
      {
        id: 'diag',
        label: t('palette.runDiagnostics'),
        keywords: 'diagnostics checks mic delivery shortcuts theme',
        run: () => {
          void runAll();
          navigate('diagnostics');
        },
      },
    ];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navigate, cycleTheme, runAll, dictation.start, dictation.cancel]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions;
    return actions.filter(
      (a) => a.label.toLowerCase().includes(q) || (a.keywords ?? '').toLowerCase().includes(q),
    );
  }, [actions, query]);

  if (!open) return null;

  const execute = (action: PaletteAction | undefined) => {
    if (!action) return;
    onClose();
    action.run();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'Tab') {
      // The search box is the only focusable element of this modal dialog:
      // Tab and Shift+Tab stay in it rather than reaching the page behind
      // (F-M11). The list is navigated with the arrow keys.
      e.preventDefault();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelected((prev) => (filtered.length === 0 ? 0 : (prev + 1) % filtered.length));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelected((prev) => (filtered.length === 0 ? 0 : (prev - 1 + filtered.length) % filtered.length));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      execute(filtered[selected]);
    }
  };

  return (
    <div className="palette-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label={t('palette.title')}>
        <input
          ref={inputRef}
          role="combobox"
          aria-expanded={filtered.length > 0}
          aria-controls="palette-list"
          aria-autocomplete="list"
          aria-activedescendant={filtered[selected] ? `palette-option-${selected}` : undefined}
          value={query}
          placeholder={t('palette.placeholder')}
          onChange={(e) => {
            setQuery(e.target.value);
            setSelected(0);
          }}
          onKeyDown={onKeyDown}
          aria-label={t('palette.placeholder')}
        />
        {filtered.length === 0 ? (
          <div className="palette-empty" role="status">{t('palette.empty')}</div>
        ) : (
          <ul id="palette-list" role="listbox" aria-label={t('palette.title')}>
            {filtered.map((action, index) => (
              <li
                key={action.id}
                id={`palette-option-${index}`}
                role="option"
                aria-selected={index === selected}
                onMouseEnter={() => setSelected(index)}
                onClick={() => execute(action)}
              >
                {action.label}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
