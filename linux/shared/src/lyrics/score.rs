/// Two pressings of the same song can differ by a few seconds; beyond this the
/// timings would be useless, so a candidate is rejected outright.
pub const MAX_DURATION_DRIFT: f64 = 15.0;
/// Below this a candidate is a different song, not a near miss.
pub const ACCEPTED_SCORE: f64 = 0.80;

/// What the player knows about the song it wants lyrics for.
pub struct Wanted<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
}

/// One record a lyrics server offered.
pub struct Candidate<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
    pub has_synced: bool,
    pub has_word_sync: bool,
}

/// Weighted similarity across the fields a server returns. Duration carries
/// real weight because it is what distinguishes pressings whose timings differ,
/// and a large drift disqualifies a candidate outright. Word timing is a
/// tie-breaker: it never rescues a different song.
pub fn score(wanted: &Wanted, candidate: &Candidate) -> Option<f64> {
    let known = wanted.duration > 0.0 && candidate.duration > 0.0;
    let drift = (wanted.duration - candidate.duration).abs();
    if known && drift > MAX_DURATION_DRIFT {
        return None;
    }
    let duration_score = if known {
        1.0 - (drift / MAX_DURATION_DRIFT).min(1.0)
    } else {
        0.5
    };
    Some(
        0.35 * similarity(wanted.title, candidate.title)
            + 0.30 * similarity(wanted.artist, candidate.artist)
            + 0.25 * duration_score
            + 0.06 * flag(candidate.has_synced)
            + 0.04 * similarity(wanted.album, candidate.album)
            + 0.02 * flag(candidate.has_word_sync),
    )
}

fn flag(value: bool) -> f64 {
    if value {
        1.0
    } else {
        0.0
    }
}

/// Case- and punctuation-insensitive token overlap (Dice coefficient over
/// words), enough to tell "Song (Remastered 2011)" from a different song.
pub fn similarity(left: &str, right: &str) -> f64 {
    let tokens = |text: &str| -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .map(|part| part.to_string())
            .collect()
    };
    let (left, right) = (tokens(left), tokens(right));
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left.iter().filter(|token| right.contains(token)).count();
    2.0 * shared as f64 / (left.len() + right.len()) as f64
}

/// The index of the best candidate that clears `ACCEPTED_SCORE`, the first
/// winning a tie.
pub fn best_candidate(wanted: &Wanted, candidates: &[Candidate]) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(value) = score(wanted, candidate) else {
            continue;
        };
        if best.is_none_or(|(_, top)| value > top) {
            best = Some((index, value));
        }
    }
    best.filter(|(_, value)| *value >= ACCEPTED_SCORE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(duration: f64) -> Wanted<'static> {
        Wanted {
            title: "Song",
            artist: "Artist",
            album: "Album",
            duration,
        }
    }

    fn candidate(title: &'static str, duration: f64, synced: bool, words: bool) -> Candidate<'static> {
        Candidate {
            title,
            artist: "Artist",
            album: "Album",
            duration,
            has_synced: synced,
            has_word_sync: words,
        }
    }

    #[test]
    fn duration_drift_disqualifies_a_candidate() {
        assert!(score(&wanted(200.0), &candidate("Song", 400.0, true, false)).is_none());
        assert!(score(&wanted(200.0), &candidate("Song", 215.0, true, false)).is_some());
        assert!(score(&wanted(200.0), &candidate("Song", 215.1, true, false)).is_none());
    }

    #[test]
    fn an_exact_match_clears_the_threshold() {
        let value = score(&wanted(200.0), &candidate("Song", 200.0, true, false)).unwrap();
        assert!((value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_different_song_falls_short() {
        let other = Candidate {
            title: "Something Else Entirely",
            artist: "Other Band",
            album: "Nope",
            duration: 199.0,
            has_synced: false,
            has_word_sync: false,
        };
        assert!(score(&wanted(200.0), &other).unwrap() < ACCEPTED_SCORE);
    }

    #[test]
    fn an_unknown_duration_scores_neutrally() {
        let value = score(&wanted(0.0), &candidate("Song", 180.0, true, false)).unwrap();
        assert!((value - (0.35 + 0.30 + 0.125 + 0.06 + 0.04)).abs() < 1e-9);
    }

    #[test]
    fn word_timing_breaks_a_tie_without_changing_the_winner_class() {
        let plain = candidate("Song", 200.0, true, false);
        let timed = candidate("Song", 200.0, true, true);
        let picked = best_candidate(&wanted(200.0), &[plain, timed]).unwrap();
        assert_eq!(picked.0, 1);
    }

    #[test]
    fn nothing_below_the_threshold_is_picked() {
        let other = candidate("Another Tune", 200.0, true, true);
        assert!(best_candidate(&wanted(200.0), &[other]).is_none());
    }

    #[test]
    fn similarity_ignores_case_and_punctuation() {
        assert_eq!(similarity("Hello, World!", "hello world"), 1.0);
        assert_eq!(similarity("a b", "c d"), 0.0);
        assert_eq!(similarity("", "x"), 0.0);
        assert!((similarity("Song (Remastered 2011)", "Song") - 0.5).abs() < 1e-9);
    }
}
