use super::document::Document;

/// Why a song's lyrics are not filled in word by word, or that nothing needs
/// saying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingNoteKind {
    LineByLine,
    Unsynced,
}

/// The short explanation a lyrics view offers next to the timing control: a
/// two-word `label` for the chip, and the `title` and `body` of its popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimingNote {
    pub kind: TimingNoteKind,
    pub label: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

const LINE_BY_LINE: TimingNote = TimingNote {
    kind: TimingNoteKind::LineByLine,
    label: "Line by line",
    title: "Timed line by line",
    body: "This song's lyrics are timed one line at a time, so Flaccy lights the line being sung but can't fill it word by word. Word timing is still rare: it comes from lrclib when a contributor adds it, or from an enhanced .elrc file saved next to the song.",
};

const UNSYNCED: TimingNote = TimingNote {
    kind: TimingNoteKind::Unsynced,
    label: "Not timed",
    title: "These lyrics aren't timed",
    body: "This song has lyrics but no timing, so they can't follow the music. Timing comes from lrclib when a contributor adds it, or from a .lrc or .elrc file saved next to the song.",
};

/// The note to show for a document: none when the words are timed, the track
/// is instrumental or there is nothing to read, otherwise the reason the fill
/// is missing.
pub fn timing_note(document: &Document) -> Option<TimingNote> {
    if document.instrumental || document.has_word_sync() {
        None
    } else if document.has_synced() {
        Some(LINE_BY_LINE)
    } else if document.plain.is_some() {
        Some(UNSYNCED)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::{parse_lrc, Line, LineKind};

    fn plain(text: &str) -> Option<String> {
        Some(text.to_string())
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

    #[test]
    fn a_word_timed_document_has_no_note() {
        let document =
            parse_lrc("[00:01.00]<00:01.00>Hello <00:01.50>world\n[00:03.00]<00:03.00>Again");
        assert!(document.has_word_sync());
        assert_eq!(timing_note(&document), None);
    }

    #[test]
    fn one_timed_line_among_plain_lines_has_no_note() {
        let document = parse_lrc(
            "[00:01.00]Hello world\n[00:03.00]<00:03.00>Again <00:03.40>now\n[00:05.00]Last line",
        );
        assert!(document.has_word_sync());
        assert_eq!(timing_note(&document), None);
    }

    #[test]
    fn a_line_timed_document_is_line_by_line() {
        let document = parse_lrc("[00:01.00]Hello world\n[00:03.00]Again");
        let note = timing_note(&document).expect("a note");
        assert_eq!(note.kind, TimingNoteKind::LineByLine);
    }

    #[test]
    fn interludes_alone_do_not_count_as_synced_lines() {
        let document = Document {
            lines: vec![interlude(0.0, 8.0), interlude(20.0, 30.0)],
            plain: plain("Hello world"),
            instrumental: false,
        };
        let note = timing_note(&document).expect("a note");
        assert_eq!(note.kind, TimingNoteKind::Unsynced);
    }

    #[test]
    fn a_document_with_plain_text_only_is_unsynced() {
        let document = Document::resolve(Vec::new(), plain("Hello world"), false);
        let note = timing_note(&document).expect("a note");
        assert_eq!(note.kind, TimingNoteKind::Unsynced);
    }

    #[test]
    fn lines_and_plain_text_together_are_line_by_line() {
        let raw = crate::lyrics::parse_raw_lines("[00:01.00]Hello world");
        let document = Document::resolve(raw, plain("Hello world"), false);
        let note = timing_note(&document).expect("a note");
        assert_eq!(note.kind, TimingNoteKind::LineByLine);
    }

    #[test]
    fn an_instrumental_track_has_no_note() {
        let document = Document::resolve(Vec::new(), None, true);
        assert_eq!(timing_note(&document), None);
        let with_text = Document::resolve(Vec::new(), plain("Hello"), true);
        assert_eq!(timing_note(&with_text), None);
    }

    #[test]
    fn an_empty_document_has_no_note() {
        assert_eq!(timing_note(&Document::default()), None);
        assert_eq!(
            timing_note(&Document::resolve(Vec::new(), plain("  \n"), false)),
            None
        );
    }

    #[test]
    fn the_copy_is_pinned() {
        let line = timing_note(&parse_lrc("[00:01.00]Hello")).expect("a note");
        assert_eq!(line.label, "Line by line");
        assert_eq!(line.title, "Timed line by line");
        assert_eq!(
            line.body,
            "This song's lyrics are timed one line at a time, so Flaccy lights the line being sung but can't fill it word by word. Word timing is still rare: it comes from lrclib when a contributor adds it, or from an enhanced .elrc file saved next to the song."
        );
        let unsynced =
            timing_note(&Document::resolve(Vec::new(), plain("Hello"), false)).expect("a note");
        assert_eq!(unsynced.label, "Not timed");
        assert_eq!(unsynced.title, "These lyrics aren't timed");
        assert_eq!(
            unsynced.body,
            "This song has lyrics but no timing, so they can't follow the music. Timing comes from lrclib when a contributor adds it, or from a .lrc or .elrc file saved next to the song."
        );
    }
}
