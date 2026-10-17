use super::document::{Document, RawLine, RawWord};

/// Parses an LRC or enhanced-LRC file. Word tags inside a line (`<mm:ss.xx>`)
/// become timed words; a file with none is plain line-synced lyrics.
pub fn parse_lrc(text: &str) -> Document {
    Document::resolve(parse_raw_lines(text), None, false)
}

/// The lines of an LRC file as stated, one per timestamp: a line carrying
/// several stamps appears once for each.
pub fn parse_raw_lines(text: &str) -> Vec<RawLine> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<RawLine> = Vec::new();
    for raw_line in text.lines() {
        let (stamps, content) = split_line_stamps(raw_line);
        let segments = split_word_tags(content);
        let words = words_from(&segments, stamps.first().copied());
        let stamps = if stamps.is_empty() {
            words.first().map(|word| vec![word.start]).unwrap_or_default()
        } else {
            stamps
        };
        let plain = if words.is_empty() {
            content.trim().to_string()
        } else {
            String::new()
        };
        for (index, stamp) in stamps.iter().enumerate() {
            let shift = if index == 0 || words.is_empty() {
                0.0
            } else {
                stamp - stamps[0]
            };
            lines.push(RawLine {
                start: *stamp,
                end: None,
                text: plain.clone(),
                words: words
                    .iter()
                    .map(|word| RawWord {
                        start: word.start + shift,
                        end: word.end.map(|end| end + shift),
                        text: word.text.clone(),
                    })
                    .collect(),
            });
        }
    }
    lines
}

/// The leading `[mm:ss.xx]` stamps of a line and what follows them. A leading
/// tag that is not a timestamp (`[ar:Artist]`) ends the stamps and, with none
/// found, makes the whole line metadata.
fn split_line_stamps(line: &str) -> (Vec<f64>, &str) {
    let mut rest = line.trim_start();
    let mut stamps: Vec<f64> = Vec::new();
    while rest.starts_with('[') {
        let Some(close) = rest.find(']') else { break };
        match parse_timestamp(&rest[1..close]) {
            Some(stamp) => stamps.push(stamp),
            None => break,
        }
        rest = &rest[close + 1..];
    }
    if stamps.is_empty() && rest.starts_with('[') {
        return (stamps, "");
    }
    (stamps, rest)
}

/// Cuts a line at every valid `<mm:ss.xx>` tag. The first segment carries no
/// time; each later one starts at the tag before it. Angle brackets that do not
/// hold a timestamp stay in the text.
fn split_word_tags(content: &str) -> Vec<(Option<f64>, String)> {
    let mut segments: Vec<(Option<f64>, String)> = Vec::new();
    let mut pending: Option<f64> = None;
    let mut buffer = String::new();
    let mut rest = content;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let tag = after
            .find('>')
            .and_then(|close| parse_timestamp(&after[..close]).map(|stamp| (close, stamp)));
        match tag {
            Some((close, stamp)) => {
                buffer.push_str(&rest[..open]);
                segments.push((pending, std::mem::take(&mut buffer)));
                pending = Some(stamp);
                rest = &after[close + 1..];
            }
            None => {
                buffer.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    buffer.push_str(rest);
    segments.push((pending, buffer));
    if segments.len() == 1 {
        return Vec::new();
    }
    segments
}

/// Turns segments into words. Text before the first tag belongs to a word that
/// starts with the line; a tag followed by nothing closes the word before it.
fn words_from(segments: &[(Option<f64>, String)], line_start: Option<f64>) -> Vec<RawWord> {
    let mut words: Vec<RawWord> = Vec::new();
    for (time, text) in segments {
        match time {
            None => {
                if !text.is_empty() {
                    if let Some(start) = line_start {
                        words.push(RawWord {
                            start,
                            end: None,
                            text: text.clone(),
                        });
                    }
                }
            }
            Some(stamp) if text.is_empty() => {
                if let Some(last) = words.last_mut() {
                    if last.end.is_none() {
                        last.end = Some(*stamp);
                    }
                }
            }
            Some(stamp) => words.push(RawWord {
                start: *stamp,
                end: None,
                text: text.clone(),
            }),
        }
    }
    words
}

/// Accepts `[mm:ss]`, `[mm:ss.xx]`, `[mm:ss:xx]` and `[hh:mm:ss.xx]` — all four
/// appear in files in the wild, and rejecting one silently drops the whole set.
pub fn parse_timestamp(tag: &str) -> Option<f64> {
    let tag = tag.trim();
    let parts: Vec<&str> = tag.split(':').collect();
    let seconds = |text: &str| -> Option<f64> { text.replace(',', ".").parse::<f64>().ok() };
    let stamp = match parts.len() {
        2 => parts[0].parse::<f64>().ok()? * 60.0 + seconds(parts[1])?,
        3 => {
            let first: f64 = parts[0].parse().ok()?;
            let second: f64 = parts[1].parse().ok()?;
            if parts[2].len() <= 2 && second < 60.0 && first < 60.0 && parts[0].len() <= 2 {
                let fraction: f64 = parts[2].parse().ok()?;
                first * 60.0 + second + fraction / 100.0
            } else {
                first * 3600.0 + second * 60.0 + seconds(parts[2])?
            }
        }
        _ => return None,
    };
    (stamp.is_finite() && stamp >= 0.0).then_some(stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::document::LineKind;

    fn texts(doc: &Document) -> Vec<(f64, String)> {
        doc.lines
            .iter()
            .filter(|line| line.kind == LineKind::Text)
            .map(|line| (line.start, line.text.clone()))
            .collect()
    }

    #[test]
    fn parses_every_timestamp_shape() {
        assert_eq!(parse_timestamp("01:30"), Some(90.0));
        assert_eq!(parse_timestamp("01:30.50"), Some(90.5));
        assert_eq!(parse_timestamp("01:30:50"), Some(90.5));
        assert_eq!(parse_timestamp("01:02:03.5"), Some(3723.5));
        assert_eq!(parse_timestamp("01:30,5"), Some(90.5));
        assert_eq!(parse_timestamp("ti"), None);
        assert_eq!(parse_timestamp("ar:Artist"), None);
    }

    #[test]
    fn repeated_stamps_expand_to_one_line_each() {
        let doc = parse_lrc("[00:10.00][01:10.00]Chorus");
        assert_eq!(
            texts(&doc),
            vec![(10.0, "Chorus".into()), (70.0, "Chorus".into())]
        );
    }

    #[test]
    fn metadata_tags_are_not_lines() {
        let doc = parse_lrc("[ar:Someone]\n[ti:Song]\n[offset:+250]\n[00:05.00]Hello");
        assert_eq!(texts(&doc), vec![(5.0, "Hello".into())]);
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_line() {
        let doc = parse_lrc("\u{feff}[00:05.00]Hello");
        assert_eq!(texts(&doc), vec![(5.0, "Hello".into())]);
    }

    #[test]
    fn enhanced_word_tags_become_timed_words() {
        let doc = parse_lrc("[00:12.00]<00:12.00>Hello <00:12.50>world<00:13.00>\n[00:20.00]x");
        let line = doc.lines.iter().find(|l| l.kind == LineKind::Text).unwrap();
        assert_eq!(line.text, "Hello world");
        let words: Vec<(f64, f64, &str)> = line
            .words
            .iter()
            .map(|w| (w.start, w.end, w.text.as_str()))
            .collect();
        assert_eq!(words, vec![(12.0, 12.5, "Hello "), (12.5, 13.0, "world")]);
    }

    #[test]
    fn text_before_the_first_word_tag_starts_with_the_line() {
        let doc = parse_lrc("[00:12.00]Oh <00:12.50>yes");
        let line = doc.lines.iter().find(|l| l.kind == LineKind::Text).unwrap();
        assert_eq!(line.text, "Oh yes");
        assert_eq!(line.words[0].start, 12.0);
        assert_eq!(line.words[0].end, 12.5);
    }

    #[test]
    fn a_line_with_only_word_tags_takes_its_start_from_the_first() {
        let doc = parse_lrc("<00:08.00>Hi <00:08.40>there");
        let line = doc.lines.iter().find(|l| l.kind == LineKind::Text).unwrap();
        assert_eq!(line.start, 8.0);
        assert_eq!(line.text, "Hi there");
    }

    #[test]
    fn angle_brackets_that_are_not_timestamps_stay_in_the_text() {
        let doc = parse_lrc("[00:05.00]I <3 you <i>so</i>");
        assert_eq!(texts(&doc), vec![(5.0, "I <3 you <i>so</i>".into())]);
    }

    #[test]
    fn a_second_stamp_shifts_the_words_with_it() {
        let doc = parse_lrc("[00:10.00][00:30.00]<00:10.00>a <00:11.00>b");
        let lines: Vec<_> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Text)
            .collect();
        assert_eq!(lines[1].start, 30.0);
        assert_eq!(lines[1].words[1].start, 31.0);
    }

    #[test]
    fn lines_are_ordered_by_time() {
        let doc = parse_lrc("[00:20.00]b\n[00:05.00]a");
        let texts = texts(&doc);
        assert_eq!(texts[0].1, "a");
        assert_eq!(texts[1].1, "b");
    }
}
