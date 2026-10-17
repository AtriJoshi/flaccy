use flaccy_shared::artwork_local;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use lofty::probe::Probe;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub const MAXIMUM_EMBEDDED: usize = 8;

#[derive(Clone, Debug)]
pub enum Origin {
    File(PathBuf),
    Embedded(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct LocalCover {
    pub caption: String,
    pub origin: Origin,
    pub width: u32,
    pub height: u32,
}

impl LocalCover {
    /// The image's bytes, read from the disk for a file and held for a picture
    /// that was embedded in a track. None when the file has gone or grew past
    /// the size the shelf offers.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        match &self.origin {
            Origin::Embedded(bytes) => Some(bytes.clone()),
            Origin::File(path) => read_candidate(path),
        }
    }
}

/// Images lying in the release's folders, in the shared order: the shared
/// parent first, then each track's own folder, front covers before everything
/// else. The library root is never listed, and one picture copied into every
/// disc folder appears once.
pub fn folder_images(root: &Path, relative_paths: &[String]) -> Vec<LocalCover> {
    let mut covers = Vec::new();
    let mut seen = HashSet::new();
    for folder in artwork_local::folders(relative_paths) {
        let directory = root.join(&folder);
        for name in ranked_names(&directory) {
            let path = directory.join(&name);
            let Some(bytes) = read_candidate(&path) else {
                continue;
            };
            if !seen.insert(content_hash(&bytes)) {
                continue;
            }
            let Some((width, height)) = crate::artwork_image::dimensions(&bytes) else {
                continue;
            };
            covers.push(LocalCover {
                caption: name,
                origin: Origin::File(path),
                width,
                height,
            });
        }
    }
    covers
}

fn ranked_names(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    artwork_local::rank(&names)
}

/// A candidate file's bytes, or None when it is unreadable or larger than the
/// shelf's size limit.
fn read_candidate(path: &Path) -> Option<Vec<u8>> {
    let length = std::fs::metadata(path).ok()?.len();
    if length > artwork_local::MAXIMUM_FILE_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// The pictures embedded in the release's tracks, read from the files on
/// demand because the database keeps one cover per release, not one per track.
/// Front covers are preferred, identical pictures count once, and at most
/// eight are returned. `tracks` pairs each file with its track number.
pub fn embedded_images(root: &Path, tracks: &[(String, i32)]) -> Vec<LocalCover> {
    let mut covers = Vec::new();
    let mut seen = HashSet::new();
    for (index, (relative, number)) in tracks.iter().enumerate() {
        if covers.len() >= MAXIMUM_EMBEDDED {
            break;
        }
        let Some(bytes) = embedded_picture(&root.join(relative)) else {
            continue;
        };
        if !seen.insert(content_hash(&bytes)) {
            continue;
        }
        let Some((width, height)) = crate::artwork_image::dimensions(&bytes) else {
            continue;
        };
        let number = if *number > 0 { *number } else { index as i32 + 1 };
        covers.push(LocalCover {
            caption: format!("Track {number}"),
            origin: Origin::Embedded(bytes),
            width,
            height,
        });
    }
    covers
}

fn embedded_picture(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    tag.pictures()
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
        .map(|picture| picture.data().to_vec())
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture};
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};
    use std::io::Cursor;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let unique = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "flaccy-local-art-{}-{unique}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch dir");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn picture(width: u32, height: u32, shade: u8, format: ImageFormat) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([shade, 40, 90, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, format)
            .expect("encode");
        out.into_inner()
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join(name), bytes).expect("write");
    }

    #[test]
    fn folder_images_are_ranked_and_junk_is_left_out() {
        let scratch = Scratch::new();
        let album = scratch.0.join("Artist/Album");
        write(&album, "01 Song.flac", b"audio");
        write(&album, "back.jpg", &picture(40, 40, 1, ImageFormat::Jpeg));
        write(&album, "cover.png", &picture(64, 48, 2, ImageFormat::Png));
        write(&album, "AlbumArtSmall.jpg", &picture(10, 10, 3, ImageFormat::Jpeg));
        write(&album, "._cover.jpg", &picture(10, 10, 4, ImageFormat::Jpeg));
        write(&album, "notes.txt", b"hello");
        write(&album, "folder.jpg", &picture(50, 50, 5, ImageFormat::Jpeg));

        let found = folder_images(&scratch.0, &["Artist/Album/01 Song.flac".to_string()]);
        let names: Vec<&str> = found.iter().map(|c| c.caption.as_str()).collect();
        assert_eq!(names, vec!["cover.png", "folder.jpg", "back.jpg"]);
        assert_eq!((found[0].width, found[0].height), (64, 48));
        assert!(found[0].bytes().is_some());
    }

    #[test]
    fn disc_folders_are_listed_after_their_shared_parent_and_the_root_never() {
        let scratch = Scratch::new();
        write(&scratch.0, "cover.jpg", &picture(30, 30, 9, ImageFormat::Jpeg));
        let album = scratch.0.join("Artist/Album");
        write(&album, "cover.jpg", &picture(30, 30, 10, ImageFormat::Jpeg));
        write(&album.join("CD1"), "disc.jpg", &picture(30, 30, 100, ImageFormat::Jpeg));
        write(&album.join("CD2"), "scan.jpg", &picture(30, 30, 200, ImageFormat::Jpeg));

        let found = folder_images(
            &scratch.0,
            &[
                "Artist/Album/CD2/01.flac".to_string(),
                "Artist/Album/CD1/01.flac".to_string(),
            ],
        );
        let places: Vec<String> = found
            .iter()
            .map(|cover| match &cover.origin {
                Origin::File(path) => path
                    .strip_prefix(&scratch.0)
                    .expect("under root")
                    .display()
                    .to_string(),
                Origin::Embedded(_) => String::new(),
            })
            .collect();
        assert_eq!(
            places,
            vec![
                "Artist/Album/cover.jpg",
                "Artist/Album/CD1/disc.jpg",
                "Artist/Album/CD2/scan.jpg"
            ]
        );
    }

    #[test]
    fn one_picture_copied_into_every_disc_folder_is_offered_once() {
        let scratch = Scratch::new();
        let same = picture(30, 30, 7, ImageFormat::Jpeg);
        let album = scratch.0.join("Artist/Album");
        write(&album.join("CD1"), "cover.jpg", &same);
        write(&album.join("CD2"), "cover.jpg", &same);
        let found = folder_images(
            &scratch.0,
            &[
                "Artist/Album/CD1/01.flac".to_string(),
                "Artist/Album/CD2/01.flac".to_string(),
            ],
        );
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn a_file_over_the_size_limit_is_skipped_and_an_unreadable_image_has_no_size() {
        let scratch = Scratch::new();
        let album = scratch.0.join("A/B");
        write(&album, "cover.jpg", b"this is not an image");
        let found = folder_images(&scratch.0, &["A/B/01.flac".to_string()]);
        assert!(found.is_empty(), "bytes that are not an image are not offered");

        let huge = album.join("front.png");
        let file = std::fs::File::create(&huge).expect("create");
        file.set_len(artwork_local::MAXIMUM_FILE_BYTES + 1).expect("sparse file");
        assert!(read_candidate(&huge).is_none());
    }

    #[test]
    fn a_missing_folder_is_an_empty_shelf() {
        let scratch = Scratch::new();
        assert!(folder_images(&scratch.0, &["Nope/Nothing/01.flac".to_string()]).is_empty());
        assert!(folder_images(&scratch.0, &[]).is_empty());
    }

    fn silent_mp3(path: &Path, pictures: &[(Vec<u8>, PictureType)]) {
        let mut frames = Vec::new();
        for _ in 0..4 {
            let mut frame = vec![0xFF, 0xFB, 0x90, 0x00];
            frame.resize(417, 0);
            frames.extend(frame);
        }
        std::fs::write(path, &frames).expect("mp3 body");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("t".to_string());
        for (bytes, kind) in pictures {
            let mut picture = Picture::new_unchecked(
                *kind,
                Some(MimeType::Jpeg),
                None,
                bytes.clone(),
            );
            picture.set_description(None);
            tag.push_picture(picture);
        }
        tag.save_to_path(path, WriteOptions::default()).expect("tag saved");
    }

    #[test]
    fn embedded_pictures_prefer_the_front_cover_dedupe_and_stop_at_eight() {
        let scratch = Scratch::new();
        let album = scratch.0.join("Artist/Album");
        std::fs::create_dir_all(&album).expect("dir");
        let front = picture(60, 60, 20, ImageFormat::Jpeg);
        let back = picture(61, 61, 21, ImageFormat::Jpeg);
        silent_mp3(
            &album.join("01.mp3"),
            &[(back.clone(), PictureType::CoverBack), (front.clone(), PictureType::CoverFront)],
        );
        silent_mp3(&album.join("02.mp3"), &[(front.clone(), PictureType::CoverFront)]);
        silent_mp3(&album.join("03.mp3"), &[]);
        for index in 4..=14 {
            let shade = 30 + index as u8;
            silent_mp3(
                &album.join(format!("{index:02}.mp3")),
                &[(picture(70, 70, shade, ImageFormat::Jpeg), PictureType::Other)],
            );
        }
        let tracks: Vec<(String, i32)> = (1..=14)
            .map(|index| (format!("Artist/Album/{index:02}.mp3"), index))
            .collect();

        let found = embedded_images(&scratch.0, &tracks);
        assert_eq!(found.len(), MAXIMUM_EMBEDDED, "capped at eight distinct pictures");
        assert_eq!(found[0].caption, "Track 1");
        assert_eq!((found[0].width, found[0].height), (60, 60), "front cover beat the back");
        assert_eq!(found[1].caption, "Track 4", "track 2 repeated track 1 and track 3 had none");
        assert_eq!(found[1].bytes().expect("held").len(), picture(70, 70, 34, ImageFormat::Jpeg).len());
    }

    #[test]
    fn a_track_that_cannot_be_read_is_just_skipped() {
        let scratch = Scratch::new();
        write(&scratch.0, "broken.mp3", b"not audio");
        let found = embedded_images(
            &scratch.0,
            &[("broken.mp3".to_string(), 1), ("missing.mp3".to_string(), 2)],
        );
        assert!(found.is_empty());
    }
}
