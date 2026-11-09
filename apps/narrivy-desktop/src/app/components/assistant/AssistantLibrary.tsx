// The assistant's library: what it remembers, its personas, projects, skills
// and routines, and how dictation into the chat box is handled. All of it is
// kept by the Narrivy service on this computer.
import { useEffect, useState, type ReactNode } from 'react';
import { bridge } from '../../../lib/bridge';
import { Icon } from '../../../ui/icons';
import { hasMessage, t } from '../../../i18n';
import {
  useAssistant,
  type Library,
  type LibraryKind,
  type Memory,
  type Persona,
  type Project,
  type Routine,
  type Skill,
  type ToolServer,
} from '../../../state/AssistantContext';

type Section = 'memory' | 'persona' | 'project' | 'skill' | 'routine' | 'tool' | 'web' | 'settings';

const SECTIONS: Section[] = ['memory', 'persona', 'project', 'skill', 'routine', 'tool', 'web', 'settings'];

export function AssistantLibraryView({ onOpenChat }: { onOpenChat: (id: string) => void }) {
  const [section, setSection] = useState<Section>('memory');
  return (
    <div className="assistant-library">
      <nav className="assistant-library-nav" aria-label={t('assistant.library')}>
        {SECTIONS.map((name) => (
          <button
            key={name}
            type="button"
            aria-current={section === name ? 'page' : undefined}
            onClick={() => setSection(name)}
          >
            {t(`assistant.lib.${name}.title`)}
          </button>
        ))}
      </nav>
      <div className="assistant-library-body">
        <h3>{t(`assistant.lib.${section}.title`)}</h3>
        <p className="page-lead">{t(`assistant.lib.${section}.lead`)}</p>
        {section === 'memory' ? <MemoryEditor /> : null}
        {section === 'persona' ? <PersonaEditor /> : null}
        {section === 'project' ? <ProjectEditor /> : null}
        {section === 'skill' ? <SkillEditor /> : null}
        {section === 'routine' ? <RoutineEditor onOpenChat={onOpenChat} /> : null}
        {section === 'tool' ? <ToolEditor /> : null}
        {section === 'web' ? <WebSettings /> : null}
        {section === 'settings' ? <AssistantSettings /> : null}
      </div>
    </div>
  );
}

/** A list of items with an editor for one at a time. */
function ItemList<K extends LibraryKind>({
  kind,
  blank,
  label,
  form,
  extra,
}: {
  kind: K;
  blank: Library[K][number];
  label: (item: Library[K][number]) => string;
  form: (item: Library[K][number], change: (next: Library[K][number]) => void) => ReactNode;
  extra?: (item: Library[K][number]) => ReactNode;
}) {
  const { library, saveItem, deleteItem } = useAssistant();
  const items = library[kind] as Library[K][number][];
  const [editing, setEditing] = useState<Library[K][number] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    if (!editing) return;
    setError(null);
    try {
      await saveItem(kind, editing);
      setEditing(null);
    } catch (failure) {
      setError((failure as { code?: string })?.code ?? 'assistant_library_bad_value');
    }
  };
  return (
    <div className="assistant-items">
      {editing ? (
        <div className="assistant-item-form">
          {form(editing, setEditing)}
          {error ? (
            <p className="assistant-meta assistant-error" role="alert">
              {hasMessage(`assistant.lib.error.${error}`)
                ? t(`assistant.lib.error.${error}`)
                : t('assistant.error.other', { code: error })}
            </p>
          ) : null}
          <div className="assistant-item-buttons">
            <button type="button" className="btn btn-default" onClick={() => setEditing(null)}>
              {t('assistant.cancel')}
            </button>
            <button type="button" className="btn btn-primary" onClick={() => void save()}>
              {t('assistant.lib.save')}
            </button>
          </div>
        </div>
      ) : (
        <button type="button" className="btn btn-primary" onClick={() => setEditing({ ...blank })}>
          <Icon name="plus" />
          {t(`assistant.lib.${kind}.add`)}
        </button>
      )}
      {items.length === 0 && !editing ? <p className="screen-empty-note">{t(`assistant.lib.${kind}.empty`)}</p> : null}
      <ul className="assistant-item-list">
        {items.map((item) => (
          <li key={(item as { id: string }).id}>
            <span className="assistant-item-label">{label(item)}</span>
            {extra ? extra(item) : null}
            <button type="button" className="btn btn-ghost" onClick={() => setEditing({ ...item })}>
              <Icon name="pencil" />
              {t('assistant.lib.edit')}
            </button>
            <button
              type="button"
              className="btn btn-ghost"
              onClick={() => void deleteItem(kind, (item as { id: string }).id)}
            >
              <Icon name="trash" />
              {t('assistant.lib.delete')}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="assistant-field">
      <span>{label}</span>
      {children}
    </label>
  );
}

function MemoryEditor() {
  return (
    <ItemList
      kind="memory"
      blank={{ id: '', text: '' } as Memory}
      label={(item) => item.text}
      form={(item, change) => (
        <Field label={t('assistant.lib.memory.text')}>
          <textarea rows={3} value={item.text} onChange={(event) => change({ ...item, text: event.target.value })} />
        </Field>
      )}
    />
  );
}

function PersonaEditor() {
  return (
    <ItemList
      kind="persona"
      blank={{ id: '', name: '', instructions: '' } as Persona}
      label={(item) => item.name}
      form={(item, change) => (
        <>
          <Field label={t('assistant.lib.name')}>
            <input type="text" value={item.name} onChange={(event) => change({ ...item, name: event.target.value })} />
          </Field>
          <Field label={t('assistant.lib.instructions')}>
            <textarea
              rows={6}
              value={item.instructions}
              placeholder={t('assistant.lib.persona.placeholder')}
              onChange={(event) => change({ ...item, instructions: event.target.value })}
            />
          </Field>
        </>
      )}
    />
  );
}

function ProjectEditor() {
  return (
    <ItemList
      kind="project"
      blank={{ id: '', name: '', folder: '', instructions: '' } as Project}
      label={(item) => (item.folder ? `${item.name} — ${item.folder}` : item.name)}
      form={(item, change) => (
        <>
          <Field label={t('assistant.lib.name')}>
            <input type="text" value={item.name} onChange={(event) => change({ ...item, name: event.target.value })} />
          </Field>
          <Field label={t('assistant.lib.project.folder')}>
            <input
              type="text"
              value={item.folder ?? ''}
              placeholder="/home/you/Documents/project"
              onChange={(event) => change({ ...item, folder: event.target.value })}
            />
          </Field>
          <Field label={t('assistant.lib.instructions')}>
            <textarea
              rows={5}
              value={item.instructions}
              onChange={(event) => change({ ...item, instructions: event.target.value })}
            />
          </Field>
        </>
      )}
    />
  );
}

function SkillEditor() {
  return (
    <ItemList
      kind="skill"
      blank={{ id: '', name: '', description: '', instructions: '' } as Skill}
      label={(item) => `/${item.name} — ${item.description}`}
      form={(item, change) => (
        <>
          <Field label={t('assistant.lib.skill.name')}>
            <input
              type="text"
              value={item.name}
              placeholder={t('assistant.lib.skill.namePlaceholder')}
              onChange={(event) => change({ ...item, name: event.target.value.replace(/[^\p{L}\p{N}_-]/gu, '') })}
            />
          </Field>
          <Field label={t('assistant.lib.skill.description')}>
            <input
              type="text"
              value={item.description}
              onChange={(event) => change({ ...item, description: event.target.value })}
            />
          </Field>
          <Field label={t('assistant.lib.instructions')}>
            <textarea
              rows={6}
              value={item.instructions}
              onChange={(event) => change({ ...item, instructions: event.target.value })}
            />
          </Field>
        </>
      )}
    />
  );
}

function ToolEditor() {
  return (
    <>
      <p className="assistant-attached">
        <Icon name="shield" />
        <span>{t('assistant.lib.tool.safety')}</span>
      </p>
      <ItemList
        kind="tool"
        blank={{ id: '', name: '', command: '', args: [] } as ToolServer}
        label={(item) => `${item.name} — ${[item.command, ...item.args].join(' ')}`}
        form={(item, change) => (
          <>
            <Field label={t('assistant.lib.name')}>
              <input type="text" value={item.name} onChange={(event) => change({ ...item, name: event.target.value })} />
            </Field>
            <Field label={t('assistant.lib.tool.command')}>
              <input
                type="text"
                value={item.command}
                placeholder="/usr/bin/npx"
                onChange={(event) => change({ ...item, command: event.target.value.trim() })}
              />
            </Field>
            <Field label={t('assistant.lib.tool.args')}>
              <textarea
                rows={4}
                value={item.args.join('\n')}
                placeholder={'-y\n@modelcontextprotocol/server-filesystem\n/home/you/Documents'}
                onChange={(event) =>
                  change({ ...item, args: event.target.value.split('\n').filter((line) => line.length > 0) })
                }
              />
            </Field>
          </>
        )}
      />
    </>
  );
}

const WEEKDAYS = [0, 1, 2, 3, 4, 5, 6];

function RoutineEditor({ onOpenChat }: { onOpenChat: (id: string) => void }) {
  const { library, runRoutine } = useAssistant();
  const [ran, setRan] = useState<string | null>(null);
  return (
    <ItemList
      kind="routine"
      blank={
        {
          id: '',
          name: '',
          prompt: '',
          every: 'daily',
          time: '08:00',
          weekday: 0,
          includeMeetings: false,
          enabled: true,
        } as Routine
      }
      label={(item) =>
        `${item.name} — ${t(`assistant.lib.routine.every.${item.every}`)} ${item.every === 'hourly' ? `:${item.time.slice(3)}` : item.time}${item.enabled ? '' : ` (${t('assistant.lib.routine.paused')})`}`
      }
      extra={(item) => (
        <>
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() =>
              void runRoutine(item.id).then(
                () => setRan(item.id),
                () => setRan(null),
              )
            }
          >
            <Icon name="play" />
            {ran === item.id ? t('assistant.lib.routine.running') : t('assistant.lib.routine.runNow')}
          </button>
          {item.chatId ? (
            <button type="button" className="btn btn-ghost" onClick={() => item.chatId && onOpenChat(item.chatId)}>
              <Icon name="doc" />
              {t('assistant.lib.routine.results')}
            </button>
          ) : null}
        </>
      )}
      form={(item, change) => (
        <>
          <Field label={t('assistant.lib.name')}>
            <input type="text" value={item.name} onChange={(event) => change({ ...item, name: event.target.value })} />
          </Field>
          <Field label={t('assistant.lib.routine.prompt')}>
            <textarea rows={4} value={item.prompt} onChange={(event) => change({ ...item, prompt: event.target.value })} />
          </Field>
          <div className="assistant-field-row">
            <Field label={t('assistant.lib.routine.when')}>
              <select
                className="pipe-select"
                value={item.every}
                onChange={(event) => change({ ...item, every: event.target.value as Routine['every'] })}
              >
                {(['hourly', 'daily', 'weekdays', 'weekly'] as const).map((every) => (
                  <option key={every} value={every}>
                    {t(`assistant.lib.routine.every.${every}`)}
                  </option>
                ))}
              </select>
            </Field>
            {item.every === 'weekly' ? (
              <Field label={t('assistant.lib.routine.day')}>
                <select
                  className="pipe-select"
                  value={item.weekday}
                  onChange={(event) => change({ ...item, weekday: Number(event.target.value) })}
                >
                  {WEEKDAYS.map((day) => (
                    <option key={day} value={day}>
                      {t(`assistant.lib.routine.weekday.${day}`)}
                    </option>
                  ))}
                </select>
              </Field>
            ) : null}
            <Field label={t('assistant.lib.routine.time')}>
              <input type="time" value={item.time} onChange={(event) => change({ ...item, time: event.target.value })} />
            </Field>
            {library.persona.length ? (
              <Field label={t('assistant.persona')}>
                <select
                  className="pipe-select"
                  value={item.persona ?? ''}
                  onChange={(event) => change({ ...item, persona: event.target.value || undefined })}
                >
                  <option value="">{t('assistant.personaNone')}</option>
                  {library.persona.map((persona) => (
                    <option key={persona.id} value={persona.id}>
                      {persona.name}
                    </option>
                  ))}
                </select>
              </Field>
            ) : null}
          </div>
          <label className="voice-check">
            <input
              type="checkbox"
              checked={item.includeMeetings}
              onChange={(event) => change({ ...item, includeMeetings: event.target.checked })}
            />
            {t('assistant.lib.routine.includeMeetings')}
          </label>
          <label className="voice-check">
            <input
              type="checkbox"
              checked={item.enabled}
              onChange={(event) => change({ ...item, enabled: event.target.checked })}
            />
            {t('assistant.lib.routine.enabled')}
          </label>
        </>
      )}
    />
  );
}

interface WebSettingsState {
  provider: string;
  searxng: string;
  braveKey: boolean;
}

/** Where web research searches: the engines, a Brave Search key kept in
 * the keyring, or the user's own SearXNG. */
function WebSettings() {
  const [settings, setSettings] = useState<WebSettingsState | null>(null);
  const [key, setKey] = useState('');
  const [searxng, setSearxng] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => {
    bridge
      .assistantLibrary('get', 'web')
      .then((json) => {
        const value = JSON.parse(json) as WebSettingsState;
        setSettings(value);
        setSearxng(value.searxng ?? '');
      })
      .catch(() => setSettings(null));
  }, []);
  const save = async (change: Record<string, string>) => {
    setMessage(null);
    try {
      const value = JSON.parse(await bridge.assistantLibrary('put', 'web', null, JSON.stringify(change))) as WebSettingsState;
      setSettings(value);
      setMessage(t('assistant.lib.web.saved'));
      setKey('');
    } catch (failure) {
      const code = (failure as { code?: string })?.code ?? 'unknown';
      setMessage(hasMessage(`assistant.lib.web.error.${code}`) ? t(`assistant.lib.web.error.${code}`) : t('assistant.error.other', { code }));
    }
  };
  if (!settings) return <p className="screen-empty-note">{t('assistant.error.olderService')}</p>;
  return (
    <div className="assistant-item-form">
      <Field label={t('assistant.lib.web.provider')}>
        <select className="pipe-select" value={settings.provider} onChange={(e) => void save({ provider: e.target.value })}>
          {['auto', 'brave', 'searxng', 'duckduckgo', 'wikipedia'].map((p) => (
            <option key={p} value={p}>
              {t(`assistant.lib.web.engine.${p}`)}
            </option>
          ))}
        </select>
      </Field>
      <Field label={t('assistant.lib.web.braveKey')}>
        <div className="assistant-field-row">
          <input
            type="password"
            autoComplete="off"
            value={key}
            placeholder={settings.braveKey ? t('assistant.lib.web.keySaved') : 'BSA…'}
            onChange={(e) => setKey(e.target.value)}
          />
          <button type="button" className="btn btn-primary" disabled={!key.trim()} onClick={() => void save({ braveKey: key })}>
            {t('assistant.lib.save')}
          </button>
          {settings.braveKey ? (
            <button type="button" className="btn btn-default" onClick={() => void save({ braveKey: '' })}>
              {t('assistant.lib.web.removeKey')}
            </button>
          ) : null}
        </div>
      </Field>
      <p className="source">{t('assistant.lib.web.braveHint')}</p>
      <Field label={t('assistant.lib.web.searxng')}>
        <div className="assistant-field-row">
          <input type="text" value={searxng} placeholder="http://localhost:8888" onChange={(e) => setSearxng(e.target.value)} />
          <button type="button" className="btn btn-default" onClick={() => void save({ searxng })}>
            {t('assistant.lib.save')}
          </button>
        </div>
      </Field>
      {message ? <p className="source" role="status">{message}</p> : null}
    </div>
  );
}

function AssistantSettings() {
  const { dictationMode, setDictationMode } = useAssistant();
  return (
    <div className="assistant-settings">
      <fieldset>
        <legend>{t('assistant.lib.settings.dictation')}</legend>
        <label className="voice-check">
          <input type="radio" name="dictation-mode" checked={dictationMode === 'edit'} onChange={() => setDictationMode('edit')} />
          {t('assistant.lib.settings.editFirst')}
        </label>
        <label className="voice-check">
          <input type="radio" name="dictation-mode" checked={dictationMode === 'send'} onChange={() => setDictationMode('send')} />
          {t('assistant.lib.settings.sendAtOnce')}
        </label>
      </fieldset>
      <h4>{t('assistant.lib.settings.shortcuts')}</h4>
      <dl className="assistant-shortcuts">
        {[
          ['Ctrl+Shift+V', 'assistant.lib.settings.keyVoice'],
          ['Ctrl+M', 'assistant.lib.settings.keyMute'],
          ['Space', 'assistant.lib.settings.keyTalk'],
          ['Esc', 'assistant.lib.settings.keyStop'],
          ['Alt+N', 'assistant.lib.settings.keyNew'],
          ['Enter / Shift+Enter', 'assistant.lib.settings.keySend'],
        ].map(([keys, label]) => (
          <div key={keys}>
            <dt>
              <span className="key-chip">{keys}</span>
            </dt>
            <dd>{t(label)}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}
