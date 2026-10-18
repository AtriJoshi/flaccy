/// Why lrclib could not be asked, or did not answer usefully. Each case is a
/// different thing for a listener to do, so each gets its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LyricsFailure {
    Unreachable,
    TimedOut,
    Busy,
    Server(u16),
    Rejected(u16),
}

impl LyricsFailure {
    /// What an HTTP status other than success and 404 means for the lookup.
    pub fn from_status(code: u16) -> LyricsFailure {
        match code {
            429 => LyricsFailure::Busy,
            500..=599 => LyricsFailure::Server(code),
            _ => LyricsFailure::Rejected(code),
        }
    }
}

/// The headline and the explanation a lyrics view shows when there is no
/// lyric to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsStatus {
    pub title: String,
    pub body: String,
}

fn status(title: impl Into<String>, body: impl Into<String>) -> LyricsStatus {
    LyricsStatus {
        title: title.into(),
        body: body.into(),
    }
}

/// Shown before anything has been looked up.
pub fn idle() -> LyricsStatus {
    status(
        "Nothing Playing",
        "Play a song and its lyrics show up here.",
    )
}

/// Shown while the lookup runs, naming what it is checking.
pub fn searching() -> LyricsStatus {
    status(
        "Looking Up Lyrics…",
        "Checking the song's tags, files next to it, and lrclib.net.",
    )
}

/// The lookup found no words anywhere it looked. It names every place that was
/// checked, so a listener knows what adding their own would change.
pub fn missing(song: &str) -> LyricsStatus {
    status(
        format!("No Lyrics for “{song}”"),
        "Flaccy checked the file's own tags, looked for a .lrc or .elrc file next to the song, and asked lrclib.net. None of them have words for it. lrclib is written by its community, so Flaccy asks again in 30 days. A .lrc file saved next to the song shows up straight away.",
    )
}

/// The track is marked as having no vocals.
pub fn instrumental() -> LyricsStatus {
    status(
        "Instrumental",
        "lrclib.net lists this track as an instrumental, so there are no words to follow.",
    )
}

/// The lookup could not be completed, and why.
pub fn failed(failure: LyricsFailure) -> LyricsStatus {
    match failure {
        LyricsFailure::Unreachable => status(
            "Can't Reach lrclib.net",
            "Flaccy couldn't open a connection. Check that you're online; lyrics load as soon as you are.",
        ),
        LyricsFailure::TimedOut => status(
            "lrclib.net Took Too Long",
            "The connection works, but the lyrics server didn't answer within 10 seconds. Try again in a moment.",
        ),
        LyricsFailure::Busy => status(
            "lrclib.net Is Busy",
            "The lyrics server asked Flaccy to slow down and still hadn't room after a retry. Give it a minute and try again.",
        ),
        LyricsFailure::Server(code) => status(
            "lrclib.net Is Having Trouble",
            format!("The lyrics server answered with an error (HTTP {code}). These usually clear up within a few minutes."),
        ),
        LyricsFailure::Rejected(code) => status(
            "lrclib.net Turned the Request Down",
            format!("The lyrics server answered HTTP {code} instead of lyrics. Trying again is harmless, but this one may need an update to Flaccy."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_the_failure_a_listener_can_act_on() {
        assert_eq!(LyricsFailure::from_status(429), LyricsFailure::Busy);
        assert_eq!(LyricsFailure::from_status(500), LyricsFailure::Server(500));
        assert_eq!(LyricsFailure::from_status(503), LyricsFailure::Server(503));
        assert_eq!(LyricsFailure::from_status(599), LyricsFailure::Server(599));
        assert_eq!(LyricsFailure::from_status(400), LyricsFailure::Rejected(400));
        assert_eq!(LyricsFailure::from_status(403), LyricsFailure::Rejected(403));
    }

    #[test]
    fn a_miss_names_the_song_and_every_place_looked() {
        let note = missing("Hyperballad");
        assert_eq!(note.title, "No Lyrics for “Hyperballad”");
        assert!(note.body.contains("tags"));
        assert!(note.body.contains(".lrc"));
        assert!(note.body.contains("lrclib.net"));
        assert!(note.body.contains("30 days"));
    }

    #[test]
    fn every_failure_has_its_own_headline() {
        let failures = [
            LyricsFailure::Unreachable,
            LyricsFailure::TimedOut,
            LyricsFailure::Busy,
            LyricsFailure::Server(503),
            LyricsFailure::Rejected(403),
        ];
        let mut titles: Vec<String> = failures.iter().map(|f| failed(*f).title).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), failures.len());
    }

    #[test]
    fn the_status_code_reaches_the_words() {
        assert!(failed(LyricsFailure::Server(503)).body.contains("HTTP 503"));
        assert!(failed(LyricsFailure::Rejected(403)).body.contains("HTTP 403"));
    }

    #[test]
    fn searching_names_what_it_checks() {
        let note = searching();
        assert_eq!(note.title, "Looking Up Lyrics…");
        assert!(note.body.contains("lrclib.net"));
    }

    #[test]
    fn idle_and_instrumental_are_distinct() {
        assert_ne!(idle(), instrumental());
        assert_eq!(idle().title, "Nothing Playing");
    }
}
