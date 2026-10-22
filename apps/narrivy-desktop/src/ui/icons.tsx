// One coherent line-icon set (24px grid, 1.6 stroke, round joins). Icons are
// decorative: callers supply the accessible name on the control.
import type { ReactNode } from 'react';

const PATHS = {
  home: <path d="M4 10.5 12 4l8 6.5V20h-5.5v-5.5h-5V20H4Z" />,
  history: (
    <>
      <path d="M4 5v5h5" />
      <path d="M5.2 14.5A7.5 7.5 0 1 0 6 7.8L4 10" />
      <path d="M12 8v4.5l3 2" />
    </>
  ),
  models: (
    <>
      <path d="m12 3 8 4.5v9L12 21l-8-4.5v-9Z" />
      <path d="m4.4 7.6 7.6 4.3 7.6-4.3M12 12v9" />
    </>
  ),
  settings: (
    <>
      <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
      <circle cx="15" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </>
  ),
  diagnostics: <path d="M3 12h4l2.5-6 5 12 2.5-6h4" />,
  about: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 11v5.5M12 7.8v.2" />
    </>
  ),
  mic: (
    <>
      <rect x="9" y="3" width="6" height="11.5" rx="3" />
      <path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v3" />
    </>
  ),
  stop: <rect x="7" y="7" width="10" height="10" rx="2.2" />,
  close: <path d="M6 6l12 12M18 6 6 18" />,
  check: <path d="m5 12.5 4.5 4.5L19 7.5" />,
  copy: (
    <>
      <rect x="8.5" y="8.5" width="11" height="11" rx="2.2" />
      <path d="M15.5 8.5V6.2a1.7 1.7 0 0 0-1.7-1.7H6.2a1.7 1.7 0 0 0-1.7 1.7v7.6a1.7 1.7 0 0 0 1.7 1.7h2.3" />
    </>
  ),
  edit: <path d="M5 19h3.5L19 8.5 15.5 5 5 15.5ZM13.5 7l3.5 3.5" />,
  undo: <path d="M9 14 4.5 9.5 9 5M5 9.5h9a5.5 5.5 0 0 1 0 11h-3" />,
  refresh: <path d="M19.5 8A8 8 0 0 0 5 7.5M4.5 16a8 8 0 0 0 14.5.5M19.5 4v4h-4M4.5 20v-4h4" />,
  lock: (
    <>
      <rect x="5" y="10.5" width="14" height="10" rx="2.2" />
      <path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5" />
    </>
  ),
  shield: <path d="M12 3.5 19 6v5.5c0 4.4-2.9 7.7-7 9-4.1-1.3-7-4.6-7-9V6Z" />,
  sun: (
    <>
      <circle cx="12" cy="12" r="3.5" />
      <path d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6 7 7M17 17l1.4 1.4M18.4 5.6 17 7M7 17l-1.4 1.4" />
    </>
  ),
  moon: <path d="M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5Z" />,
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m16 16 4 4" />
    </>
  ),
  alert: (
    <>
      <path d="M12 4 21 19.5H3Z" />
      <path d="M12 10v4.5M12 17v.2" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 11v5M12 8v.2" />
    </>
  ),
  keyboard: (
    <>
      <rect x="3" y="6" width="18" height="12" rx="2.2" />
      <path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10" />
    </>
  ),
  type: <path d="M5 7V5h14v2M12 5v14M9 19h6" />,
  cpu: (
    <>
      <rect x="7" y="7" width="10" height="10" rx="2" />
      <path d="M10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4" />
    </>
  ),
  spark: <path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M18 6l-2.5 2.5M8.5 15.5 6 18" />,
  arrowRight: <path d="M5 12h14M13 6l6 6-6 6" />,
  people: (
    <>
      <circle cx="9" cy="8" r="3.2" />
      <path d="M3.5 19c.6-3 2.8-4.8 5.5-4.8s4.9 1.8 5.5 4.8" />
      <circle cx="17" cy="9" r="2.5" />
      <path d="M15.6 14.4c2.4-.3 4.3 1.2 4.9 4.1" />
    </>
  ),
  folder: <path d="M3.5 7.5A1.5 1.5 0 0 1 5 6h4l2 2h8a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 19H5a1.5 1.5 0 0 1-1.5-1.5Z" />,
  pencil: <path d="M4 20h4L19 9l-4-4L4 16v4ZM13.5 6.5l4 4" />,
  play: <path d="M8 5.5v13l10.5-6.5Z" />,
  pause: <path d="M9 6v12M15 6v12" />,
  pin: <path d="M14.5 4.5 19.5 9.5l-3 1-3.5 3.5.5 4-2 1-8-8 1-2 4 .5 3.5-3.5ZM8.5 15.5 4.5 19.5" />,
  chevron: <path d="m7 10 5 5 5-5" />,
  sidebar: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
      <path d="M9.5 4.5v15" />
    </>
  ),
  transcript: <path d="M5 7h14M5 11h14M5 15h9M5 19h6" />,
  speaker: (
    <>
      <path d="M4 9.5h3.5L12 6v12l-4.5-3.5H4Z" />
      <path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11" />
    </>
  ),
  command: <path d="M9 6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3Z" />,
  inbox: (
    <>
      <path d="M4 13.5 6.5 5h11L20 13.5V19H4Z" />
      <path d="M4 13.5h4.5l1 2h5l1-2H20" />
    </>
  ),
  pipeline: (
    <>
      <rect x="9" y="3" width="6" height="5" rx="1.4" />
      <rect x="3" y="16" width="6" height="5" rx="1.4" />
      <rect x="15" y="16" width="6" height="5" rx="1.4" />
      <path d="M12 8v4M6 16v-2h12v2" />
    </>
  ),
  apps: (
    <>
      <rect x="3.5" y="3.5" width="17" height="17" rx="2.5" />
      <path d="M3.5 10h17M10 3.5v17" />
    </>
  ),
  gear: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z" />
    </>
  ),
  pulse: (
    <>
      <path d="M19.5 12.6 12 20l-7.5-7.4A4.6 4.6 0 0 1 12 6.4a4.6 4.6 0 0 1 7.5 6.2Z" />
      <path d="M4 12.5h4l1.5-2.5 3 5 1.5-2.5h6" />
    </>
  ),
  bookmark: <path d="M7 4h10v16l-5-3.5L7 20Z" />,
  dots: <path d="M6 12h.01M12 12h.01M18 12h.01" />,
  paste: (
    <>
      <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3" />
      <path d="M4.5 4.5v4h4" />
    </>
  ),
  bulb: (
    <>
      <path d="M9 18h6M10 21h4" />
      <path d="M12 3a6 6 0 0 0-3.6 10.8c.6.5 1 1.2 1 2V16h5.2v-.2c0-.8.4-1.5 1-2A6 6 0 0 0 12 3Z" />
    </>
  ),
  send: <path d="M21 3 10 14M21 3l-7 18-4-7-7-4Z" />,
  sliders: (
    <>
      <path d="M4 7h16M4 12h16M4 17h16" />
      <circle cx="9" cy="7" r="2" />
      <circle cx="15" cy="12" r="2" />
      <circle cx="8" cy="17" r="2" />
    </>
  ),
  wave: <path d="M3 12h2l2-5 3 11 3-14 3 12 2-4h3" />,
  doc: (
    <>
      <path d="M7 3.5h7l4 4V20a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1Z" />
      <path d="M14 3.5v4h4M9 12h6M9 16h6" />
    </>
  ),
  gauge: (
    <>
      <path d="M4 16a8 8 0 1 1 16 0" />
      <path d="m12 16 4-5" />
    </>
  ),
  memory: (
    <>
      <rect x="3" y="7" width="18" height="10" rx="1.6" />
      <path d="M7 10v4M11 10v4M15 10v4M7 17v2M12 17v2M17 17v2" />
    </>
  ),
  checkCircle: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="m8.5 12.2 2.4 2.4 4.6-5" />
    </>
  ),
  chevronDown: <path d="m6 9 6 6 6-6" />,
  plus: <path d="M12 5v14M5 12h14" />,
  download: <path d="M12 4v11M7 10l5 5 5-5M5 20h14" />,
  clipboard: (
    <>
      <rect x="6" y="4.5" width="12" height="16" rx="2" />
      <path d="M9 4.5V3.5h6v1M9 10h6M9 14h4" />
    </>
  ),
  cloudOff: <path d="M3 3l18 18M8 7.5A5 5 0 0 1 17 9h.5a3.5 3.5 0 0 1 2.2 6.2M16 17H7a4 4 0 0 1-1.5-7.7" />,
  trash: <path d="M5 7h14M10 7V5h4v2M7 7l1 12.5a1 1 0 0 0 1 .9h6a1 1 0 0 0 1-.9L17 7M10.5 11v6M13.5 11v6" />,
  grip: <path d="M9 6h.01M15 6h.01M9 12h.01M15 12h.01M9 18h.01M15 18h.01" />,
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </>
  ),
  store: (
    <>
      <path d="M5 8h14l-1.2 11.2a2 2 0 0 1-2 1.8H8.2a2 2 0 0 1-2-1.8Z" />
      <path d="M9 10V7a3 3 0 0 1 6 0v3" />
    </>
  ),
  drive: (
    <>
      <rect x="3" y="13" width="18" height="7" rx="2" />
      <path d="M5.5 13 8 5h8l2.5 8M7 16.5h.01M11 16.5h6" />
    </>
  ),
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof PATHS;

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      {PATHS[name]}
    </svg>
  );
}

/** Narrivy mark: a voice waveform in the accent gradient (approved design). */
export function BrandMark({ className = 'brand-mark' }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 32 32" aria-hidden="true" focusable="false">
      <defs>
        <linearGradient id="narrivy-mark" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor="var(--orb-idle-a)" />
          <stop offset="1" stopColor="var(--accent-strong)" />
        </linearGradient>
      </defs>
      <path
        d="M2 17h3l2.2-6 3 13 3.2-19 3.2 22 3-15 2.6 9 2-4h5.8"
        stroke="url(#narrivy-mark)"
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
      />
    </svg>
  );
}
