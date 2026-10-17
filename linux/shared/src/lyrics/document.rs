use std::cmp::Ordering;

/// How long a line with no end of its own stays "being sung" when nothing
/// marks where it stops: past this the music has moved on.
pub const MAX_INFERRED_HOLD: f64 = 10.0;
/// The shortest gap between two lines that is worth showing as an instrumental
/// break rather than simply holding the previous line.
pub const MIN_INTERLUDE: f64 = 4.0;

/// One timed word or syllable of a sung line. `text` keeps the space that
/// follows it, so joining every word rebuilds the line.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// A word as a file states it, before missing ends are worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct RawWord {
    pub start: f64,
    pub end: Option<f64>,
    pub text: String,
}

/// A line as a file states it, before ends, breaks and interludes are worked
/// out. An empty `text` with no words is an LRC break marker.
#[derive(Clone, Debug, PartialEq)]
pub struct RawLine {
    pub start: f64,
    pub end: Option<f64>,
    pub text: String,
    pub words: Vec<RawWord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Text,
    Interlude,
}

/// A resolved line: it always knows where it ends, and an instrumental break
/// is a line of its own so a list of rows maps one to one onto `lines`.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
    pub kind: LineKind,
    pub background: bool,
}

impl Line {
    pub fn is_interlude(&self) -> bool {
        self.kind == LineKind::Interlude
    }

    pub fn has_words(&self) -> bool {
        !self.words.is_empty()
    }
}

/// Everything known about one track's lyrics, whichever file format or server
/// it came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub lines: Vec<Line>,
    pub plain: Option<String>,
    pub instrumental: bool,
}

impl Document {
    /// Orders the lines, gives each one an end, folds LRC break markers into
    /// those ends, and turns every long enough gap into an interlude line.
    pub fn resolve(raw: Vec<RawLine>, plain: Option<String>, instrumental: bool) -> Document {
        let mut ordered: Vec<(usize, RawLine)> = raw
            .into_iter()
            .filter(|line| line.start.is_finite() && line.start >= 0.0)
            .enumerate()
            .collect();
        ordered.sort_by(|a, b| {
            a.1.start
                .partial_cmp(&b.1.start)
                .unwrap_or(Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });

        let mut entries: Vec<RawLine> = Vec::new();
        let mut breaks: Vec<f64> = Vec::new();
        for (_, line) in ordered {
            if is_blank(&line) {
                breaks.push(line.start);
            } else {
                entries.push(line);
            }
        }

        let next_starts = next_distinct_starts(&entries);
        let text_lines: Vec<Line> = entries
            .iter()
            .zip(next_starts)
            .map(|(entry, next)| {
                let end = line_end(entry, next, &breaks);
                build_line(entry, end)
            })
            .collect();

        Document {
            lines: with_interludes(text_lines),
            plain: plain.filter(|text| !text.trim().is_empty()),
            instrumental,
        }
    }

    /// True when there is nothing to show at all.
    pub fn is_empty(&self) -> bool {
        !self.instrumental && !self.has_synced() && self.plain.is_none()
    }

    pub fn has_synced(&self) -> bool {
        self.lines.iter().any(|line| !line.is_interlude())
    }

    pub fn has_word_sync(&self) -> bool {
        self.lines.iter().any(Line::has_words)
    }
}

/// The marks lyric files use to say "instrumental here".
const NOTE_MARKS: [char; 4] = ['\u{266A}', '\u{266B}', '\u{266C}', '\u{2669}'];

/// A line with nothing to sing: empty, or only whitespace and music notes,
/// which is how a file marks an instrumental stretch.
fn is_silent(text: &str) -> bool {
    text.chars()
        .all(|character| character.is_whitespace() || NOTE_MARKS.contains(&character))
}

fn is_blank(line: &RawLine) -> bool {
    is_silent(&line.text) && line.words.iter().all(|word| is_silent(&word.text))
}

/// For every entry, the start of the first later entry that starts strictly
/// after it: lines stamped at the same instant are one moment of the song and
/// share an end.
fn next_distinct_starts(entries: &[RawLine]) -> Vec<Option<f64>> {
    let mut next = vec![None; entries.len()];
    for index in (0..entries.len().saturating_sub(1)).rev() {
        next[index] = if entries[index + 1].start > entries[index].start {
            Some(entries[index + 1].start)
        } else {
            next[index + 1]
        };
    }
    next
}

/// An end a file states wins, and failing that the end of the line's last
/// timed word. Otherwise the line runs to the first break marker after it,
/// else to the next line, and never past the longest hold.
fn line_end(entry: &RawLine, next_start: Option<f64>, breaks: &[f64]) -> f64 {
    if let Some(end) = entry.end {
        return end.max(entry.start);
    }
    let last_word_end = entry
        .words
        .iter()
        .filter_map(|word| word.end)
        .fold(f64::NEG_INFINITY, f64::max);
    if last_word_end.is_finite() {
        return last_word_end.max(entry.start);
    }
    let limit = entry.start + MAX_INFERRED_HOLD;
    let marker = breaks
        .iter()
        .copied()
        .find(|at| *at > entry.start && next_start.is_none_or(|next| *at <= next));
    marker
        .or(next_start)
        .unwrap_or(limit)
        .min(limit)
        .max(entry.start)
}

fn build_line(entry: &RawLine, end: f64) -> Line {
    let words = resolve_words(&entry.words, end);
    let text = if words.is_empty() {
        entry.text.trim().to_string()
    } else {
        words.iter().map(|word| word.text.as_str()).collect()
    };
    Line {
        start: entry.start,
        end,
        background: is_background(&text),
        text,
        words,
        kind: LineKind::Text,
    }
}

/// Folds whitespace-only fragments into the word before them, trims the line's
/// outer whitespace, orders by start, and gives each word the end the next one
/// implies.
fn resolve_words(raw: &[RawWord], line_end: f64) -> Vec<Word> {
    let mut merged: Vec<RawWord> = Vec::with_capacity(raw.len());
    for word in raw {
        if word.text.is_empty() {
            continue;
        }
        if word.text.trim().is_empty() {
            if let Some(previous) = merged.last_mut() {
                previous.text.push_str(&word.text);
            }
            continue;
        }
        merged.push(word.clone());
    }
    merged.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(Ordering::Equal));
    if let Some(first) = merged.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = merged.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    let starts: Vec<f64> = merged.iter().map(|word| word.start).collect();
    merged
        .into_iter()
        .enumerate()
        .map(|(index, word)| {
            let implied = starts.get(index + 1).copied().unwrap_or(line_end);
            Word {
                end: word.end.unwrap_or(implied).max(word.start),
                start: word.start,
                text: word.text,
            }
        })
        .collect()
}

/// Backing vocals are conventionally written in one pair of parentheses.
fn is_background(text: &str) -> bool {
    let text = text.trim();
    if !text.starts_with('(') || !text.ends_with(')') {
        return false;
    }
    let mut depth = 0i32;
    for (index, character) in text.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && index + character.len_utf8() < text.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// Inserts an interlude line into the intro and into every gap of at least
/// `MIN_INTERLUDE` seconds between the furthest end so far and the next start.
fn with_interludes(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::with_capacity(lines.len() + 4);
    let mut furthest_end = 0.0f64;
    for line in lines {
        if line.start - furthest_end >= MIN_INTERLUDE {
            out.push(interlude(furthest_end, line.start));
        }
        furthest_end = furthest_end.max(line.end);
        out.push(line);
    }
    out
}

fn interlude(start: f64, end: f64) -> Line {
    Line {
        start,
        end,
        text: String::new(),
        words: Vec::new(),
        kind: LineKind::Interlude,
        background: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(start: f64, end: Option<f64>, text: &str) -> RawLine {
        RawLine {
            start,
            end,
            text: text.to_string(),
            words: Vec::new(),
        }
    }

    #[test]
    fn an_unmarked_line_runs_to_the_next_one() {
        let doc = Document::resolve(
            vec![raw(1.0, None, "a"), raw(4.0, None, "b")],
            None,
            false,
        );
        let ends: Vec<f64> = doc.lines.iter().map(|l| l.end).collect();
        assert_eq!(ends, vec![4.0, 14.0]);
        assert_eq!(doc.lines[0].kind, LineKind::Text);
    }

    #[test]
    fn a_blank_stamp_ends_the_line_before_it() {
        let doc = Document::resolve(
            vec![raw(1.0, None, "a"), raw(3.0, None, ""), raw(20.0, None, "b")],
            None,
            false,
        );
        assert_eq!(doc.lines[0].end, 3.0);
    }

    #[test]
    fn a_line_of_music_notes_is_a_break_not_a_lyric() {
        let doc = Document::resolve(
            vec![
                raw(1.0, Some(3.0), "a"),
                raw(3.0, Some(14.0), "\u{266A}"),
                raw(14.0, Some(16.0), "b"),
            ],
            None,
            false,
        );
        let kinds: Vec<LineKind> = doc.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![LineKind::Text, LineKind::Interlude, LineKind::Text]
        );
        assert_eq!((doc.lines[1].start, doc.lines[1].end), (3.0, 14.0));
    }

    #[test]
    fn a_long_gap_becomes_an_interlude_and_the_intro_too() {
        let doc = Document::resolve(
            vec![raw(6.0, Some(8.0), "a"), raw(20.0, Some(22.0), "b")],
            None,
            false,
        );
        let kinds: Vec<LineKind> = doc.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Interlude,
                LineKind::Text,
                LineKind::Interlude,
                LineKind::Text
            ]
        );
        assert_eq!((doc.lines[0].start, doc.lines[0].end), (0.0, 6.0));
        assert_eq!((doc.lines[2].start, doc.lines[2].end), (8.0, 20.0));
    }

    #[test]
    fn a_short_gap_is_not_an_interlude() {
        let doc = Document::resolve(
            vec![raw(1.0, Some(3.0), "a"), raw(5.0, Some(7.0), "b")],
            None,
            false,
        );
        assert_eq!(doc.lines.len(), 2);
    }

    #[test]
    fn overlapping_lines_leave_no_interlude_between_them() {
        let doc = Document::resolve(
            vec![raw(10.0, Some(15.0), "a"), raw(12.5, Some(16.5), "b")],
            None,
            false,
        );
        let kinds: Vec<LineKind> = doc.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![LineKind::Interlude, LineKind::Text, LineKind::Text]
        );
    }

    #[test]
    fn lines_stamped_together_share_one_end() {
        let doc = Document::resolve(
            vec![raw(1.0, None, "a"), raw(1.0, None, "b"), raw(4.0, None, "c")],
            None,
            false,
        );
        assert_eq!(doc.lines[0].end, 4.0);
        assert_eq!(doc.lines[1].end, 4.0);
    }

    #[test]
    fn parentheses_around_the_whole_line_mark_backing_vocals() {
        assert!(is_background("(oh oh oh)"));
        assert!(is_background(" (a (b) c) "));
        assert!(!is_background("(a) b (c)"));
        assert!(!is_background("plain"));
    }

    #[test]
    fn words_get_ends_and_the_line_text_follows_them() {
        let line = RawLine {
            start: 1.0,
            end: Some(4.0),
            text: "ignored".into(),
            words: vec![
                RawWord { start: 1.0, end: None, text: " Hello ".into() },
                RawWord { start: 2.0, end: Some(2.5), text: "big ".into() },
                RawWord { start: 3.0, end: None, text: "world ".into() },
            ],
        };
        let doc = Document::resolve(vec![line], None, false);
        let text = doc.lines.iter().find(|l| !l.is_interlude()).unwrap();
        assert_eq!(text.text, "Hello big world");
        let ends: Vec<f64> = text.words.iter().map(|w| w.end).collect();
        assert_eq!(ends, vec![2.0, 2.5, 4.0]);
    }

    #[test]
    fn a_line_ends_with_its_last_timed_word_when_nothing_says_otherwise() {
        let line = RawLine {
            start: 1.0,
            end: None,
            text: String::new(),
            words: vec![
                RawWord { start: 1.0, end: Some(1.5), text: "Hi ".into() },
                RawWord { start: 1.5, end: Some(2.25), text: "there".into() },
            ],
        };
        let next = raw(9.0, None, "later");
        let doc = Document::resolve(vec![line, next], None, false);
        assert_eq!(doc.lines[0].end, 2.25);
        assert_eq!(doc.lines[1].kind, LineKind::Interlude);
        assert_eq!((doc.lines[1].start, doc.lines[1].end), (2.25, 9.0));
    }

    #[test]
    fn a_whitespace_fragment_joins_the_word_before_it() {
        let line = RawLine {
            start: 0.0,
            end: Some(2.0),
            text: String::new(),
            words: vec![
                RawWord { start: 0.0, end: None, text: "a".into() },
                RawWord { start: 0.5, end: None, text: " ".into() },
                RawWord { start: 1.0, end: None, text: "b".into() },
            ],
        };
        let doc = Document::resolve(vec![line], None, false);
        assert_eq!(doc.lines[0].text, "a b");
        assert_eq!(doc.lines[0].words.len(), 2);
    }
}
