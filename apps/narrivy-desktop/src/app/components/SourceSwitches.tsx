// While a meeting, a share or a recording runs: switch the screen and the
// microphone and the computer's sound on and off as the meeting goes, for
// example talking first and sharing the screen later. The screen asks which one to
// share only the first time.
import { useEffect, useState } from 'react';
import { bridge } from '../../lib/bridge';
import { t } from '../../i18n';
import { Icon } from '../../ui/icons';

export function SourceSwitches({
  meetingId,
  onChange,
}: {
  meetingId: string;
  onChange?: (on: { screen: boolean; sound: boolean; microphone: boolean }) => void;
}) {
  const [on, setOn] = useState<{ screen: boolean; sound: boolean; microphone: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    Promise.resolve()
      .then(() => bridge.meeting(meetingId))
      .then((detail) => {
        if (!alive) return;
        const now = {
          screen: Boolean(detail.meeting.screen_on),
          sound: Boolean(detail.meeting.sound_on),
          microphone: Boolean(detail.meeting.mic_on),
        };
        setOn(now);
        onChange?.(now);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
    // Asked once per meeting; onChange is the caller's to keep current.
  }, [meetingId]);

  if (!on) return null;
  const flip = (which: 'screen' | 'sound' | 'microphone') => {
    setBusy(true);
    setError(null);
    bridge
      .meetingSources(meetingId, { [which]: !on[which] })
      .then((meeting) => {
        const now = {
          screen: Boolean(meeting.screen_on),
          sound: Boolean(meeting.sound_on),
          microphone: Boolean(meeting.mic_on),
        };
        setOn(now);
        onChange?.(now);
      })
      .catch((e: unknown) => setError((e as { code?: string })?.code ?? 'unavailable'))
      .finally(() => setBusy(false));
  };
  return (
    <div className="source-switches">
      {(['microphone', 'sound', 'screen'] as const).map((which) => (
        <button
          key={which}
          type="button"
          role="switch"
          aria-checked={on[which]}
          className="source-switch"
          disabled={busy}
          title={t(`sources.${which}Hint`)}
          onClick={() => flip(which)}
        >
          <Icon name={which === 'screen' ? 'play' : which === 'sound' ? 'speaker' : 'mic'} />
          <span>{t(`sources.${which}`)}</span>
          <span className="switch" aria-hidden="true">
            <span className="switch-thumb" />
          </span>
        </button>
      ))}
      {error ? <span className="assistant-error" role="alert">{t('assistant.error.other', { code: error })}</span> : null}
    </div>
  );
}
