use flaccy_shared::artwork_framing::ArtworkFraming;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use std::io::Cursor;

/// The long edge a source image is decoded at. Interaction and the saved
/// cover both work from this one decode, so what the person frames is exactly
/// what gets rendered.
pub const DECODE_LIMIT: u32 = 3200;

/// The side of the square a saved cover is rendered at, at most.
pub const OUTPUT_LIMIT: u32 = 1600;

const JPEG_QUALITY: u8 = 90;

/// Decodes `bytes`, upright and no larger than `limit` on the long edge. None
/// when the bytes are not an image the build can read.
pub fn decode(bytes: &[u8], limit: u32) -> Option<RgbaImage> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Some(reduced(image, limit).to_rgba8())
}

/// The pixel size of an image without decoding it.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// A shelf thumbnail: decoded, upright and fitted inside `side` × `side`.
pub fn thumbnail(bytes: &[u8], side: u32) -> Option<RgbaImage> {
    decode(bytes, side)
}

fn reduced(image: DynamicImage, limit: u32) -> DynamicImage {
    if image.width().max(image.height()) <= limit {
        return image;
    }
    image.resize(limit, limit, FilterType::Lanczos3)
}

/// The cover the framing describes, as a square JPEG: the visible source
/// square resampled to `framing.output_pixels(OUTPUT_LIMIT)` with Lanczos,
/// transparency flattened onto black the way a JPEG has to be.
pub fn render_jpeg(source: &RgbaImage, framing: &ArtworkFraming) -> Option<Vec<u8>> {
    let square = crop_visible(source, framing)?;
    let side = framing.output_pixels(OUTPUT_LIMIT);
    let resampled = if square.width() == side && square.height() == side {
        square
    } else {
        imageops::resize(&square, side, side, FilterType::Lanczos3)
    };
    encode_jpeg(&resampled)
}

/// The source rectangle the window shows, clamped inside the image and made
/// square after rounding to whole pixels.
fn crop_visible(source: &RgbaImage, framing: &ArtworkFraming) -> Option<RgbaImage> {
    let (width, height) = source.dimensions();
    let side = (framing.visible_side().round() as u32).clamp(1, width.min(height));
    let x = (framing.visible_x().round().max(0.0) as u32).min(width - side);
    let y = (framing.visible_y().round().max(0.0) as u32).min(height - side);
    Some(imageops::crop_imm(source, x, y, side, side).to_image())
}

fn encode_jpeg(image: &RgbaImage) -> Option<Vec<u8>> {
    let flattened = flatten_onto_black(image);
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&flattened)
        .ok()?;
    Some(out)
}

fn flatten_onto_black(image: &RgbaImage) -> DynamicImage {
    let mut rgb = image::RgbImage::new(image.width(), image.height());
    for (target, pixel) in rgb.pixels_mut().zip(image.pixels()) {
        let alpha = pixel[3] as u32;
        let blend = |channel: u8| ((channel as u32 * alpha + 127) / 255) as u8;
        *target = image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]);
    }
    DynamicImage::ImageRgb8(rgb)
}

/// A level of a source image reduced by powers of two, so a zoomed-out window
/// never scales a 3 000 px image down by ten in one bilinear step, which
/// shimmers while a drag moves it.
pub struct MipChain {
    levels: Vec<RgbaImage>,
}

impl MipChain {
    pub fn new(source: RgbaImage) -> Self {
        let mut levels = vec![source];
        while let Some(last) = levels.last() {
            let (width, height) = last.dimensions();
            if width.min(height) < 400 {
                break;
            }
            let next = imageops::resize(last, (width / 2).max(1), (height / 2).max(1), FilterType::Triangle);
            levels.push(next);
        }
        Self { levels }
    }

    pub fn full(&self) -> &RgbaImage {
        &self.levels[0]
    }

    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// The most detailed level that is scaled down by no more than half to
    /// reach the screen, with the factor mapping its coordinates back to the
    /// full image.
    pub fn level_for(&self, device_pixels_per_source_pixel: f64) -> (usize, f64) {
        let mut index = 0usize;
        while index + 1 < self.levels.len()
            && device_pixels_per_source_pixel * 2f64.powi(index as i32) < 0.5
        {
            index += 1;
        }
        (index, 2f64.powi(index as i32))
    }

    pub fn level(&self, index: usize) -> &RgbaImage {
        &self.levels[index.min(self.levels.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageFormat, Rgba};

    fn gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([(x * 255 / width.max(1)) as u8, (y * 255 / height.max(1)) as u8, 90, 255])
        })
    }

    fn encoded(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut out, format)
            .expect("encode fixture");
        out.into_inner()
    }

    #[test]
    fn decoding_reduces_a_large_image_to_the_limit_and_keeps_its_shape() {
        let bytes = encoded(&gradient(800, 400), ImageFormat::Png);
        let decoded = decode(&bytes, 200).expect("decodes");
        assert_eq!(decoded.dimensions(), (200, 100));
        let small = decode(&bytes, DECODE_LIMIT).expect("decodes");
        assert_eq!(small.dimensions(), (800, 400), "never upscaled");
    }

    #[test]
    fn bytes_that_are_not_an_image_do_not_decode() {
        assert!(decode(b"not an image at all", DECODE_LIMIT).is_none());
        assert!(dimensions(b"nope").is_none());
    }

    #[test]
    fn dimensions_read_the_header_only() {
        let bytes = encoded(&gradient(123, 45), ImageFormat::Png);
        assert_eq!(dimensions(&bytes), Some((123, 45)));
    }

    #[test]
    fn the_saved_cover_is_a_square_jpeg_at_the_framing_output_size() {
        let source = gradient(3000, 2000);
        let framing = ArtworkFraming::new(3000.0, 2000.0, 400.0).expect("framing");
        let jpeg = render_jpeg(&source, &framing).expect("renders");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
        let back = image::load_from_memory(&jpeg).expect("reads back");
        assert_eq!(back.dimensions(), (1600, 1600), "capped at the output limit");
        assert_eq!(framing.output_pixels(OUTPUT_LIMIT), 1600);
    }

    #[test]
    fn a_small_source_is_not_blown_up_past_what_it_can_supply() {
        let source = gradient(900, 900);
        let framing = ArtworkFraming::new(900.0, 900.0, 400.0).expect("framing");
        let jpeg = render_jpeg(&source, &framing).expect("renders");
        let back = image::load_from_memory(&jpeg).expect("reads back");
        assert_eq!(back.dimensions(), (900, 900));
    }

    #[test]
    fn a_zoomed_framing_renders_only_what_the_window_shows() {
        let mut source = RgbaImage::from_pixel(2000, 2000, Rgba([0, 0, 255, 255]));
        for y in 0..500 {
            for x in 0..500 {
                source.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let fill = ArtworkFraming::new(2000.0, 2000.0, 400.0).expect("framing");
        let corner = fill.zoomed(fill.fill_zoom() * 4.0, Some((0.0, 0.0)));
        assert!((corner.visible_side() - 500.0).abs() < 1e-6);
        let jpeg = render_jpeg(&source, &corner).expect("renders");
        let back = image::load_from_memory(&jpeg).expect("reads back").to_rgb8();
        assert_eq!(back.dimensions(), (500, 500));
        let middle = back.get_pixel(250, 250);
        assert!(middle[0] > 200 && middle[2] < 60, "the red corner fills the cover: {middle:?}");
    }

    #[test]
    fn transparency_is_flattened_onto_black() {
        let source = RgbaImage::from_pixel(600, 600, Rgba([255, 255, 255, 0]));
        let framing = ArtworkFraming::new(600.0, 600.0, 400.0).expect("framing");
        let jpeg = render_jpeg(&source, &framing).expect("renders");
        let back = image::load_from_memory(&jpeg).expect("reads back").to_rgb8();
        assert!(back.get_pixel(10, 10)[0] < 10);
    }

    #[test]
    fn the_mip_chain_picks_a_level_that_never_undersamples() {
        let chain = MipChain::new(gradient(3200, 3200));
        let (full, factor) = chain.level_for(1.0);
        assert_eq!((full, factor), (0, 1.0), "at one device pixel per source pixel use the original");
        let (level, factor) = chain.level_for(0.125);
        assert_eq!((level, factor), (2, 4.0), "zoomed far out uses a reduced level");
        assert_eq!(chain.level(level).width(), 800);
        for density in [0.5, 0.3, 0.2, 0.07] {
            let (shown, _) = chain.level_for(density);
            let scale = density * 2f64.powi(shown as i32);
            assert!((0.5..1.0 + 1e-9).contains(&scale), "never shrunk by more than half: {density} -> {scale}");
        }
    }
}
