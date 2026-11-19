//! A long live transcript for the assistant (HANDOFF 000b): the newest part
//! word for word, older stretches as running notes.
//!
//! During a live share the assistant gets what has been said so far beside
//! each question. Up to [`VERBATIM_BYTES`] (about an hour of speech) goes
//! whole. A longer transcript is cut, from its start, into stretches of
//! about [`BLOCK_BYTES`]; a stretch closes only while at least that much
//! newer text follows it, so the cuts never move and the newest 24–48 KiB
//! always stays word for word. A closed stretch is given as short notes the
//! assistant wrote about it, or, until those exist, as its own lines while
//! [`TOTAL_BYTES`] allows (newest first). Notes are written after the
//! question has been answered, one stretch at a time on the assistant's own
//! queue, so a question asked meanwhile comes first. They are kept in
//! memory only (never on disk), keyed by the stretch's text.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Below this the whole transcript goes word for word (~12,000 tokens).
pub const VERBATIM_BYTES: usize = 48 * 1024;
/// One stretch condensed into notes (~6,000 tokens).
pub const BLOCK_BYTES: usize = 24 * 1024;
/// Everything given beside one question: notes, unsummarised stretches and
/// the newest part (~24,000 tokens; read on the GPU in about 15 s).
pub const TOTAL_BYTES: usize = 96 * 1024;
/// Notes kept per stretch at most.
const MAX_NOTES_BYTES: usize = 3 * 1024;
/// Stretches remembered at most (about 50 hours of speech).
const MAX_KEPT: usize = 128;
/// How long one stretch's notes may take before the rest are given up.
const NOTES_BOUND: Duration = Duration::from_secs(300);

const NOTES_TASK: &str = "You write running notes of one stretch of a live transcript (lines \
`[time] Speaker: what was said`), so that questions about it can be answered later. Write in \
English, translating anything said in another language. Keep, in order: topics, claims, names, \
numbers, dates, decisions, questions asked and answers given. Use only what is said. At most 15 \
short bullet points. Return only the bullets.";

/// Notes written so far, by the stretch's text hash.
static NOTES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// Stretches being written now.
static WRITING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Where the stretches end: `ends[i]` is one past the last line of stretch
/// `i`; the newest part starts at the last end (or 0).
#[must_use]
pub fn stretches(lines: &[String]) -> Vec<usize> {
    let size = |line: &String| line.len() + 1;
    let total: usize = lines.iter().map(size).sum();
    let mut ends = Vec::new();
    if total <= VERBATIM_BYTES {
        return ends;
    }
    let mut consumed = 0;
    let mut block = 0;
    for (index, line) in lines.iter().enumerate() {
        block += size(line);
        if block >= BLOCK_BYTES {
            if total - consumed - block < BLOCK_BYTES {
                break;
            }
            consumed += block;
            block = 0;
            ends.push(index + 1);
        }
    }
    ends
}

/// A stretch's key: its length and a hash of its text (in memory only, so
/// a fast hash is enough; the length makes a mix-up even less likely).
fn key(lines: &[String]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    lines.hash(&mut hash);
    let bytes: usize = lines.iter().map(String::len).sum();
    format!("{bytes}-{:016x}", hash.finish())
}

fn notes_for(key: &str) -> Option<String> {
    NOTES.lock().ok()?.as_ref()?.get(key).cloned()
}

/// The `[m:ss]` time a transcript line starts with.
fn time_of(line: &str) -> &str {
    line.strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map_or("", |(time, _)| time)
}

/// What the assistant is given of `lines` (newest last).
#[must_use]
pub fn context(lines: &[String]) -> String {
    context_with(lines, notes_for)
}

fn context_with(lines: &[String], notes: impl Fn(&str) -> Option<String>) -> String {
    let ends = stretches(lines);
    let start = ends.last().copied().unwrap_or(0);
    let newest = lines[start..].join("\n");
    if ends.is_empty() {
        return newest;
    }
    let mut parts: Vec<Option<String>> = Vec::with_capacity(ends.len());
    let mut room = TOTAL_BYTES.saturating_sub(newest.len());
    // Notes first: they are short and cover their stretch.
    let mut begin = 0;
    let mut pending = Vec::new();
    for (index, &end) in ends.iter().enumerate() {
        let stretch = &lines[begin..end];
        let span = format!(
            "{}–{}",
            time_of(&stretch[0]),
            time_of(&stretch[stretch.len() - 1])
        );
        match notes(&key(stretch)) {
            Some(text) => {
                let text = format!("(Notes on [{span}])\n{}", text.trim());
                room = room.saturating_sub(text.len() + 1);
                parts.push(Some(text));
            }
            None => {
                parts.push(None);
                pending.push((index, begin, end, span));
            }
        }
        begin = end;
    }
    // Stretches without notes yet: their own lines, newest first, while
    // they fit; older ones are named as left out.
    for (index, begin, end, span) in pending.into_iter().rev() {
        let text = lines[begin..end].join("\n");
        parts[index] = Some(if text.len() < room {
            room -= text.len() + 1;
            text
        } else {
            format!("(What was said in [{span}] is left out here: too long to include.)")
        });
    }
    let mut out: Vec<String> = parts.into_iter().flatten().collect();
    out.push(newest);
    out.join("\n")
}

/// Write the notes still missing for the closed stretches of `lines`, one
/// at a time on the assistant's queue, in the background.
pub fn write_missing(assistant: Arc<super::assistant::Assistant>, lines: Vec<String>) {
    let ends = stretches(&lines);
    let mut wanted = Vec::new();
    let mut begin = 0;
    for end in ends {
        let stretch = lines[begin..end].to_vec();
        let id = key(&stretch);
        begin = end;
        if notes_for(&id).is_some() {
            continue;
        }
        let claimed = WRITING
            .lock()
            .ok()
            .is_some_and(|mut writing| writing.get_or_insert_with(HashSet::new).insert(id.clone()));
        if claimed {
            wanted.push((id, stretch));
        }
    }
    if wanted.is_empty() {
        return;
    }
    let ids: Vec<String> = wanted.iter().map(|(id, _)| id.clone()).collect();
    let spawned = std::thread::Builder::new()
        .name("narrivy-live-notes".into())
        .spawn(move || {
            for (id, stretch) in wanted {
                let written = write_one(&assistant, &stretch);
                if let Ok(mut writing) = WRITING.lock()
                    && let Some(writing) = writing.as_mut()
                {
                    writing.remove(&id);
                }
                let Some(text) = written else {
                    // The model is busy failing or gone: try again with
                    // the next question.
                    break;
                };
                if let Ok(mut notes) = NOTES.lock() {
                    let notes = notes.get_or_insert_with(HashMap::new);
                    if notes.len() >= MAX_KEPT {
                        notes.clear();
                    }
                    notes.insert(id, text);
                }
            }
            if let Ok(mut writing) = WRITING.lock()
                && let Some(writing) = writing.as_mut()
            {
                writing.clear();
            }
        });
    if spawned.is_err()
        && let Ok(mut writing) = WRITING.lock()
        && let Some(writing) = writing.as_mut()
    {
        for id in ids {
            writing.remove(&id);
        }
    }
}

fn write_one(assistant: &super::assistant::Assistant, stretch: &[String]) -> Option<String> {
    let messages = serde_json::json!([
        {"role": "system", "content": NOTES_TASK},
        {"role": "user", "content": stretch.join("\n")},
    ]);
    let (outcome, text) = assistant
        .ask(None, &messages)
        .ok()?
        .recv_timeout(NOTES_BOUND)
        .ok()?;
    let text = text.trim();
    if outcome != "ok" || text.is_empty() {
        return None;
    }
    let mut cut = text.len().min(MAX_NOTES_BYTES);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    Some(text[..cut].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(minutes: usize) -> Vec<String> {
        // About 820 bytes a minute: a line every 10 s, as speech comes.
        (0..minutes * 6)
            .map(|i| {
                let s = i * 10;
                format!(
                    "[{}:{:02}] Person 1: line {i} {}",
                    s / 60,
                    s % 60,
                    "word ".repeat(25)
                )
            })
            .collect()
    }

    fn bytes(lines: &[String]) -> usize {
        lines.iter().map(|l| l.len() + 1).sum()
    }

    #[test]
    fn an_hour_goes_word_for_word() {
        let lines = transcript(50);
        assert!(bytes(&lines) <= VERBATIM_BYTES);
        assert!(stretches(&lines).is_empty());
        assert_eq!(context(&lines), lines.join("\n"));
    }

    #[test]
    fn stretches_never_move_as_the_transcript_grows() {
        let long = transcript(240);
        let ends = stretches(&long);
        assert!(ends.len() >= 7);
        // The newest part stays between one and two stretches long.
        let newest = bytes(&long[*ends.last().unwrap_or(&0)..]);
        assert!(
            (BLOCK_BYTES..=VERBATIM_BYTES + 200).contains(&newest),
            "{newest}"
        );
        for minutes in [60, 90, 120, 180] {
            let shorter = stretches(&transcript(minutes));
            assert_eq!(shorter, ends[..shorter.len()], "{minutes}");
        }
    }

    #[test]
    fn two_hours_fit_with_notes_or_without() {
        let lines = transcript(120);
        // No notes yet: the newest stretches go word for word, the oldest
        // are named as left out, all within the total.
        let plain = context_with(&lines, |_| None);
        assert!(plain.len() <= TOTAL_BYTES + 200, "{}", plain.len());
        assert!(plain.ends_with(&lines[lines.len() - 1]));
        assert!(plain.contains("is left out here"));
        // With notes: every stretch is covered and nothing is left out.
        let noted = context_with(&lines, |_| Some("- they talked".to_owned()));
        assert!(noted.starts_with("(Notes on [0:00–"));
        assert!(!noted.contains("left out"));
        assert!(noted.len() < plain.len());
        assert!(noted.ends_with(&lines[lines.len() - 1]));
    }
}
