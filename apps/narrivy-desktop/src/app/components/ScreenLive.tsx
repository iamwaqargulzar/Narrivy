// What a meeting or a screen share is showing right now: a small live
// picture of each shared screen or window, so the user can see which one.
import { useEffect, useState } from 'react';
import { bridge } from '../../lib/bridge';
import { t } from '../../i18n';

interface Source {
  source: number;
  kind: string;
  width: number;
  height: number;
}

export function ScreenLive({ meetingId }: { meetingId: string }) {
  const [sources, setSources] = useState<Source[]>([]);
  const [pictures, setPictures] = useState<Record<number, string>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      try {
        const detail = await bridge.meeting(meetingId);
        if (!alive) return;
        setSources(detail.screen_sources ?? []);
        setError(detail.screen_error ?? null);
        const found: Record<number, string> = {};
        for (const source of detail.screen_sources ?? []) {
          try {
            const png = await bridge.meetingSlide(meetingId, `live-${source.source}.png`);
            if (png) found[source.source] = `data:image/png;base64,${png}`;
          } catch {
            // Not taken yet.
          }
        }
        if (alive) setPictures(found);
      } catch {
        // The meeting is gone or the service restarted.
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [meetingId]);

  if (error) return <p className="meeting-note meeting-error" role="alert">{t('meetings.screen.liveError', { code: error })}</p>;
  if (!sources.length) return <p className="source">{t('meetings.screen.choosing')}</p>;
  return (
    <ul className="screen-live" aria-label={t('meetings.screen.watching')}>
      {sources.map((source) => (
        <li key={source.source}>
          {pictures[source.source] ? (
            <img src={pictures[source.source]} alt={t('meetings.screen.liveAlt', { n: source.source + 1 })} />
          ) : (
            <span className="meeting-screen-placeholder" role="status">
              {t('meetings.screen.waitingPicture')}
            </span>
          )}
          <span className="source">
            {t(`meetings.screen.kind.${source.kind}`)} {source.width && source.height ? `· ${source.width}×${source.height}` : ''}
          </span>
        </li>
      ))}
    </ul>
  );
}
