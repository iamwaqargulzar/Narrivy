// The user's vocabulary, in the pipeline's Transcribe stage: words the
// speech model should recognise (names, products, terms), and replacements
// (say this, write that). The service passes the words to the model and
// corrects near misses of them; replacements apply last. Every pipeline
// shares one vocabulary.
import { useEffect, useState, type FormEvent } from "react";
import { bridge, type Vocabulary } from "../../lib/bridge";
import { t } from "../../i18n";
import { Button } from "../../ui/primitives/Button";
import { useErrorHandler } from "../../ui/primitives/Toast";
import { LIVE, Option } from "./StandingOptions";

export function VocabularyEditor() {
  const report = useErrorHandler();
  const [vocabulary, setVocabulary] = useState<Vocabulary | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [word, setWord] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    bridge.vocabulary().then(
      (found) => alive && setVocabulary(found),
      () => alive && setUnavailable(true),
    );
    return () => {
      alive = false;
    };
  }, []);

  const save = async (next: Vocabulary) => {
    setSaving(true);
    try {
      setVocabulary(await bridge.setVocabulary(next));
      return true;
    } catch (err) {
      report(err, { retry: () => void save(next) });
      return false;
    } finally {
      setSaving(false);
    }
  };

  const addWords = async (event: FormEvent) => {
    event.preventDefault();
    if (!vocabulary) return;
    // Several at once, separated by commas.
    const added = word
      .split(",")
      .map((w) => w.trim())
      .filter(Boolean);
    if (!added.length) return;
    if (await save({ ...vocabulary, words: [...vocabulary.words, ...added] }))
      setWord("");
  };

  const addReplacement = async (event: FormEvent) => {
    event.preventDefault();
    if (!vocabulary || !from.trim() || !to.trim()) return;
    const replacements = [
      ...vocabulary.replacements,
      { from: from.trim(), to: to.trim() },
    ];
    if (await save({ ...vocabulary, replacements })) {
      setFrom("");
      setTo("");
    }
  };

  if (unavailable) {
    return (
      <Option
        label={t("vocab.title")}
        hint={t("vocab.unavailable")}
        standing={LIVE}
      />
    );
  }

  return (
    <div className="vocab">
      <Option label={t("vocab.title")} hint={t("vocab.hint")} standing={LIVE} />
      <ul className="vocab-words" aria-label={t("vocab.title")}>
        {vocabulary?.words.map((w) => (
          <li key={w} className="vocab-word">
            <span>{w}</span>
            <button
              type="button"
              aria-label={t("vocab.remove", { word: w })}
              disabled={saving}
              onClick={() =>
                vocabulary &&
                void save({
                  ...vocabulary,
                  words: vocabulary.words.filter((x) => x !== w),
                })
              }
            >
              ×
            </button>
          </li>
        ))}
        {vocabulary && vocabulary.words.length === 0 ? (
          <li className="source">{t("vocab.none")}</li>
        ) : null}
      </ul>
      <form className="vocab-add" onSubmit={(e) => void addWords(e)}>
        <input
          value={word}
          onChange={(e) => setWord(e.target.value)}
          placeholder={t("vocab.wordPlaceholder")}
          aria-label={t("vocab.wordLabel")}
          maxLength={400}
          disabled={!vocabulary}
        />
        <Button type="submit" disabled={!vocabulary || saving || !word.trim()}>
          {t("vocab.add")}
        </Button>
      </form>

      <Option
        label={t("vocab.replacements")}
        hint={t("vocab.replacementsHint")}
        standing={LIVE}
      />
      <ul className="vocab-replacements" aria-label={t("vocab.replacements")}>
        {vocabulary?.replacements.map((r) => (
          <li key={r.from} className="vocab-replacement">
            <span>{r.from}</span>
            <span aria-hidden="true">→</span>
            <span>{r.to}</span>
            <button
              type="button"
              aria-label={t("vocab.remove", { word: r.from })}
              disabled={saving}
              onClick={() =>
                vocabulary &&
                void save({
                  ...vocabulary,
                  replacements: vocabulary.replacements.filter(
                    (x) => x.from !== r.from,
                  ),
                })
              }
            >
              ×
            </button>
          </li>
        ))}
      </ul>
      <form className="vocab-add" onSubmit={(e) => void addReplacement(e)}>
        <input
          value={from}
          onChange={(e) => setFrom(e.target.value)}
          placeholder={t("vocab.fromPlaceholder")}
          aria-label={t("vocab.fromLabel")}
          maxLength={64}
          disabled={!vocabulary}
        />
        <input
          value={to}
          onChange={(e) => setTo(e.target.value)}
          placeholder={t("vocab.toPlaceholder")}
          aria-label={t("vocab.toLabel")}
          maxLength={64}
          disabled={!vocabulary}
        />
        <Button
          type="submit"
          disabled={!vocabulary || saving || !from.trim() || !to.trim()}
        >
          {t("vocab.add")}
        </Button>
      </form>
    </div>
  );
}
