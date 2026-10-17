use super::document::{Document, RawLine, RawWord};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_LINES: usize = 5_000;
const MAX_WORDS: usize = 100_000;

/// Reads a Lyricsfile 1.0 (the open YAML format lrclib serves): synchronized
/// lines, each optionally carrying timed words. The reader understands exactly
/// the YAML subset the format allows and refuses anything else — a duplicate
/// key, a tag, an alias, a tab, an unknown version — by returning `None`, so a
/// caller falls back to the plain LRC text instead of showing a half-read file.
pub fn parse_lyricsfile(text: &str) -> Option<Document> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let root = Reader::new(text).read_document().ok()?;
    document_from(&root)
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Map(Vec<(String, Node)>),
    Seq(Vec<Node>),
    Str(String),
    Int(i64),
    Bool(bool),
    Null,
}

type Parsed<T> = Result<T, ()>;
type Scan = for<'a> fn(&'a str) -> Parsed<Option<(String, &'a str)>>;

#[derive(Clone, Copy, PartialEq)]
enum Chomp {
    Clip,
    Strip,
    Keep,
}

struct Reader {
    lines: Vec<String>,
    pos: usize,
}

impl Reader {
    fn new(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        Reader {
            lines: text.lines().map(str::to_string).collect(),
            pos: 0,
        }
    }

    fn read_document(&mut self) -> Parsed<Node> {
        if let Some((_, content)) = self.peek()? {
            if content == "---" {
                self.pos += 1;
            }
        }
        let (indent, _) = self.peek()?.ok_or(())?;
        let node = self.block(indent, 0)?;
        if self.peek()?.is_some() {
            return Err(());
        }
        Ok(node)
    }

    /// The next line that carries content, as its indent and trimmed text,
    /// without consuming it. Blank and comment lines are skipped; a tab in the
    /// indentation is an error.
    fn peek(&mut self) -> Parsed<Option<(usize, String)>> {
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            let content = line.trim_start_matches(' ');
            if content.trim().is_empty() || content.starts_with('#') {
                self.pos += 1;
                continue;
            }
            if content.starts_with('\t') {
                return Err(());
            }
            let indent = line.len() - content.len();
            return Ok(Some((indent, content.trim_end().to_string())));
        }
        Ok(None)
    }

    fn block(&mut self, indent: usize, depth: usize) -> Parsed<Node> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let (_, content) = self.peek()?.ok_or(())?;
        if is_dash(&content) {
            self.sequence(indent, depth)
        } else {
            self.mapping(indent, depth)
        }
    }

    fn sequence(&mut self, indent: usize, depth: usize) -> Parsed<Node> {
        let mut items: Vec<Node> = Vec::new();
        while let Some((found, content)) = self.peek()? {
            if found > indent {
                return Err(());
            }
            if found != indent || !is_dash(&content) {
                break;
            }
            let after_dash = &content[1..];
            let rest = after_dash.trim_start_matches(' ');
            if rest.is_empty() {
                self.pos += 1;
                match self.peek()? {
                    Some((next, _)) if next > indent => items.push(self.block(next, depth + 1)?),
                    _ => items.push(Node::Null),
                }
                continue;
            }
            if rest.starts_with('\t') || is_dash(rest) {
                return Err(());
            }
            let offset = indent + 1 + (after_dash.len() - rest.len());
            let rest = rest.to_string();
            self.lines[self.pos] = format!("{}{}", " ".repeat(offset), rest);
            if split_entry(&rest).is_some() {
                items.push(self.mapping(offset, depth + 1)?);
            } else {
                self.pos += 1;
                items.push(self.scalar(&rest)?);
            }
        }
        Ok(Node::Seq(items))
    }

    fn mapping(&mut self, indent: usize, depth: usize) -> Parsed<Node> {
        let mut entries: Vec<(String, Node)> = Vec::new();
        while let Some((found, content)) = self.peek()? {
            if found < indent || is_dash(&content) {
                break;
            }
            if found > indent {
                return Err(());
            }
            let (key, rest) = split_entry(&content).ok_or(())?;
            if entries.iter().any(|(existing, _)| *existing == key) {
                return Err(());
            }
            self.pos += 1;
            let value = self.value(rest, indent, depth)?;
            entries.push((key, value));
        }
        if entries.is_empty() {
            return Err(());
        }
        Ok(Node::Map(entries))
    }

    /// The value after `key:`. Nothing after the colon means a nested block on
    /// the following lines, which for a sequence may sit at the key's own
    /// indent.
    fn value(&mut self, rest: &str, indent: usize, depth: usize) -> Parsed<Node> {
        if rest.is_empty() || rest.starts_with('#') {
            return match self.peek()? {
                Some((next, _)) if next > indent => self.block(next, depth + 1),
                Some((next, content)) if next == indent && is_dash(&content) => {
                    self.sequence(indent, depth + 1)
                }
                _ => Ok(Node::Null),
            };
        }
        if rest.starts_with('|') || rest.starts_with('>') {
            return self.block_scalar(rest, indent);
        }
        self.scalar(rest)
    }

    fn scalar(&mut self, text: &str) -> Parsed<Node> {
        let text = text.trim();
        match text.chars().next() {
            Some('\'') => self.quoted(text, scan_single),
            Some('"') => self.quoted(text, scan_double),
            Some('[') if text == "[]" => Ok(Node::Seq(Vec::new())),
            Some('{') if text == "{}" => Ok(Node::Map(Vec::new())),
            Some('[' | '{' | '&' | '*' | '!' | '%' | '@' | '`') => Err(()),
            _ => Ok(plain_scalar(text)),
        }
    }

    /// A quoted scalar, which a long line may fold across several; the pieces
    /// are joined with a space as YAML specifies.
    fn quoted(&mut self, first: &str, scan: Scan) -> Parsed<Node> {
        let mut buffer = first.to_string();
        loop {
            if let Some((value, rest)) = scan(&buffer[1..])? {
                let rest = rest.trim();
                if !rest.is_empty() && !rest.starts_with('#') {
                    return Err(());
                }
                return Ok(Node::Str(value));
            }
            let next = self.lines.get(self.pos).ok_or(())?.trim().to_string();
            self.pos += 1;
            buffer.push(' ');
            buffer.push_str(&next);
        }
    }

    fn block_scalar(&mut self, header: &str, parent_indent: usize) -> Parsed<Node> {
        let folded = header.starts_with('>');
        let mut chomp = Chomp::Clip;
        for character in header[1..].chars() {
            match character {
                '-' => chomp = Chomp::Strip,
                '+' => chomp = Chomp::Keep,
                ' ' | '#' => break,
                _ => return Err(()),
            }
        }
        let mut collected: Vec<String> = Vec::new();
        let mut content_indent: Option<usize> = None;
        while let Some(line) = self.lines.get(self.pos) {
            let content = line.trim_start_matches(' ');
            if content.trim().is_empty() {
                collected.push(String::new());
                self.pos += 1;
                continue;
            }
            let indent = line.len() - content.len();
            if indent <= parent_indent {
                break;
            }
            let base = *content_indent.get_or_insert(indent);
            if indent < base {
                break;
            }
            collected.push(line[base..].trim_end().to_string());
            self.pos += 1;
        }
        let trailing = collected
            .iter()
            .rev()
            .take_while(|line| line.is_empty())
            .count();
        collected.truncate(collected.len() - trailing);
        let mut text = if folded {
            fold(&collected)
        } else {
            collected.join("\n")
        };
        if !collected.is_empty() {
            match chomp {
                Chomp::Strip => {}
                Chomp::Clip => text.push('\n'),
                Chomp::Keep => {
                    text.push('\n');
                    text.push_str(&"\n".repeat(trailing));
                }
            }
        }
        Ok(Node::Str(text))
    }
}

fn is_dash(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

/// Splits `key: value` at the first colon that is followed by a space or ends
/// the line. Text that starts like a quoted, flow or special scalar is a value,
/// never a key.
fn split_entry(content: &str) -> Option<(String, &str)> {
    if content.starts_with(['\'', '"', '[', '{', '|', '>', '&', '*', '!', '#']) {
        return None;
    }
    let colon = match (content.find(": "), content.ends_with(':')) {
        (Some(inside), _) => inside,
        (None, true) => content.len() - 1,
        (None, false) => return None,
    };
    let key = content[..colon].trim_end();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), content[colon + 1..].trim_start()))
}

fn plain_scalar(text: &str) -> Node {
    let text = match text.find(" #") {
        Some(cut) => &text[..cut],
        None => text,
    }
    .trim();
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Node::Null,
        "true" | "True" | "TRUE" => return Node::Bool(true),
        "false" | "False" | "FALSE" => return Node::Bool(false),
        _ => {}
    }
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        if let Ok(value) = text.parse::<i64>() {
            return Node::Int(value);
        }
    }
    Node::Str(text.to_string())
}

fn fold(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        if line.is_empty() {
            text.push('\n');
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push(' ');
        }
        text.push_str(line);
    }
    text
}

/// A single-quoted scalar's value and what follows its closing quote, or `None`
/// when the quote is still open at the end of the text.
fn scan_single(text: &str) -> Parsed<Option<(String, &str)>> {
    let mut value = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        if character != '\'' {
            value.push(character);
            continue;
        }
        if matches!(chars.peek(), Some((_, '\''))) {
            value.push('\'');
            chars.next();
            continue;
        }
        return Ok(Some((value, &text[index + 1..])));
    }
    Ok(None)
}

/// A double-quoted scalar's value and what follows its closing quote, or `None`
/// when the quote is still open. Only the escapes YAML defines are accepted.
fn scan_double(text: &str) -> Parsed<Option<(String, &str)>> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((index, character)) = chars.next() {
        match character {
            '"' => return Ok(Some((value, &text[index + 1..]))),
            '\\' => {
                let Some((_, escape)) = chars.next() else {
                    return Ok(None);
                };
                match escape {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    '0' => value.push('\0'),
                    'a' => value.push('\u{7}'),
                    'b' => value.push('\u{8}'),
                    'e' => value.push('\u{1b}'),
                    'f' => value.push('\u{c}'),
                    'v' => value.push('\u{b}'),
                    ' ' | '"' | '/' | '\\' => value.push(escape),
                    'N' => value.push('\u{85}'),
                    '_' => value.push('\u{a0}'),
                    'L' => value.push('\u{2028}'),
                    'P' => value.push('\u{2029}'),
                    'x' => value.push(hex_escape(&mut chars, 2)?),
                    'u' => value.push(hex_escape(&mut chars, 4)?),
                    'U' => value.push(hex_escape(&mut chars, 8)?),
                    _ => return Err(()),
                }
            }
            other => value.push(other),
        }
    }
    Ok(None)
}

fn hex_escape(chars: &mut std::str::CharIndices, digits: usize) -> Parsed<char> {
    let mut code: u32 = 0;
    for _ in 0..digits {
        let (_, digit) = chars.next().ok_or(())?;
        code = code
            .checked_mul(16)
            .and_then(|shifted| shifted.checked_add(digit.to_digit(16)?))
            .ok_or(())?;
    }
    char::from_u32(code).ok_or(())
}

fn get<'a>(map: &'a [(String, Node)], key: &str) -> Option<&'a Node> {
    map.iter().find(|(name, _)| name == key).map(|(_, node)| node)
}

fn as_map(node: &Node) -> Option<&[(String, Node)]> {
    match node {
        Node::Map(entries) => Some(entries),
        _ => None,
    }
}

fn document_from(root: &Node) -> Option<Document> {
    let map = as_map(root)?;
    match get(map, "version")? {
        Node::Str(version) if version == "1.0" => {}
        _ => return None,
    }
    let metadata = as_map(get(map, "metadata")?)?;
    let instrumental = match get(metadata, "instrumental") {
        Some(Node::Bool(flag)) => *flag,
        None | Some(Node::Null) => false,
        Some(_) => return None,
    };
    let plain = match get(map, "plain") {
        Some(Node::Str(text)) => Some(text.clone()),
        None | Some(Node::Null) => None,
        Some(_) => return None,
    };
    let mut lines: Vec<RawLine> = Vec::new();
    let mut words_seen = 0usize;
    match get(map, "lines") {
        Some(Node::Seq(items)) => {
            if items.len() > MAX_LINES {
                return None;
            }
            for item in items {
                let line = line_from(item)?;
                words_seen += line.words.len();
                if words_seen > MAX_WORDS {
                    return None;
                }
                lines.push(line);
            }
        }
        None | Some(Node::Null) => {}
        Some(_) => return None,
    }
    if instrumental {
        return Some(Document::resolve(Vec::new(), None, true));
    }
    Some(Document::resolve(lines, plain, false))
}

fn line_from(node: &Node) -> Option<RawLine> {
    let map = as_map(node)?;
    let mut words: Vec<RawWord> = Vec::new();
    match get(map, "words") {
        Some(Node::Seq(items)) => {
            for item in items {
                words.push(word_from(item)?);
            }
        }
        None | Some(Node::Null) => {}
        Some(_) => return None,
    }
    Some(RawLine {
        start: millis(get(map, "start_ms")?)?,
        end: optional_millis(get(map, "end_ms"))?,
        text: text_of(get(map, "text")?)?,
        words,
    })
}

fn word_from(node: &Node) -> Option<RawWord> {
    let map = as_map(node)?;
    Some(RawWord {
        start: millis(get(map, "start_ms")?)?,
        end: optional_millis(get(map, "end_ms"))?,
        text: text_of(get(map, "text")?)?,
    })
}

fn text_of(node: &Node) -> Option<String> {
    match node {
        Node::Str(text) => Some(text.clone()),
        Node::Int(number) => Some(number.to_string()),
        Node::Bool(flag) => Some(flag.to_string()),
        Node::Null => Some(String::new()),
        _ => None,
    }
}

fn millis(node: &Node) -> Option<f64> {
    match node {
        Node::Int(value) if *value >= 0 => Some(*value as f64 / 1000.0),
        _ => None,
    }
}

fn optional_millis(node: Option<&Node>) -> Option<Option<f64>> {
    match node {
        None | Some(Node::Null) => Some(None),
        Some(other) => millis(other).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::document::LineKind;

    const WORD_SYNCED: &str = "version: '1.0'\n\nmetadata:\n  title: 'Small Hours'\n  artist: 'Example Artist'\n  language: 'en'\n\nlines:\n  - text: 'Stay until the morning'\n    start_ms: 4200\n    end_ms: 6800\n    words:\n      - text: 'Stay '\n        start_ms: 4200\n        end_ms: 4800\n      - text: 'until '\n        start_ms: 4800\n        end_ms: 5400\n      - text: 'the '\n        start_ms: 5400\n        end_ms: 5750\n      - text: 'morning'\n        start_ms: 5750\n        end_ms: 6800\n\nplain: |\n  Stay until the morning\n";

    const OVERLAPPING: &str = "version: '1.0'\n\nmetadata:\n  title: 'Two Voices'\n  artist: 'Example Duo'\n\nlines:\n  - text: 'I will follow the river'\n    start_ms: 10000\n    end_ms: 15000\n  - text: 'Follow the river home'\n    start_ms: 12500\n    end_ms: 16500\n\nplain: |\n  I will follow the river\n  Follow the river home\n";

    const INSTRUMENTAL: &str = "version: '1.0'\n\nmetadata:\n  title: 'Quiet Transit'\n  artist: 'Example Ensemble'\n  duration_ms: 240000\n  instrumental: true\n";

    const MINIMAL: &str = "version: '1.0'\n\nmetadata:\n  title: 'Morning Light'\n  artist: 'Example Artist'\n\nplain: |\n  Morning light through the window\n  A new day begins\n";

    const LRCLIB: &str = "version: '1.0'\nmetadata:\n  title: Blinding Lights\n  artist: The Weeknd\n  album: After Hours\n  duration_ms: 200000\n  instrumental: false\nlines:\n- text: Yeah\n  start_ms: 13130\n  end_ms: 16560\n- text: ♪\n  start_ms: 16560\n  end_ms: 27160\n- text: I've been tryna call\n  start_ms: 27160\n  end_ms: 29960\n- text:\n  start_ms: 29960\n  end_ms: 30000\n- text: 'Don''t stop'\n  start_ms: 30000\n  end_ms: 32000\n  words:\n  - text: 'Don''t '\n    start_ms: 30000\n    end_ms: 31000\n  - text: \"stop\\u0021\"\n    start_ms: 31000\n    end_ms: 32000\n";

    fn text_lines(doc: &Document) -> Vec<&crate::lyrics::document::Line> {
        doc.lines
            .iter()
            .filter(|line| line.kind == LineKind::Text)
            .collect()
    }

    #[test]
    fn reads_the_word_synced_example() {
        let doc = parse_lyricsfile(WORD_SYNCED).unwrap();
        let lines = text_lines(&doc);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Stay until the morning");
        assert_eq!((lines[0].start, lines[0].end), (4.2, 6.8));
        let words: Vec<(f64, f64, &str)> = lines[0]
            .words
            .iter()
            .map(|w| (w.start, w.end, w.text.as_str()))
            .collect();
        assert_eq!(
            words,
            vec![
                (4.2, 4.8, "Stay "),
                (4.8, 5.4, "until "),
                (5.4, 5.75, "the "),
                (5.75, 6.8, "morning"),
            ]
        );
        assert_eq!(doc.plain.as_deref(), Some("Stay until the morning\n"));
        assert!(doc.has_word_sync());
    }

    #[test]
    fn keeps_overlapping_lines_overlapping() {
        let doc = parse_lyricsfile(OVERLAPPING).unwrap();
        let lines = text_lines(&doc);
        assert_eq!((lines[0].start, lines[0].end), (10.0, 15.0));
        assert_eq!((lines[1].start, lines[1].end), (12.5, 16.5));
    }

    #[test]
    fn an_instrumental_file_has_no_lines() {
        let doc = parse_lyricsfile(INSTRUMENTAL).unwrap();
        assert!(doc.instrumental);
        assert!(doc.lines.is_empty());
    }

    #[test]
    fn a_plain_only_file_has_text_and_no_lines() {
        let doc = parse_lyricsfile(MINIMAL).unwrap();
        assert!(!doc.has_synced());
        assert_eq!(
            doc.plain.as_deref(),
            Some("Morning light through the window\nA new day begins\n")
        );
    }

    #[test]
    fn reads_what_lrclib_actually_serves() {
        let doc = parse_lyricsfile(LRCLIB).unwrap();
        let lines = text_lines(&doc);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Yeah", "I've been tryna call", "Don't stop!"]
        );
        assert_eq!(lines[2].words.len(), 2);
        assert_eq!(lines[2].words[1].text, "stop!");
        assert_eq!(lines[0].end, 16.56);
    }

    #[test]
    fn a_long_quoted_line_folded_across_lines_is_joined() {
        let text = "version: '1.0'\nmetadata:\n  title: t\n  artist: a\nlines:\n- text: 'one two\n    three'\n  start_ms: 1000\n";
        let doc = parse_lyricsfile(text).unwrap();
        assert_eq!(text_lines(&doc)[0].text, "one two three");
    }

    #[test]
    fn refuses_what_the_format_forbids() {
        let header = "version: '1.0'\nmetadata:\n  title: t\n  artist: a\n";
        assert!(parse_lyricsfile("version: '2.0'\nmetadata:\n  title: t\n  artist: a\n").is_none());
        assert!(parse_lyricsfile(&format!("{header}version: '1.0'\n")).is_none());
        assert!(parse_lyricsfile(&format!("{header}lines:\n\t- text: x\n")).is_none());
        assert!(parse_lyricsfile(&format!("{header}plain: &a x\n")).is_none());
        assert!(parse_lyricsfile(&format!("{header}lines:\n- text: 'open\n  start_ms: 1\n")).is_none());
        assert!(parse_lyricsfile(&format!("{header}lines:\n- text: x\n  start_ms: -5\n")).is_none());
        assert!(parse_lyricsfile(&format!("{header}lines:\n- text: x\n")).is_none());
        assert!(parse_lyricsfile("not yaml at all").is_none());
        assert!(parse_lyricsfile("").is_none());
    }

    #[test]
    fn scalars_are_typed_the_way_yaml_types_them() {
        assert_eq!(plain_scalar("42"), Node::Int(42));
        assert_eq!(plain_scalar("-7"), Node::Int(-7));
        assert_eq!(plain_scalar("true"), Node::Bool(true));
        assert_eq!(plain_scalar("~"), Node::Null);
        assert_eq!(plain_scalar("1.0"), Node::Str("1.0".into()));
        assert_eq!(plain_scalar("a # note"), Node::Str("a".into()));
        assert_eq!(plain_scalar("a#b"), Node::Str("a#b".into()));
    }
}
