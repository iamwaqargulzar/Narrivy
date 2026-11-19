// The overlay window's page on hosts without the native Linux card
// (Windows). The shell decides what to show and when (`webview_overlay.rs`);
// this page only draws the view it is sent: the logo's waveform in a pill,
// a running timer while listening, the live or cleaned text, and a label
// when the user has to act (docs/product/ux/overlay.md).
import { useEffect, useState, type CSSProperties } from "react";
import { onOverlayView, type OverlayView } from "../lib/bridge";
import "./overlay.css";

/** The newest words of a long text, so the card stays one or two lines. */
const TAIL_WORDS = 16;

export function elapsedLabel(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function textTail(text: string): string {
  const words = text.trim().split(/\s+/);
  return words.length > TAIL_WORDS ? `… ${words.slice(-TAIL_WORDS).join(" ")}` : words.join(" ");
}

export function Overlay() {
  const [view, setView] = useState<OverlayView | null>(null);
  const [received, setReceived] = useState(0);
  const [now, setNow] = useState(0);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let closed = false;
    void onOverlayView((next) => {
      setView(next);
      setReceived(performance.now());
      setNow(performance.now());
    }).then((stop) => {
      if (closed) stop();
      else unlisten = stop;
    });
    return () => {
      closed = true;
      unlisten?.();
    };
  }, []);

  const listening = view?.mode === "listening" && view.listeningMs !== null;
  useEffect(() => {
    if (!listening) return undefined;
    const timer = window.setInterval(() => setNow(performance.now()), 250);
    return () => window.clearInterval(timer);
  }, [listening]);

  if (!view) return null;
  const p = view.palette;
  const style = {
    "--ov-top": p.surfaceTop,
    "--ov-bottom": p.surfaceBottom,
    "--ov-border": p.border,
    "--ov-highlight": p.highlight,
    "--ov-shadow": p.shadow,
    "--ov-text": p.textStrong,
    "--ov-muted": p.mutedStrong,
    "--ov-accent": p.accent,
    "--ov-accent-strong": p.accentStrong,
    "--ov-live": p.live,
    "--ov-err": p.err,
  } as CSSProperties;
  const elapsed = listening ? elapsedLabel((view.listeningMs ?? 0) + (now - received)) : null;
  const text = view.text ? textTail(view.text) : null;

  return (
    <div className={`ov-stage ${view.top ? "is-top" : "is-bottom"}`} style={style}>
      <div className={`ov-card is-${view.mode}${text ? " has-text" : ""}`} role="status" aria-live="polite">
        <div className="ov-row">
          <svg className="ov-mark" viewBox="0 0 32 32" aria-hidden="true" focusable="false">
            <defs>
              <linearGradient id="ov-mark-gradient" x1="0" y1="0" x2="1" y2="0">
                <stop offset="0" stopColor="var(--ov-accent)" />
                <stop offset="1" stopColor="var(--ov-accent-strong)" />
              </linearGradient>
            </defs>
            <path
              d="M2 17h3l2.2-6 3 13 3.2-19 3.2 22 3-15 2.6 9 2-4h5.8"
              stroke={view.mode === "error" ? "var(--ov-err)" : "url(#ov-mark-gradient)"}
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
              fill="none"
            />
          </svg>
          {elapsed ? <span className="ov-timer">{elapsed}</span> : null}
          {view.label ? <span className="ov-label">{view.label}</span> : null}
        </div>
        {text ? <p className="ov-text">{text}</p> : null}
      </div>
    </div>
  );
}
