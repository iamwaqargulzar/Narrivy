// "Press the keys": the dialog that captures a key combination for one
// pipeline and asks the service to bind it. Keys are read only while this
// dialog is open and only to build the combination; nothing else is kept.
// The service decides whether a combination is allowed or already in use;
// a combination in use is never replaced without the user saying so here.
import { useEffect, useRef, useState } from 'react';
import { t } from '../../i18n';
import { bridge, type ShortcutBinding, type ShortcutSetResult } from '../../lib/bridge';
import { MODIFIER_ORDER, comboParts, pressedKeys, type PressedKeys } from '../../lib/shortcutKeys';
import { useErrorHandler } from '../../ui/primitives/Toast';

export interface CaptureTarget {
  pipelineId: string;
  pipelineName: string;
  spokenLanguage?: string;
  outputLanguage?: string;
}

export function KeyChips({ parts }: { parts: string[] }) {
  return (
    <span className="key-chips">
      {parts.map((part, index) => (
        <span key={`${part}-${index}`} className="key-chip">
          {part}
        </span>
      ))}
    </span>
  );
}

export function ShortcutCapture({ target, onClose, onSaved }: {
  target: CaptureTarget;
  onClose: () => void;
  onSaved: () => void;
}) {
  const report = useErrorHandler();
  const dialogRef = useRef<HTMLDivElement>(null);
  const [held, setHeld] = useState<PressedKeys>({ modifiers: [], key: null });
  const [combo, setCombo] = useState<{ modifiers: string[]; key: string } | null>(null);
  const [result, setResult] = useState<ShortcutSetResult | null>(null);
  const [saving, setSaving] = useState(false);
  // Every modifier held since the last release of all keys: releasing them
  // without pressing another key captures the modifiers alone.
  const peak = useRef<string[]>([]);

  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  const onKeyDown = (event: React.KeyboardEvent) => {
    // Tab and the buttons keep working once a combination is on screen.
    if (combo && (event.key === 'Tab' || event.key === 'Enter' || event.key === ' ')) return;
    event.preventDefault();
    event.stopPropagation();
    const pressed = pressedKeys(event.nativeEvent);
    if (event.key === 'Escape' && pressed.modifiers.length === 0) {
      onClose();
      return;
    }
    setHeld(pressed);
    if (pressed.key) {
      peak.current = [];
      setCombo({ modifiers: pressed.modifiers, key: pressed.key });
      setResult(null);
    } else if (!combo) {
      peak.current = MODIFIER_ORDER.filter(
        (name) => peak.current.includes(name) || pressed.modifiers.includes(name),
      );
    }
  };

  const onKeyUp = (event: React.KeyboardEvent) => {
    if (combo) return;
    const pressed = pressedKeys(event.nativeEvent);
    setHeld({ ...pressed, key: null });
    if (pressed.modifiers.length === 0 && peak.current.length > 0) {
      setCombo({ modifiers: peak.current, key: '' });
      setResult(null);
      peak.current = [];
    }
  };

  const save = async (replace: boolean) => {
    if (!combo) return;
    const binding: ShortcutBinding = {
      modifiers: combo.modifiers,
      key: combo.key,
      pipeline_id: target.pipelineId,
      ...(target.spokenLanguage ? { spoken_language: target.spokenLanguage } : {}),
      ...(target.outputLanguage ? { output_language: target.outputLanguage } : {}),
    };
    setSaving(true);
    try {
      const outcome = await bridge.setShortcut(binding, replace);
      if (outcome.outcome === 'created' || outcome.outcome === 'replaced') {
        onSaved();
        return;
      }
      setResult(outcome);
    } catch (err) {
      report(err, { retry: () => void save(replace) });
    } finally {
      setSaving(false);
    }
  };

  const again = () => {
    setCombo(null);
    setResult(null);
    setHeld({ modifiers: [], key: null });
    peak.current = [];
    dialogRef.current?.focus();
  };

  const conflict = result?.outcome === 'conflict';
  const shown = combo ? comboParts(combo) : held.modifiers;

  return (
    <div className="modal-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div
        ref={dialogRef}
        className="modal shortcut-capture"
        role="dialog"
        aria-modal="true"
        aria-label={t('capture.title', { name: target.pipelineName })}
        tabIndex={-1}
        onKeyDown={onKeyDown}
        onKeyUp={onKeyUp}
      >
        <h2>{t('capture.title', { name: target.pipelineName })}</h2>
        <p>{combo ? t('capture.confirm') : t('capture.prompt')}</p>
        <div className="capture-well" data-filled={combo ? 'true' : 'false'} aria-live="polite">
          {shown.length > 0 ? <KeyChips parts={shown} /> : <span className="capture-placeholder">{t('capture.waiting')}</span>}
        </div>
        {result?.outcome === 'invalid' ? <p className="capture-note" role="alert">{t('capture.invalid')}</p> : null}
        {result?.outcome === 'unsupported' ? <p className="capture-note" role="alert">{t('capture.unsupported')}</p> : null}
        {conflict ? (
          <p className="capture-note" role="alert">
            {result?.conflict_kind === 'desktop'
              ? t('capture.conflictDesktop', { name: result.conflict_name ?? '' })
              : t('capture.conflictShortcut', { name: result?.conflict_name ?? '' })}
          </p>
        ) : null}
        <p className="capture-hint">{t('capture.hint')}</p>
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>
            {t('capture.cancel')}
          </button>
          {combo ? (
            <button type="button" className="btn" onClick={again}>
              {t('capture.again')}
            </button>
          ) : null}
          {conflict ? (
            <button type="button" className="btn btn-danger" disabled={saving} onClick={() => void save(true)}>
              {t('capture.replace')}
            </button>
          ) : (
            <button type="button" className="btn btn-primary" disabled={!combo || saving} onClick={() => void save(false)}>
              {t('capture.save')}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
