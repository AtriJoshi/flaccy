/// Words that mark a bracketed or dashed title suffix as the same recording
/// re-released or credited differently. Live, acoustic, demo, instrumental and
/// remix are left out on purpose: those are different recordings whose lyrics
/// time differently. Mirrors the app's `songEditionKeywords`.
pub const EDITION_KEYWORDS: [&str; 17] = [
    "remaster",
    "remastered",
    "remasters",
    "mono",
    "stereo",
    "explicit",
    "clean",
    "deluxe",
    "edition",
    "bonus",
    "reissue",
    "reissued",
    "expanded",
    "anniversary",
    "feat",
    "featuring",
    "ft",
];

const TITLE_SUFFIX_SEPARATORS: [&str; 3] = [" - ", " \u{2013} ", ": "];

const FEATURING_MARKERS: [&str; 5] = [" feat. ", " feat ", " ft. ", " ft ", " featuring "];

const ARTIST_SEPARATORS: [&str; 9] = [
    ";",
    " / ",
    " feat. ",
    " feat ",
    " ft. ",
    " ft ",
    " featuring ",
    " (feat.",
    " (ft.",
];

/// The title a lyrics server is most likely to know the song by: bracketed
/// and dashed edition decoration removed ("Song (Remastered 2011)", "Song -
/// 2011 Remaster", "Song [Deluxe]"), a trailing featuring clause cut, case and
/// punctuation kept. Falls back to the input when nothing would be left.
pub fn clean_title(title: &str) -> String {
    let without_groups = remove_decorated_groups(title);
    let without_suffix = cut_decorated_suffix(&without_groups);
    let without_featuring = cut_featuring(&without_suffix);
    let collapsed = without_featuring
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        title.trim().to_string()
    } else {
        collapsed
    }
}

/// The first credited artist of a collaboration: the text before a `;`, ` / `
/// or featuring marker. Names that merely contain `&` or `,` stay whole.
pub fn lead_artist(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let cut = ARTIST_SEPARATORS
        .iter()
        .filter_map(|separator| lower.find(separator))
        .min()
        .filter(|_| lower.len() == artist.len());
    let lead = match cut {
        Some(index) => artist.get(..index).unwrap_or(artist),
        None => artist,
    }
    .trim();
    if lead.is_empty() {
        artist.trim().to_string()
    } else {
        lead.to_string()
    }
}

fn contains_keyword(text: &str) -> bool {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .any(|token| EDITION_KEYWORDS.contains(&token))
}

/// Drops every top-level `(…)` or `[…]` group that holds an edition keyword.
/// A group that never closes is kept as written.
fn remove_decorated_groups(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '(' || character == '[' {
            if let Some(close) = matching_close(&characters, index) {
                let inner: String = characters[index + 1..close].iter().collect();
                if contains_keyword(&inner) {
                    index = close + 1;
                    continue;
                }
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

fn matching_close(characters: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, character) in characters[open..].iter().enumerate() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Cuts at the first separator whose suffix holds an edition keyword.
fn cut_decorated_suffix(text: &str) -> String {
    let mut cut = text.len();
    for separator in TITLE_SUFFIX_SEPARATORS {
        if let Some(index) = text.find(separator) {
            if contains_keyword(&text[index + separator.len()..]) {
                cut = cut.min(index);
            }
        }
    }
    text[..cut].to_string()
}

fn cut_featuring(text: &str) -> String {
    let lower = text.to_lowercase();
    let cut = FEATURING_MARKERS
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()
        .filter(|_| lower.len() == text.len());
    match cut.and_then(|index| text.get(..index)) {
        Some(head) => head.to_string(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edition_decoration_is_removed_from_a_title() {
        assert_eq!(clean_title("Song (Remastered 2011)"), "Song");
        assert_eq!(clean_title("Song - 2011 Remaster"), "Song");
        assert_eq!(clean_title("Song [Deluxe Edition]"), "Song");
        assert_eq!(clean_title("Don't Stop Me Now - Remastered"), "Don't Stop Me Now");
        assert_eq!(clean_title("Song (Remastered) (Live)"), "Song (Live)");
        assert_eq!(clean_title("Song (Mono)  "), "Song");
    }

    #[test]
    fn a_featuring_clause_is_cut() {
        assert_eq!(clean_title("Song (feat. Someone)"), "Song");
        assert_eq!(clean_title("Song feat. Someone"), "Song");
        assert_eq!(clean_title("Song (ft. A & B) [Explicit]"), "Song");
    }

    #[test]
    fn different_recordings_and_plain_titles_are_untouched() {
        assert_eq!(clean_title("Song (Live)"), "Song (Live)");
        assert_eq!(clean_title("Song (Acoustic Version)"), "Song (Acoustic Version)");
        assert_eq!(clean_title("Song (Radio Edit)"), "Song (Radio Edit)");
        assert_eq!(clean_title("Stereo Love"), "Stereo Love");
        assert_eq!(clean_title("Clean - Part 2"), "Clean - Part 2");
        assert_eq!(clean_title("Song (Remix"), "Song (Remix");
    }

    #[test]
    fn a_title_that_would_vanish_is_kept() {
        assert_eq!(clean_title("(Remastered)"), "(Remastered)");
        assert_eq!(clean_title(""), "");
    }

    #[test]
    fn the_lead_artist_is_the_first_credit() {
        assert_eq!(lead_artist("Artist A feat. B"), "Artist A");
        assert_eq!(lead_artist("A; B"), "A");
        assert_eq!(lead_artist("A / B"), "A");
        assert_eq!(lead_artist("A (ft. B)"), "A");
        assert_eq!(lead_artist("Simon & Garfunkel"), "Simon & Garfunkel");
        assert_eq!(lead_artist("AC/DC"), "AC/DC");
        assert_eq!(lead_artist("Tyler, The Creator"), "Tyler, The Creator");
        assert_eq!(lead_artist("; B"), "; B");
    }
}
