// Learning from corrections: when the user edits a dictation, the words
// they corrected (a misheard name fixed by hand) are worth offering for
// their vocabulary. Only replacements that look like a fix of the word that
// was heard count: a new sentence typed in, or one everyday word swapped
// for another, teaches nothing.

const COMMON = new Set(
  `the and for not with you this but his from they say her she will one all would there their what out
  about who get which when make can like time just him know take into year your good some could them see
  other than then now look only come over think also back after use two how our work first well way even
  new want because any these give day most are was were been has had did does said went made got came took
  saw knew thought where why here very much many still too again off down never every own same another
  both each few more less last next while before through under between should might must being have that
  its yes okay hello thanks please sorry today tomorrow`.split(/\s+/),
);

/** Words with their letters and digits only; the original spelling kept. */
function words(text: string): string[] {
  return text
    .split(/\s+/)
    .map((w) => w.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, ""))
    .filter(Boolean);
}

const plain = (w: string) => w.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");

function distance(a: string, b: string): number {
  const row = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    let previous = row[0];
    row[0] = i;
    for (let j = 1; j <= b.length; j++) {
      const substitute = previous + (a[i - 1] === b[j - 1] ? 0 : 1);
      previous = row[j];
      row[j] = Math.min(substitute, row[j - 1] + 1, previous + 1);
    }
  }
  return row[b.length];
}

/** Whether `fixed` reads as a correction of `heard`: about half the
 * letters alike, so a different word entirely does not count. */
function corrects(heard: string, fixed: string): boolean {
  const a = plain(heard);
  const b = plain(fixed);
  if (!a || !b || a === b) return false;
  return distance(a, b) * 2 <= Math.max(a.length, b.length);
}

/** The stretches that changed, by longest common run of words. */
function changes(
  before: string[],
  after: string[],
): Array<{ removed: string[]; added: string[] }> {
  const n = before.length;
  const m = after.length;
  const lcs: number[][] = Array.from({ length: n + 1 }, () =>
    new Array<number>(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] =
        plain(before[i]) === plain(after[j])
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: Array<{ removed: string[]; added: string[] }> = [];
  let current = { removed: [] as string[], added: [] as string[] };
  let i = 0;
  let j = 0;
  const flush = () => {
    if (current.removed.length || current.added.length) out.push(current);
    current = { removed: [], added: [] };
  };
  while (i < n || j < m) {
    if (i < n && j < m && plain(before[i]) === plain(after[j])) {
      flush();
      i++;
      j++;
    } else if (j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j])) {
      current.added.push(after[j++]);
    } else {
      current.removed.push(before[i++]);
    }
  }
  flush();
  return out;
}

/** Words worth offering for the vocabulary after `original` was edited
 * into `edited`, leaving out those already known (`known`). */
export function learnedWords(
  original: string,
  edited: string,
  known: string[] = [],
  limit = 5,
): string[] {
  const have = new Set(known.map(plain));
  const found: string[] = [];
  for (const change of changes(words(original), words(edited))) {
    if (!change.removed.length) continue; // typed in, not corrected
    const heard = [...change.removed, change.removed.join("")];
    for (const word of change.added) {
      const key = plain(word);
      if (
        key.length < 3 ||
        COMMON.has(key) ||
        have.has(key) ||
        found.some((f) => plain(f) === key)
      )
        continue;
      if (heard.some((h) => corrects(h, word))) found.push(word);
    }
  }
  return found.slice(0, limit);
}
