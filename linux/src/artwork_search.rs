use crate::hygiene::artist_key;
use crate::wantlist::base_title;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAXIMUM_BODY_BYTES: u64 = 25 * 1024 * 1024;
const MAXIMUM_CANDIDATES: usize = 18;
const CONFIDENT_SCORE: f64 = 0.5;
const CONFIDENT_COUNT: usize = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const MAXIMUM_REDIRECTS: usize = 4;
const MUSICBRAINZ_INTERVAL: Duration = Duration::from_millis(1100);
const USER_AGENT: &str = "flaccy/1.0 (https://github.com/guitaripod/flaccy)";
const VARIOUS_ARTISTS: &str = "Various Artists";

static LAST_MUSICBRAINZ: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub source: &'static str,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: String,
    pub full_url: String,
    pub score: f64,
}

#[derive(Debug, Default)]
pub struct SearchOutcome {
    pub candidates: Vec<Candidate>,
    pub providers_failed: usize,
    pub providers_asked: usize,
}

impl SearchOutcome {
    /// Every catalog failed to answer: the person is offline rather than out
    /// of luck.
    pub fn unreachable(&self) -> bool {
        self.providers_asked > 0 && self.providers_failed == self.providers_asked
    }
}

#[derive(Debug)]
pub enum FetchError {
    Insecure,
    TooLarge,
    Status(u16),
    Transport(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Insecure => write!(f, "only https links are fetched"),
            FetchError::TooLarge => write!(f, "the image is larger than 25 MB"),
            FetchError::Status(code) => write!(f, "the server answered {code}"),
            FetchError::Transport(message) => write!(f, "{message}"),
        }
    }
}

/// The search box's starting text: "<artist> <title>", with no artist when the
/// release is credited to Various Artists, which no catalog files anything under.
pub fn default_query(title: &str, artist: &str) -> String {
    let credit = catalog_credit(artist);
    format!("{credit} {title}").trim().to_string()
}

fn catalog_credit(artist: &str) -> &str {
    if artist.eq_ignore_ascii_case(VARIOUS_ARTISTS) {
        ""
    } else {
        artist
    }
}

/// Title carries most of the weight: an exact edition-folded match is worth far
/// more than a substring, and the artist breaks ties between different records
/// that share a name. Identical to `OnlineArtworkSearch.score`.
pub fn score(candidate_title: &str, candidate_artist: &str, title: &str, artist: &str) -> f64 {
    let wanted = base_title(title);
    let found = base_title(candidate_title);
    let title_score = if wanted == found {
        0.7
    } else if !wanted.is_empty()
        && !found.is_empty()
        && (found.contains(&wanted) || wanted.contains(&found))
    {
        0.4
    } else {
        0.0
    };
    let artist_score = if artist.is_empty() {
        0.15
    } else if artist_key(candidate_artist) == artist_key(artist) {
        0.3
    } else {
        0.0
    };
    title_score + artist_score
}

/// Merges the providers' answers: unique by full URL, best score first; unless
/// the person searched for something of their own, only confident matches when
/// there are enough of them, never more than eighteen.
pub fn rank(merged: Vec<Candidate>, confident_filter: bool) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let mut unique: Vec<Candidate> = merged
        .into_iter()
        .filter(|candidate| seen.insert(candidate.full_url.clone()))
        .collect();
    unique.sort_by(|a, b| b.score.total_cmp(&a.score));
    let confident = unique
        .iter()
        .filter(|candidate| candidate.score >= CONFIDENT_SCORE)
        .count();
    if confident_filter && confident >= CONFIDENT_COUNT {
        unique.retain(|candidate| candidate.score >= CONFIDENT_SCORE);
    }
    unique.truncate(MAXIMUM_CANDIDATES);
    unique
}

#[derive(Deserialize)]
struct AppleResponse {
    results: Vec<AppleItem>,
}

#[derive(Deserialize)]
struct AppleItem {
    #[serde(rename = "collectionId")]
    collection_id: Option<u64>,
    #[serde(rename = "collectionName")]
    collection_name: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
}

/// Apple's art URLs end `…/100x100bb.jpg`; the CDN serves the same file at any
/// size up to the source's own, so a bigger number asks for the full image.
pub fn parse_apple(json: &str, title: &str, artist: &str) -> Vec<Candidate> {
    let Ok(response) = serde_json::from_str::<AppleResponse>(json) else {
        return Vec::new();
    };
    response
        .results
        .into_iter()
        .filter_map(|item| {
            let small = item.artwork_url_100?;
            if !small.starts_with("https://") || !small.contains("100x100bb") {
                return None;
            }
            let name = item.collection_name.unwrap_or_default();
            let credit = item.artist_name.unwrap_or_default();
            Some(Candidate {
                id: format!("apple-{}", item.collection_id.unwrap_or(0)),
                source: "Apple Music",
                score: score(&name, &credit, title, artist),
                title: name,
                artist: credit,
                thumbnail_url: small.replace("100x100bb", "360x360bb"),
                full_url: small.replace("100x100bb", "3000x3000bb"),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct DeezerResponse {
    data: Vec<DeezerItem>,
}

#[derive(Deserialize)]
struct DeezerItem {
    id: u64,
    title: String,
    artist: Option<DeezerArtist>,
    cover_medium: Option<String>,
    cover_big: Option<String>,
    cover_xl: Option<String>,
}

#[derive(Deserialize)]
struct DeezerArtist {
    name: String,
}

pub fn parse_deezer(json: &str, title: &str, artist: &str) -> Vec<Candidate> {
    let Ok(response) = serde_json::from_str::<DeezerResponse>(json) else {
        return Vec::new();
    };
    response
        .data
        .into_iter()
        .filter_map(|item| {
            let thumbnail = item.cover_medium.filter(|url| url.starts_with("https://"))?;
            let full = item
                .cover_xl
                .or(item.cover_big)
                .filter(|url| url.starts_with("https://"))?;
            let credit = item.artist.map(|artist| artist.name).unwrap_or_default();
            Some(Candidate {
                id: format!("deezer-{}", item.id),
                source: "Deezer",
                score: score(&item.title, &credit, title, artist),
                title: item.title,
                artist: credit,
                thumbnail_url: thumbnail,
                full_url: full,
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct MusicBrainzResponse {
    #[serde(rename = "release-groups")]
    release_groups: Vec<ReleaseGroup>,
}

#[derive(Deserialize)]
struct ReleaseGroup {
    id: String,
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
}

#[derive(Deserialize)]
struct ArtistCredit {
    name: String,
    #[serde(default)]
    joinphrase: String,
}

/// Cover Art Archive keys its art by MusicBrainz release group. Many groups
/// have none, which shows up as a thumbnail that answers 404 and is dropped by
/// whoever shows it.
pub fn parse_musicbrainz(json: &str, title: &str, artist: &str) -> Vec<Candidate> {
    let Ok(response) = serde_json::from_str::<MusicBrainzResponse>(json) else {
        return Vec::new();
    };
    response
        .release_groups
        .into_iter()
        .filter(|group| is_identifier(&group.id))
        .map(|group| {
            let credit: String = group
                .artist_credit
                .iter()
                .map(|part| format!("{}{}", part.name, part.joinphrase))
                .collect();
            Candidate {
                id: format!("caa-{}", group.id),
                source: "Cover Art Archive",
                score: score(&group.title, &credit, title, artist),
                title: group.title,
                artist: credit,
                thumbnail_url: format!(
                    "https://coverartarchive.org/release-group/{}/front-250",
                    group.id
                ),
                full_url: format!(
                    "https://coverartarchive.org/release-group/{}/front-1200",
                    group.id
                ),
            }
        })
        .collect()
}

fn is_identifier(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// What each catalog is asked. An untouched search box asks the way the iOS
/// editor does, by title and credit; a query the person typed is theirs and goes
/// through as written.
#[derive(Clone, Debug)]
pub struct Request {
    pub query: String,
    pub title: String,
    pub artist: String,
    pub edited: bool,
}

impl Request {
    pub fn new(query: &str, title: &str, artist: &str) -> Self {
        let query = query.trim().to_string();
        let edited = query != default_query(title, artist);
        Self {
            query,
            title: title.to_string(),
            artist: catalog_credit(artist).to_string(),
            edited,
        }
    }

    fn deezer_query(&self) -> String {
        if self.edited || self.artist.is_empty() {
            self.query.clone()
        } else {
            format!(
                "artist:\"{}\" album:\"{}\"",
                strip_quotes(&self.artist),
                strip_quotes(&self.title)
            )
        }
    }

    fn musicbrainz_query(&self) -> String {
        if self.edited {
            lucene_phrases(&self.query)
        } else if self.artist.is_empty() {
            format!("releasegroup:\"{}\"", strip_quotes(&self.title))
        } else {
            format!(
                "releasegroup:\"{}\" AND artist:\"{}\"",
                strip_quotes(&self.title),
                strip_quotes(&self.artist)
            )
        }
    }
}

fn strip_quotes(text: &str) -> String {
    text.replace(['"', '\\'], " ")
}

/// A free-text query as Lucene terms: quotes and operators lose their meaning.
fn lucene_phrases(query: &str) -> String {
    strip_quotes(query)
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '-')
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Asks the three catalogs at once and ranks what they say. Blocking: call it
/// from a worker thread.
pub fn search(request: &Request) -> SearchOutcome {
    let (apple, deezer, musicbrainz) = std::thread::scope(|scope| {
        let apple = scope.spawn(|| apple_candidates(request));
        let deezer = scope.spawn(|| deezer_candidates(request));
        let musicbrainz = scope.spawn(|| musicbrainz_candidates(request));
        (
            apple.join().unwrap_or_else(|_| Err("panicked".into())),
            deezer.join().unwrap_or_else(|_| Err("panicked".into())),
            musicbrainz.join().unwrap_or_else(|_| Err("panicked".into())),
        )
    });
    let mut merged = Vec::new();
    let mut failed = 0;
    for answer in [apple, deezer, musicbrainz] {
        match answer {
            Ok(mut candidates) => merged.append(&mut candidates),
            Err(message) => {
                failed += 1;
                crate::logger::warn("artwork", &format!("catalog search failed: {message}"));
            }
        }
    }
    SearchOutcome {
        candidates: rank(merged, !request.edited),
        providers_failed: failed,
        providers_asked: 3,
    }
}

fn apple_candidates(request: &Request) -> Result<Vec<Candidate>, String> {
    let url = format!(
        "https://itunes.apple.com/search?term={}&media=music&entity=album&limit=20",
        percent_encode(&request.query)
    );
    let body = fetch_text(&url)?;
    Ok(parse_apple(&body, &request.title, &request.artist))
}

fn deezer_candidates(request: &Request) -> Result<Vec<Candidate>, String> {
    let url = format!(
        "https://api.deezer.com/search/album?q={}&limit=12",
        percent_encode(&request.deezer_query())
    );
    let body = fetch_text(&url)?;
    Ok(parse_deezer(&body, &request.title, &request.artist))
}

fn musicbrainz_candidates(request: &Request) -> Result<Vec<Candidate>, String> {
    let url = format!(
        "https://musicbrainz.org/ws/2/release-group/?query={}&fmt=json&limit=8",
        percent_encode(&request.musicbrainz_query())
    );
    wait_for_musicbrainz();
    let body = fetch_text(&url)?;
    Ok(parse_musicbrainz(&body, &request.title, &request.artist))
}

/// MusicBrainz allows one request a second per client.
fn wait_for_musicbrainz() {
    let wait = {
        let Ok(mut last) = LAST_MUSICBRAINZ.lock() else {
            return;
        };
        let wait = last
            .map(|at| MUSICBRAINZ_INTERVAL.saturating_sub(at.elapsed()))
            .unwrap_or(Duration::ZERO);
        *last = Some(Instant::now() + wait);
        wait
    };
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn fetch_text(url: &str) -> Result<String, String> {
    let bytes = fetch_bytes(url, false, 4 * 1024 * 1024).map_err(|err| err.to_string())?;
    String::from_utf8(bytes).map_err(|err| err.to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirects(0)
        .user_agent(USER_AGENT)
        .build()
}

/// Reads a URL's body, at most `limit` bytes, following up to four redirects
/// by hand so a link that starts on https can never end up on plain http.
/// `allow_http` is for a link the person dropped themselves; catalog art is
/// https only.
pub fn fetch_bytes(url: &str, allow_http: bool, limit: u64) -> Result<Vec<u8>, FetchError> {
    let agent = agent();
    let mut current = url.to_string();
    let started_secure = !current.starts_with("http://");
    for _ in 0..=MAXIMUM_REDIRECTS {
        check_scheme(&current, allow_http && !started_secure)?;
        let response = agent
            .get(&current)
            .call()
            .map_err(|err| match err {
                ureq::Error::Status(code, _) => FetchError::Status(code),
                ureq::Error::Transport(transport) => FetchError::Transport(transport.to_string()),
            })?;
        let status = response.status();
        if (300..400).contains(&status) {
            let Some(location) = response.header("location") else {
                return Err(FetchError::Status(status));
            };
            current = resolve_location(&current, location);
            continue;
        }
        if status != 200 {
            return Err(FetchError::Status(status));
        }
        return read_capped(response, limit);
    }
    Err(FetchError::Transport("too many redirects".into()))
}

fn check_scheme(url: &str, http_allowed: bool) -> Result<(), FetchError> {
    if url.starts_with("https://") || (http_allowed && url.starts_with("http://")) {
        Ok(())
    } else {
        Err(FetchError::Insecure)
    }
}

fn resolve_location(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    let scheme_end = base.find("://").map_or(0, |index| index + 3);
    let host_end = base[scheme_end..]
        .find('/')
        .map_or(base.len(), |index| scheme_end + index);
    if location.starts_with('/') {
        format!("{}{}", &base[..host_end], location)
    } else {
        let directory_end = base.rfind('/').map_or(host_end, |index| index.max(host_end));
        format!("{}/{}", &base[..directory_end], location)
    }
}

fn read_capped(response: ureq::Response, limit: u64) -> Result<Vec<u8>, FetchError> {
    let declared = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > limit) {
        return Err(FetchError::TooLarge);
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|err| FetchError::Transport(err.to_string()))?;
    if body.len() as u64 > limit {
        return Err(FetchError::TooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLE: &str = r#"{"resultCount":3,"results":[
        {"collectionId":1,"collectionName":"Music Has the Right to Children","artistName":"Boards of Canada","artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/aa/100x100bb.jpg"},
        {"collectionId":2,"collectionName":"Music Has the Right to Children (Remastered)","artistName":"Boards of Canada","artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/bb/100x100bb.jpg"},
        {"collectionId":3,"collectionName":"No Art","artistName":"Nobody"}]}"#;

    const DEEZER: &str = r#"{"data":[
        {"id":10,"title":"Music Has The Right To Children","artist":{"name":"Boards Of Canada"},"cover_medium":"https://e-cdns-images.dzcdn.net/images/cover/x/250x250-000000-80-0-0.jpg","cover_big":"https://e-cdns-images.dzcdn.net/images/cover/x/500x500-000000-80-0-0.jpg","cover_xl":"https://e-cdns-images.dzcdn.net/images/cover/x/1000x1000-000000-80-0-0.jpg"},
        {"id":11,"title":"Geogaddi","artist":{"name":"Boards Of Canada"},"cover_medium":"https://e-cdns-images.dzcdn.net/images/cover/y/250x250.jpg","cover_big":"https://e-cdns-images.dzcdn.net/images/cover/y/500x500.jpg"},
        {"id":12,"title":"Insecure","artist":{"name":"X"},"cover_medium":"http://insecure.example/m.jpg","cover_xl":"http://insecure.example/xl.jpg"}]}"#;

    const MUSICBRAINZ: &str = r#"{"created":"2026-10-17T10:00:00Z","count":2,"offset":0,"release-groups":[
        {"id":"11111111-2222-3333-4444-555555555555","title":"Music Has the Right to Children","primary-type":"Album","artist-credit":[{"name":"Boards of Canada","joinphrase":"","artist":{"id":"x","name":"Boards of Canada"}}]},
        {"id":"not a uuid/../evil","title":"Music Has the Right to Children","artist-credit":[{"name":"Boards of Canada"}]}]}"#;

    fn wanted() -> (&'static str, &'static str) {
        ("Music Has the Right to Children", "Boards of Canada")
    }

    #[test]
    fn an_edition_folded_title_and_the_same_artist_score_highest() {
        let (title, artist) = wanted();
        let exact = score("Music Has the Right to Children", "Boards of Canada", title, artist);
        let remaster = score(
            "Music Has the Right to Children (Deluxe Edition)",
            "boards of canada",
            title,
            artist,
        );
        assert!((exact - 1.0).abs() < 1e-9);
        assert!((remaster - 1.0).abs() < 1e-9, "decoration folds away: {remaster}");
    }

    #[test]
    fn a_substring_title_scores_less_and_a_different_artist_scores_nothing() {
        let substring = score("Music Has the Right to Children Live", "Boards of Canada", "Music Has the Right to Children", "Boards of Canada");
        assert!((substring - 0.7).abs() < 1e-9, "0.4 title + 0.3 artist: {substring}");
        let stranger = score("Music Has the Right to Children", "Cover Band", "Music Has the Right to Children", "Boards of Canada");
        assert!((stranger - 0.7).abs() < 1e-9, "title only");
        let unrelated = score("Other", "Cover Band", "Music Has the Right to Children", "Boards of Canada");
        assert!(unrelated.abs() < 1e-9);
    }

    #[test]
    fn an_empty_artist_earns_the_flat_artist_share() {
        assert!((score("Score", "Anyone", "Score", "") - 0.85).abs() < 1e-9);
        assert!((score("Other", "Anyone", "Score", "") - 0.15).abs() < 1e-9);
    }

    #[test]
    fn apple_results_carry_a_small_thumbnail_and_a_three_thousand_pixel_original() {
        let (title, artist) = wanted();
        let found = parse_apple(APPLE, title, artist);
        assert_eq!(found.len(), 2, "an item with no art is dropped");
        assert_eq!(found[0].source, "Apple Music");
        assert_eq!(found[0].id, "apple-1");
        assert_eq!(
            found[0].thumbnail_url,
            "https://is1-ssl.mzstatic.com/image/thumb/Music/aa/360x360bb.jpg"
        );
        assert_eq!(
            found[0].full_url,
            "https://is1-ssl.mzstatic.com/image/thumb/Music/aa/3000x3000bb.jpg"
        );
        assert!((found[0].score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn deezer_prefers_the_xl_cover_and_falls_back_to_big_and_refuses_http() {
        let (title, artist) = wanted();
        let found = parse_deezer(DEEZER, title, artist);
        assert_eq!(found.len(), 2, "the plain-http item is dropped");
        assert!(found[0].full_url.contains("1000x1000"));
        assert!(found[1].full_url.contains("500x500"), "cover_big is the fallback");
        assert!(found[0].thumbnail_url.contains("250x250"));
    }

    #[test]
    fn musicbrainz_groups_become_cover_art_archive_urls_and_odd_ids_are_ignored() {
        let (title, artist) = wanted();
        let found = parse_musicbrainz(MUSICBRAINZ, title, artist);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, "Cover Art Archive");
        assert_eq!(
            found[0].thumbnail_url,
            "https://coverartarchive.org/release-group/11111111-2222-3333-4444-555555555555/front-250"
        );
        assert_eq!(
            found[0].full_url,
            "https://coverartarchive.org/release-group/11111111-2222-3333-4444-555555555555/front-1200"
        );
        assert_eq!(found[0].artist, "Boards of Canada");
    }

    #[test]
    fn a_garbled_answer_is_no_answer() {
        assert!(parse_apple("<html>", "a", "b").is_empty());
        assert!(parse_deezer("{}", "a", "b").is_empty());
        assert!(parse_musicbrainz("[]", "a", "b").is_empty());
    }

    fn candidate(id: &str, url: &str, score: f64) -> Candidate {
        Candidate {
            id: id.into(),
            source: "Apple Music",
            title: String::new(),
            artist: String::new(),
            thumbnail_url: String::new(),
            full_url: url.into(),
            score,
        }
    }

    #[test]
    fn ranking_dedupes_by_full_url_and_leads_with_the_best_score() {
        let ranked = rank(
            vec![
                candidate("a", "https://x/1", 0.4),
                candidate("b", "https://x/2", 0.9),
                candidate("c", "https://x/1", 0.95),
            ],
            true,
        );
        let ids: Vec<&str> = ranked.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a"], "the first sighting of a URL stands");
    }

    #[test]
    fn three_confident_matches_hide_the_weak_ones_unless_the_query_was_edited() {
        let mixed = || {
            vec![
                candidate("a", "https://x/1", 1.0),
                candidate("b", "https://x/2", 0.85),
                candidate("c", "https://x/3", 0.7),
                candidate("d", "https://x/4", 0.3),
                candidate("e", "https://x/5", 0.0),
            ]
        };
        assert_eq!(rank(mixed(), true).len(), 3);
        assert_eq!(rank(mixed(), false).len(), 5, "an edited query is not filtered");
        let two_good = vec![
            candidate("a", "https://x/1", 1.0),
            candidate("b", "https://x/2", 0.85),
            candidate("d", "https://x/4", 0.3),
        ];
        assert_eq!(rank(two_good, true).len(), 3, "fewer than three confident keeps everything");
    }

    #[test]
    fn ranking_is_capped_at_eighteen() {
        let many: Vec<Candidate> = (0..40)
            .map(|index| candidate(&index.to_string(), &format!("https://x/{index}"), 0.6))
            .collect();
        assert_eq!(rank(many, true).len(), 18);
    }

    #[test]
    fn the_default_query_leaves_out_various_artists() {
        assert_eq!(default_query("Score", "Various Artists"), "Score");
        assert_eq!(default_query("Score", "various artists"), "Score");
        assert_eq!(default_query("Record", "Band"), "Band Record");
    }

    #[test]
    fn an_untouched_query_is_structured_and_an_edited_one_is_free_text() {
        let untouched = Request::new("Band Record", "Record", "Band");
        assert!(!untouched.edited);
        assert_eq!(untouched.deezer_query(), "artist:\"Band\" album:\"Record\"");
        assert_eq!(
            untouched.musicbrainz_query(),
            "releasegroup:\"Record\" AND artist:\"Band\""
        );
        let edited = Request::new("record deluxe", "Record", "Band");
        assert!(edited.edited);
        assert_eq!(edited.deezer_query(), "record deluxe");
        assert_eq!(edited.musicbrainz_query(), "record deluxe");
        let compilation = Request::new("Score", "Score", "Various Artists");
        assert!(!compilation.edited);
        assert_eq!(compilation.artist, "");
        assert_eq!(compilation.musicbrainz_query(), "releasegroup:\"Score\"");
    }

    #[test]
    fn quotes_in_a_title_cannot_break_out_of_a_query() {
        let request = Request::new(&default_query("Say \"Hi\" \\ there", "A\"B"), "Say \"Hi\" \\ there", "A\"B");
        let query = request.musicbrainz_query();
        assert_eq!(query.matches('"').count(), 4, "only the four delimiters: {query}");
        assert!(!query.contains('\\'));
    }

    #[test]
    fn percent_encoding_covers_spaces_and_non_ascii() {
        assert_eq!(percent_encode("a b&c"), "a%20b%26c");
        assert_eq!(percent_encode("é"), "%C3%A9");
        assert_eq!(percent_encode("safe-_.~09"), "safe-_.~09");
    }

    #[test]
    fn relative_redirects_resolve_against_the_current_url() {
        assert_eq!(
            resolve_location("https://a.example/x/y.jpg", "/z.jpg"),
            "https://a.example/z.jpg"
        );
        assert_eq!(
            resolve_location("https://a.example/x/y.jpg", "w.jpg"),
            "https://a.example/x/w.jpg"
        );
        assert_eq!(
            resolve_location("https://a.example", "https://b.example/q"),
            "https://b.example/q"
        );
    }

    #[test]
    fn plain_http_is_refused_unless_the_person_supplied_the_link() {
        assert!(matches!(
            fetch_bytes("http://example.invalid/a.jpg", false, 10),
            Err(FetchError::Insecure)
        ));
        assert!(matches!(
            fetch_bytes("ftp://example.invalid/a.jpg", true, 10),
            Err(FetchError::Insecure)
        ));
    }
}
