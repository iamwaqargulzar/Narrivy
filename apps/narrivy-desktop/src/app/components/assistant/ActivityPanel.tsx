// What the assistant did for its replies: its thinking, each search with
// its results, each page read, each file and command, with the sources at
// the end. It follows the reply being written.
import { useEffect, useRef, useState } from 'react';
import { Icon } from '../../../ui/icons';
import { t } from '../../../i18n';
import { useAssistant, type Chat, type Turn, type ToolUse } from '../../../state/AssistantContext';
import { useLive } from './LiveSession';

/** The thinking in what a reply streamed: its `<think>` parts (the opening
 * tag may be in the prompt, not the stream) and what it wrote before each
 * tool call. */
export function thoughtsOf(raw: string): string[] {
  const thoughts: string[] = [];
  let rest = raw;
  const firstOpen = rest.indexOf('<think>');
  const firstClose = rest.indexOf('</think>');
  if (firstClose >= 0 && (firstOpen < 0 || firstClose < firstOpen)) {
    thoughts.push(rest.slice(0, firstClose));
    rest = rest.slice(firstClose + '</think>'.length);
  }
  let outside = '';
  for (;;) {
    const open = rest.indexOf('<think>');
    if (open < 0) {
      outside += rest;
      break;
    }
    outside += rest.slice(0, open);
    const after = rest.slice(open + '<think>'.length);
    const close = after.indexOf('</think>');
    thoughts.push(close < 0 ? after : after.slice(0, close));
    if (close < 0) break;
    rest = after.slice(close + '</think>'.length);
  }
  // Words before a call: the model saying what it is about to do.
  const calls = outside.split(/<tool_call>[\s\S]*?(?:<\/tool_call>|$)/);
  calls.slice(0, -1).forEach((part) => thoughts.push(part));
  return thoughts.map((part) => part.trim()).filter((part) => part.length > 0);
}

/** The answer in what a reply streamed: without its thinking and calls. */
function answerOf(raw: string): string {
  return raw
    .replace(/^[\s\S]*?<\/think>/, (head) => (head.includes('<think>') ? head : ''))
    .replace(/<think>[\s\S]*?(<\/think>|$)/g, '')
    .replace(/[^]*?<tool_call>[\s\S]*?(<\/tool_call>|$)/g, '')
    .trim();
}

function words(text: string): number {
  return text.split(/\s+/).filter(Boolean).length;
}

/** When each reply got its first piece and ended, by turn: kept while the
 * app runs, so the panel can be closed and opened again. */
const moments = new Map<string, { first?: number; end?: number }>();

/** A clock that ticks every second while `on`. */
function useNow(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [on]);
  return now;
}

/** Long values (a file being written) shown by their last lines. */
function tailed(parameters: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(parameters)) {
    const lines = value.split('\n');
    out[key] = lines.length > 24 ? `… ${lines.length - 24} lines above …\n${lines.slice(-24).join('\n')}` : value;
  }
  return out;
}

function seconds(ms: number): string {
  return t('assistant.activity.seconds', { n: Math.max(0, Math.round(ms / 1000)) });
}

/** The stages of a reply as they happen: reading what it was given,
 * thinking, the tools, writing the answer. Shown for every reply, with or
 * without thinking. */
function Progress({ chat, turn, question }: { chat: Chat; turn: Turn; question?: Turn }) {
  const { streamed, loadingModel } = useAssistant();
  const live = useLive();
  const working = turn.state === 'streaming';
  const now = useNow(working);
  const raw = streamed[turn.id] ?? '';
  const known = moments.get(turn.id) ?? {};
  if (raw && !known.first) known.first = Date.now();
  if (!working && !known.end && known.first) known.end = Date.now();
  moments.set(turn.id, known);
  const asked = question?.at;
  // A reply not streamed this session (a kept chat) counts its saved text.
  const answer = raw ? answerOf(raw) : turn.text;
  const answered = words(answer);
  const reading = working && !raw && !(turn.tools?.length ?? 0);
  // What it was given with the question.
  const given: string[] = [];
  if (chat.live && live.id === chat.live) {
    if (live.turns.length) given.push(t('assistant.activity.givenTranscript', { n: live.turns.length }));
    const screens = Object.keys(live.pictures).length;
    if (screens) given.push(t('assistant.activity.givenScreens', { n: screens }));
  }
  if (chat.meeting) given.push(t('assistant.activity.givenMeeting'));
  if (question?.pictures?.length) given.push(t('assistant.activity.givenPictures', { n: question.pictures.length }));
  const earlier = chat.turns.indexOf(turn) - 1;
  if (earlier > 0) given.push(t('assistant.activity.givenEarlier', { n: earlier }));
  const readFor = asked && known.first ? known.first - asked : asked && reading ? now - asked : null;
  const calling = working ? pendingCall(raw) : null;
  return (
    <ol className="activity-progress" data-testid="activity-progress">
      {loadingModel && reading ? (
        <li data-state="active" data-spin="">
          <Icon name="refresh" />
          <span>{t('assistant.activity.loading')}</span>
        </li>
      ) : null}
      <li data-state={reading ? 'active' : 'done'} data-spin={reading ? '' : undefined}>
        <Icon name={reading ? 'refresh' : 'check'} />
        <span>
          {reading ? t('assistant.activity.reading') : t('assistant.activity.read_')}
          {given.length ? <span className="source"> · {given.join(', ')}</span> : null}
          {readFor !== null ? <span className="source"> · {seconds(readFor)}</span> : null}
        </span>
      </li>
      {calling ? (
        <li data-state="active" className="activity-calling">
          <Icon name="command" />
          <span>
            {t('assistant.activity.calling', { tool: calling.name })}
            <pre className="assistant-code activity-json" data-testid="activity-calling">
              {JSON.stringify({ tool: calling.name, arguments: tailed(calling.parameters) }, null, 2)}
            </pre>
          </span>
        </li>
      ) : null}
      {answered > 0 || (!working && turn.state === 'done') ? (
        <li data-state={working ? 'active' : 'done'}>
          <Icon name={working ? 'pencil' : 'check'} />
          <span>
            {working ? t('assistant.activity.writing', { n: answered }) : t('assistant.activity.wroteAnswer', { n: answered })}
            {!working && known.first && known.end && asked ? (
              <span className="source"> · {seconds(known.end - asked)}</span>
            ) : null}
          </span>
        </li>
      ) : null}
    </ol>
  );
}

/** The reply the panel follows: the newest that is being written, used
 * tools, or was written while the app was open. */
function followed(chat: Chat, streamed: Record<string, string>): Turn | undefined {
  return [...chat.turns]
    .reverse()
    .find(
      (turn) =>
        turn.role === 'assistant' &&
        // Being written, used tools, or written while the app was open.
        (turn.state === 'streaming' || (turn.tools?.length ?? 0) > 0 || streamed[turn.id] !== undefined),
    );
}

/** Whether the chat has anything for the panel to show. */
export function hasActivity(chat: Chat | null, streamed: Record<string, string>): boolean {
  return chat !== null && followed(chat, streamed) !== undefined;
}

interface Detail {
  kind?: string;
  query?: string;
  results?: Array<{ title: string; url: string; snippet: string }>;
  url?: string;
  title?: string;
  excerpt?: string;
  path?: string;
  entries?: string[];
  total?: number;
  command?: string;
  exit?: string;
  output?: string;
  bytes?: number;
  target?: string;
}

function parse(detail?: string): Detail {
  try {
    return (JSON.parse(detail ?? '{}') as Detail) ?? {};
  } catch {
    return {};
  }
}

function argument(use: ToolUse, key: string): string {
  try {
    const value = (JSON.parse(use.arguments) as Record<string, unknown>)[key];
    return typeof value === 'string' ? value : JSON.stringify(value ?? '');
  } catch {
    return '';
  }
}

function host(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, '');
  } catch {
    return url;
  }
}

const ICONS: Record<string, string> = {
  web_search: 'search',
  open_page: 'globe',
  list_folder: 'folder',
  find_files: 'search',
  read_file: 'doc',
  write_file: 'pencil',
  append_file: 'pencil',
  run_command: 'command',
  open: 'arrowRight',
  look_at_screen: 'apps',
  view_page: 'globe',
};

/** A call as JSON, the way the tool receives it. */
function callJson(use: ToolUse): string {
  let args: unknown = use.arguments;
  try {
    args = JSON.parse(use.arguments);
  } catch {
    // Shown as written.
  }
  return JSON.stringify({ tool: `${use.server}/${use.tool}`, arguments: args }, null, 2);
}

/** What came back, as JSON: the details the tool kept, else its summary. */
function resultJson(use: ToolUse): string | null {
  if (use.detail) {
    try {
      return JSON.stringify(JSON.parse(use.detail), null, 2);
    } catch {
      return use.detail;
    }
  }
  return use.summary ? JSON.stringify({ summary: use.summary }, null, 2) : null;
}

/** The call a reply is writing right now (it streams before it runs): its
 * tool and the parameters so far. */
export function pendingCall(raw: string): { name: string; parameters: Record<string, string> } | null {
  const start = raw.lastIndexOf('<tool_call>');
  if (start < 0 || raw.indexOf('</tool_call>', start) >= 0) return null;
  const block = raw.slice(start);
  const name = /<function=([^>\n]+)>/.exec(block)?.[1]?.trim();
  if (!name) return { name: '…', parameters: {} };
  const parameters: Record<string, string> = {};
  const found = /<parameter=([^>\n]+)>\n?([\s\S]*?)(?:\n?<\/parameter>|$)/g;
  for (let match = found.exec(block); match; match = found.exec(block)) {
    parameters[match[1].trim()] = match[2];
  }
  return { name, parameters };
}

function Step({ use }: { use: ToolUse }) {
  const detail = parse(use.detail);
  const result = resultJson(use);
  const title = (() => {
    switch (use.tool) {
      case 'web_search':
        return t('assistant.activity.searched', { query: argument(use, 'query') });
      case 'open_page':
        return t('assistant.activity.read', { page: detail.title || host(argument(use, 'url')) });
      case 'list_folder':
        return t('assistant.activity.listed', { path: argument(use, 'path') });
      case 'find_files':
        return t('assistant.activity.found', { name: argument(use, 'name'), folder: argument(use, 'folder') });
      case 'read_file':
        return t('assistant.activity.readFile', { path: argument(use, 'path') });
      case 'write_file':
        return t('assistant.activity.wrote', { path: argument(use, 'path') });
      case 'append_file':
        return t('assistant.activity.appended', { path: argument(use, 'path') });
      case 'run_command':
        return t('assistant.activity.ran');
      case 'open':
        return t('assistant.activity.opened', { target: argument(use, 'target') });
      case 'look_at_screen':
        return t('assistant.activity.looked');
      case 'view_page':
        return t('assistant.activity.viewed', { page: host(argument(use, 'target')) || argument(use, 'target') });
      default:
        return `${use.server} · ${use.tool}`;
    }
  })();
  return (
    <li className="activity-step" data-state={use.state}>
      <div className="activity-step-head">
        <Icon name={(ICONS[use.tool] ?? 'apps') as 'apps'} />
        <span className="activity-step-title">{title}</span>
        <span className="chip">{t(`assistant.tool.state.${use.state}`)}</span>
      </div>
      {use.tool === 'run_command' ? <pre className="assistant-code">{argument(use, 'command')}</pre> : null}
      {detail.kind === 'search' && detail.results?.length ? (
        <ol className="activity-results">
          {detail.results.map((result) => (
            <li key={result.url}>
              <span className="activity-result-title">{result.title}</span>
              <span className="source activity-url" title={result.url}>
                {host(result.url)}
              </span>
              {result.snippet ? <span className="activity-snippet">{result.snippet}</span> : null}
            </li>
          ))}
        </ol>
      ) : null}
      {detail.kind === 'page' ? (
        <div className="activity-page">
          <span className="source activity-url" title={detail.url}>
            {detail.url}
          </span>
          {detail.excerpt ? <p className="activity-snippet">{detail.excerpt}…</p> : null}
        </div>
      ) : null}
      {detail.kind === 'files' && detail.entries ? (
        <ul className="activity-files">
          {detail.entries.slice(0, 30).map((entry) => (
            <li key={entry}>{entry}</li>
          ))}
          {(detail.total ?? 0) > 30 ? <li className="source">{t('assistant.activity.more', { n: (detail.total ?? 0) - 30 })}</li> : null}
        </ul>
      ) : null}
      {detail.kind === 'view' ? (
        <div className="activity-page">
          <span className="source activity-url" title={detail.url}>
            {detail.url}
          </span>
          {detail.output ? (
            <pre className="assistant-code activity-output">{detail.output}</pre>
          ) : (
            <p className="activity-snippet">{t('assistant.activity.noErrors')}</p>
          )}
        </div>
      ) : null}
      {detail.kind === 'file' && detail.excerpt ? <pre className="assistant-code activity-output">{detail.excerpt}</pre> : null}
      {detail.kind === 'command' ? (
        <pre className="assistant-code activity-output">{`exit ${detail.exit}\n${detail.output ?? ''}`}</pre>
      ) : null}
      {!detail.kind && use.summary ? <p className="activity-snippet">{use.summary}</p> : null}
      <details className="activity-raw">
        <summary>{t('assistant.activity.call')}</summary>
        <pre className="assistant-code activity-json">{callJson(use)}</pre>
      </details>
      {result ? (
        <details className="activity-raw">
          <summary>{t('assistant.activity.result')}</summary>
          <pre className="assistant-code activity-json">{result}</pre>
        </details>
      ) : null}
    </li>
  );
}

export function ActivityPanel({ chat }: { chat: Chat }) {
  const { streamed } = useAssistant();
  const end = useRef<HTMLDivElement>(null);
  const thoughtsEnd = useRef<HTMLDivElement>(null);
  const latest = followed(chat, streamed);
  const steps = latest?.tools ?? [];
  const thoughts = latest ? thoughtsOf(streamed[latest.id] ?? '') : [];
  const working = latest?.state === 'streaming';
  const thinkingLength = thoughts.join('').length;
  const sources = steps
    .filter((use) => use.tool === 'open_page' && use.state === 'done')
    .map((use) => parse(use.detail))
    .filter((detail) => detail.url);
  useEffect(() => {
    end.current?.scrollIntoView?.({ block: 'nearest' });
  }, [steps.length]);
  useEffect(() => {
    if (working) thoughtsEnd.current?.scrollIntoView?.({ block: 'nearest' });
  }, [thinkingLength, working]);
  if (!latest) return null;
  const question = chat.turns[chat.turns.indexOf(latest) - 1];
  return (
    <section className="activity-section" aria-label={t('assistant.activity.title')} aria-live="polite">
      <h3>{t('assistant.activity.title')}</h3>
      {question?.role === 'user' ? <p className="source activity-question">{question.text}</p> : null}
      <Progress chat={chat} turn={latest} question={question?.role === 'user' ? question : undefined} />
      {thoughts.length ? (
        <details className="activity-thinking" open={working}>
          <summary>
            <Icon name="spark" />
            {working ? t('assistant.activity.thinkingNow') : t('assistant.activity.thought')}
          </summary>
          <div className="activity-thoughts" data-testid="activity-thinking">
            {thoughts.map((thought, i) => (
              <p key={i}>{thought}</p>
            ))}
            <div ref={thoughtsEnd} />
          </div>
        </details>
      ) : null}
      <ol className="activity-steps">
        {steps.map((use, i) => (
          <Step key={use.callId ?? i} use={use} />
        ))}
      </ol>
      {sources.length ? (
        <>
          <h4>{t('assistant.activity.sources')}</h4>
          <ol className="activity-sources">
            {sources.map((source) => (
              <li key={source.url}>
                <span className="activity-result-title">{source.title || host(source.url ?? '')}</span>
                <span className="source activity-url">{source.url}</span>
              </li>
            ))}
          </ol>
        </>
      ) : null}
      <div ref={end} />
    </section>
  );
}
