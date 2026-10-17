use super::document::{Document, Line};

/// How many lines before the latest-started one are checked for a line that is
/// still sounding, which is how overlapping voices stay lit together.
pub const OVERLAP_LOOKBACK: usize = 8;

/// An interlude is shown as this many beats that light one after another.
pub const INTERLUDE_BEATS: usize = 3;

/// Where a clock stands inside a line that has timed words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineProgress {
    NotStarted,
    Singing { word: usize, fraction: f64 },
    Done,
}

impl Line {
    /// The word being sung at `time` and how far through it, or `None` for a
    /// line without timed words. Between two words the earlier one counts as
    /// finished, so a fill never runs backwards.
    pub fn progress(&self, time: f64) -> Option<LineProgress> {
        let first = self.words.first()?;
        let last = self.words.last()?;
        if time < first.start {
            return Some(LineProgress::NotStarted);
        }
        if time >= last.end {
            return Some(LineProgress::Done);
        }
        let index = self.words.partition_point(|word| word.start <= time) - 1;
        let word = &self.words[index];
        let span = word.end - word.start;
        let fraction = if span <= 0.0 {
            1.0
        } else {
            ((time - word.start) / span).clamp(0.0, 1.0)
        };
        Some(LineProgress::Singing {
            word: index,
            fraction,
        })
    }

    /// How far through an instrumental break `time` is, 0 to 1.
    pub fn interlude_fraction(&self, time: f64) -> f64 {
        let span = self.end - self.start;
        if !self.is_interlude() || span <= 0.0 {
            return 0.0;
        }
        ((time - self.start) / span).clamp(0.0, 1.0)
    }

    /// How many of the interlude's beats are lit at `time`.
    pub fn lit_beats(&self, time: f64) -> usize {
        let lit = (self.interlude_fraction(time) * INTERLUDE_BEATS as f64).floor() as usize;
        lit.min(INTERLUDE_BEATS)
    }
}

impl Document {
    /// The last line to have started by `time`: where a view keeps its scroll
    /// and its emphasis, even through the stretch after that line has ended.
    pub fn primary_index(&self, time: f64) -> Option<usize> {
        self.lines
            .partition_point(|line| line.start <= time)
            .checked_sub(1)
    }

    /// Every line that should be lit at `time`: the primary one plus any other
    /// still sounding, in order.
    pub fn highlighted(&self, time: f64) -> Vec<usize> {
        let Some(primary) = self.primary_index(time) else {
            return Vec::new();
        };
        let from = primary.saturating_sub(OVERLAP_LOOKBACK);
        (from..=primary)
            .filter(|index| {
                *index == primary || {
                    let line = &self.lines[*index];
                    line.start <= time && time < line.end && !line.is_interlude()
                }
            })
            .collect()
    }

    /// The sung line's text at `time`, for a surface that shows one line of
    /// lyrics and nothing else: blank during a break and once a line has ended.
    pub fn text_at(&self, time: f64) -> Option<&str> {
        let line = &self.lines[self.primary_index(time)?];
        (!line.is_interlude() && time < line.end).then_some(line.text.as_str())
    }

    /// The next instant after `time` at which anything on screen could change:
    /// a line starting or ending, a word starting or ending, or an interlude
    /// lighting its next beat. `None` once the song has nothing left to say.
    pub fn next_boundary(&self, time: f64) -> Option<f64> {
        let mut best = f64::INFINITY;
        let consider = |at: f64, best: &mut f64| {
            if at > time && at < *best {
                *best = at;
            }
        };
        for line in &self.lines {
            if line.start > best {
                break;
            }
            consider(line.start, &mut best);
            consider(line.end, &mut best);
            for word in &line.words {
                consider(word.start, &mut best);
                consider(word.end, &mut best);
            }
            if line.is_interlude() {
                for beat in 1..INTERLUDE_BEATS {
                    let at = line.start + (line.end - line.start) * beat as f64
                        / INTERLUDE_BEATS as f64;
                    consider(at, &mut best);
                }
            }
        }
        best.is_finite().then_some(best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::document::{LineKind, RawLine, RawWord};

    fn raw(start: f64, end: Option<f64>, text: &str) -> RawLine {
        RawLine {
            start,
            end,
            text: text.into(),
            words: Vec::new(),
        }
    }

    fn song() -> Document {
        Document::resolve(
            vec![
                raw(1.0, Some(3.0), "one"),
                raw(3.5, Some(6.0), "two"),
                raw(20.0, Some(24.0), "three"),
            ],
            None,
            false,
        )
    }

    fn singing_line() -> Line {
        let line = RawLine {
            start: 10.0,
            end: Some(14.0),
            text: String::new(),
            words: vec![
                RawWord { start: 10.0, end: Some(11.0), text: "Stay ".into() },
                RawWord { start: 11.0, end: Some(12.0), text: "until ".into() },
                RawWord { start: 13.0, end: Some(14.0), text: "dawn".into() },
            ],
        };
        Document::resolve(vec![line], None, false)
            .lines
            .into_iter()
            .find(|l| l.kind == LineKind::Text)
            .unwrap()
    }

    #[test]
    fn the_primary_line_is_the_last_one_started() {
        let doc = song();
        assert_eq!(doc.primary_index(0.5), None);
        assert_eq!(doc.primary_index(1.0), Some(0));
        assert_eq!(doc.primary_index(3.2), Some(0));
        assert_eq!(doc.primary_index(3.5), Some(1));
        assert_eq!(doc.primary_index(7.0), Some(2));
        assert_eq!(doc.primary_index(500.0), Some(3));
    }

    #[test]
    fn text_at_goes_blank_once_a_line_has_ended() {
        let doc = song();
        assert_eq!(doc.text_at(0.5), None);
        assert_eq!(doc.text_at(1.0), Some("one"));
        assert_eq!(doc.text_at(2.9), Some("one"));
        assert_eq!(doc.text_at(3.2), None);
        assert_eq!(doc.text_at(4.0), Some("two"));
        assert_eq!(doc.text_at(8.0), None);
        assert_eq!(doc.text_at(21.0), Some("three"));
        assert_eq!(doc.text_at(30.0), None);
    }

    #[test]
    fn overlapping_voices_are_lit_together() {
        let doc = Document::resolve(
            vec![
                raw(1.0, Some(5.0), "lead"),
                raw(2.5, Some(6.5), "echo"),
            ],
            None,
            false,
        );
        assert_eq!(doc.highlighted(0.5), Vec::<usize>::new());
        assert_eq!(doc.highlighted(2.0), vec![0]);
        assert_eq!(doc.highlighted(3.0), vec![0, 1]);
        assert_eq!(doc.highlighted(5.5), vec![1]);
    }

    #[test]
    fn progress_follows_the_words() {
        let line = singing_line();
        assert_eq!(line.progress(9.0), Some(LineProgress::NotStarted));
        assert_eq!(
            line.progress(10.5),
            Some(LineProgress::Singing { word: 0, fraction: 0.5 })
        );
        assert_eq!(
            line.progress(12.4),
            Some(LineProgress::Singing { word: 1, fraction: 1.0 })
        );
        assert_eq!(
            line.progress(13.25),
            Some(LineProgress::Singing { word: 2, fraction: 0.25 })
        );
        assert_eq!(line.progress(14.0), Some(LineProgress::Done));
    }

    #[test]
    fn a_line_without_words_has_no_progress() {
        let doc = song();
        assert_eq!(doc.lines[1].progress(2.0), None);
    }

    #[test]
    fn an_interlude_lights_its_beats_in_turn() {
        let doc = song();
        let interlude = doc
            .lines
            .iter()
            .find(|l| l.is_interlude() && l.start == 6.0)
            .unwrap();
        assert_eq!(interlude.end, 20.0);
        assert_eq!(interlude.lit_beats(6.0), 0);
        assert_eq!(interlude.lit_beats(11.0), 1);
        assert_eq!(interlude.lit_beats(16.0), 2);
        assert_eq!(interlude.lit_beats(19.99), 2);
        assert_eq!(interlude.lit_beats(25.0), 3);
        assert!((interlude.interlude_fraction(13.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_next_boundary_is_the_soonest_change() {
        let doc = song();
        assert_eq!(doc.next_boundary(0.0), Some(1.0));
        assert_eq!(doc.next_boundary(1.0), Some(3.0));
        assert_eq!(doc.next_boundary(3.0), Some(3.5));
        let beat = doc.next_boundary(6.0).unwrap();
        assert!((beat - 10.666_666_666_666_666).abs() < 1e-9);
        assert_eq!(doc.next_boundary(23.9), Some(24.0));
        assert_eq!(doc.next_boundary(24.0), None);
    }
}
