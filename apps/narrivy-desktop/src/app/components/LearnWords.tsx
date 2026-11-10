// After the user corrects a dictation: offer the corrected words for their
// vocabulary ("Your words" in the Transcribe stage), one click each.
import { useState } from "react";
import { bridge } from "../../lib/bridge";
import { t } from "../../i18n";
import { useErrorHandler } from "../../ui/primitives/Toast";

export function LearnWords({
  words,
  onDone,
}: {
  words: string[];
  onDone: () => void;
}) {
  const report = useErrorHandler();
  const [left, setLeft] = useState(words);
  const [added, setAdded] = useState<string[]>([]);

  const add = async (word: string) => {
    try {
      const current = await bridge.vocabulary();
      await bridge.setVocabulary({
        ...current,
        words: [...current.words, word],
      });
      setAdded((list) => [...list, word]);
      const rest = left.filter((w) => w !== word);
      setLeft(rest);
      if (!rest.length) window.setTimeout(onDone, 1500);
    } catch (err) {
      report(err, { retry: () => void add(word) });
    }
  };

  return (
    <div className="learn-words" role="group" aria-label={t("learn.title")}>
      <span className="source">
        {left.length
          ? t("learn.title")
          : t("learn.added", { words: added.join(", ") })}
      </span>
      {left.map((word) => (
        <button
          key={word}
          type="button"
          className="learn-word"
          onClick={() => void add(word)}
        >
          + {word}
        </button>
      ))}
      {left.length ? (
        <button type="button" className="text-button" onClick={onDone}>
          {t("learn.notNow")}
        </button>
      ) : null}
    </div>
  );
}
