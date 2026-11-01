//! An animated tile picture: GIF frames decoded once, cover-cropped to the
//! tile, masked to its rounded corners, and cached per file + draw size, so
//! the renderer picks a frame from a `Vec<Handle>` and nothing is decoded in
//! `view`.
//!
//! Everything else — PNG, JPG, WebP, SVG, a still GIF — stays on the single-
//! handle `tileimage::masked` path. The format test is byte-level, so a
//! file's name is never the authority.
//!
//! Decoded straight into tile pixel sizes (never a 4K GIF at 150 px) through
//! the same mask the still path uses, so a GIF corner is cut exactly as its
//! PNG neighbour's would be. If the file is missing or malformed, the loader
//! returns `None` and the caller falls back; the UI never panics on a tile.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use cosmic::widget::image::Handle;
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, ImageDecoder, Limits};

/// A decoded animated image, ready to draw.
pub struct Animation {
    /// One handle per frame, each already masked to the tile's rounded corners.
    pub frames: Vec<Handle>,
    /// Per-frame display time, in the same order as `frames`.
    pub delays: Vec<Duration>,
}

/// Whether the bytes at the start of `data` are a GIF — magic alone, so a
/// `.jpg` saved from an animated GIF still animates and a `.gif` of a single
/// still still-frames correctly.
pub fn is_gif(data: &[u8]) -> bool {
    data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")
}

/// Whether the file at `path` begins with a GIF magic, reading only its first
/// six bytes.
///
/// The render path sniffs every tile on every frame, so a full read — a video,
/// a 200 MB photo set as a tile picture — to test six bytes is not acceptable.
pub fn is_gif_file(path: &str) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 6];
    file.read_exact(&mut head).is_ok() && is_gif(&head)
}

/// Whether the file at `path` is an animated GIF with more than one frame.
///
/// A single-frame GIF reads as a still: a one-frame "animation" is a still
/// picture, and `tileimage::masked` draws stills more cheaply.
#[allow(dead_code)] // public helper with its own tests; the UI reaches the
                    // same decision via `load(..).frames.len() > 1`.
pub fn is_animated(path: &str) -> bool {
    load(path, 1, 1, 0.0).is_some_and(|a| a.frames.len() > 1)
}

/// What the cache stores a decoded animation under: the file, the tile's
/// drawn pixel size, and the rounded-corner radius. Rounded to whole pixels,
/// which is all a mask can resolve.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    w: u32,
    h: u32,
    radius: u32,
}

type Cache = HashMap<Key, Option<std::sync::Arc<Animation>>>;

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The animation for `path`, decoded to `w`x`h` and masked to `radius`, or
/// `None` for a malformed or missing file (so the caller falls back to the
/// finish). A still file at that path, including a one-frame GIF, is still
/// handled here; it comes back as a one-frame `Animation`, which the UI then
/// draws through the still path instead.
///
/// Cached by `path + w + h + radius`, so a `view` builds no bytes.
pub fn load(path: &str, w: u32, h: u32, radius: f32) -> Option<std::sync::Arc<Animation>> {
    let key = Key {
        path: path.to_owned(),
        w: w.max(1),
        h: h.max(1),
        radius: radius.round().max(0.0) as u32,
    };
    if let Ok(mut guard) = CACHE.lock() {
        if let Some(hit) = guard.get_or_insert_with(HashMap::new).get(&key) {
            return hit.clone();
        }
    }
    let built = decode(&key).map(std::sync::Arc::new);
    if let Ok(mut guard) = CACHE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(key, built.clone());
    }
    built
}

/// The most frames a tile animation is decoded to. A tile picture is a file
/// the user points at, and a long looping GIF there should not be allowed to
/// fill memory with frames the menu will never show.
/// ponytail: one flat ceiling; raise it if a genuinely longer loop is wanted.
const MAX_FRAMES: usize = 120;

fn decode(key: &Key) -> Option<Animation> {
    let data = std::fs::read(&key.path).ok()?;
    if !is_gif(&data) {
        return None;
    }
    let mut decoder = GifDecoder::new(std::io::Cursor::new(data)).ok()?;
    // A GIF header declares its canvas and `image`'s decoder allocates it
    // before reading a frame — 65535x65535 is 17 GB from a ten-byte header,
    // which kills a resident applet (it also decodes with `no_limits` by
    // default). A tile picture is drawn at a few hundred pixels.
    // ponytail: one flat ceiling; a legitimately larger picture falls back to
    // the still path, which has its own cap.
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 << 20);
    decoder.set_limits(limits).ok()?;
    // Decoded a frame at a time, not collected first: the decoder hands each
    // frame back at the GIF's full canvas, so collecting them all costs
    // canvas x frames (a 1080p, 100-frame GIF is most of a gigabyte) before a
    // single resize. Resizing as we go keeps only the tile-sized frames live.
    // A malformed frame means "no animation" — the still path draws what it
    // can of the file instead.
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    for frame in decoder.into_frames().take(MAX_FRAMES) {
        let frame = frame.ok()?;
        let delay = Duration::from(frame.delay());
        // Clamp to a sensible minimum: the GIF spec's 100 Hz headroom reads
        // as "animate as fast as possible" and some pack `delay = 0`.
        let delay = delay.max(Duration::from_millis(20));
        let mut rgba = fit(frame.into_buffer(), key.w, key.h).into_raw();
        crate::tileimage::apply_mask(&mut rgba, key.w, key.h, key.radius as f32);
        frames.push(Handle::from_rgba(key.w, key.h, rgba));
        delays.push(delay);
    }
    if frames.is_empty() {
        return None;
    }
    Some(Animation { frames, delays })
}

/// Scale one frame to the tile, cover-cropped, the same treatment
/// `tileimage::build` gives a still — so a non-square GIF keeps its aspect
/// ratio instead of being stretched to the tile's box.
fn fit(buffer: image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    image::DynamicImage::ImageRgba8(buffer)
        .resize_to_fill(w, h, image::imageops::FilterType::Triangle)
        .into_rgba8()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_magic_is_bytes_not_the_name() {
        assert!(is_gif(b"GIF89a\0\0rest"));
        assert!(is_gif(b"GIF87a\0\0rest"));
        assert!(!is_gif(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_gif(b""));
        assert!(!is_gif(b"GIF"));
    }

    /// A minimal two-frame GIF, built via the `image` crate's own encoder
    /// — the only reliable way to hand the decoder a well-formed fixture.
    fn two_frame_gif() -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Rgba, RgbaImage};
        let mut buf = Vec::new();
        {
            let mut enc = GifEncoder::new_with_speed(&mut buf, 10);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for colour in [[0u8, 0, 0, 255], [255, 255, 255, 255]] {
                let mut frame = RgbaImage::new(4, 4);
                for p in frame.pixels_mut() {
                    *p = Rgba(colour);
                }
                enc.encode_frame(image::Frame::from_parts(
                    frame,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(10, 1),
                ))
                .unwrap();
            }
        }
        buf
    }

    #[test]
    fn a_real_multi_frame_gif_decodes_through_the_cache() {
        let tmp = std::env::temp_dir().join(format!("start-menu-anim-{}.gif", std::process::id()));
        std::fs::write(&tmp, two_frame_gif()).unwrap();
        let path = tmp.to_string_lossy().into_owned();

        let a = load(&path, 16, 16, 4.0).expect("two-frame gif should decode");
        assert_eq!(a.frames.len(), 2);
        assert_eq!(a.delays.len(), 2);
        // The hand-coded 10ms delay is below the 20ms clamp.
        assert_eq!(a.delays[0], Duration::from_millis(20));

        // Second call reuses the Arc rather than re-decoding.
        let b = load(&path, 16, 16, 4.0).unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        assert!(is_animated(&path));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn a_non_gif_file_is_not_animated() {
        let tmp = std::env::temp_dir().join(format!("start-menu-png-{}.png", std::process::id()));
        std::fs::write(&tmp, b"\x89PNG\r\n\x1a\n").unwrap();
        let path = tmp.to_string_lossy().into_owned();
        assert!(load(&path, 16, 16, 0.0).is_none());
        assert!(!is_animated(&path));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn the_file_sniff_needs_only_the_magic() {
        let dir = std::env::temp_dir();
        let gif = dir.join(format!("start-menu-sniff-{}.gif", std::process::id()));
        std::fs::write(&gif, two_frame_gif()).unwrap();
        assert!(is_gif_file(&gif.to_string_lossy()));
        let png = dir.join(format!("start-menu-sniff-{}.png", std::process::id()));
        // A file too short to hold a magic is a false, never a panic.
        std::fs::write(&png, b"\x89PNG").unwrap();
        assert!(!is_gif_file(&png.to_string_lossy()));
        assert!(!is_gif_file("/does/not/exist.gif"));
        let _ = std::fs::remove_file(&gif);
        let _ = std::fs::remove_file(&png);
    }

    #[test]
    fn a_frame_is_cover_cropped_not_stretched() {
        // 8x4, leftmost column red. Cover-cropped into a 16x16 tile (scale 4
        // -> 32 wide, centre-cropped to 16) that column falls outside the
        // crop; a stretch to 16x16 would keep it against the left edge.
        let mut src = image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 255, 255, 255]));
        for y in 0..4 {
            src.put_pixel(0, y, image::Rgba([255, 0, 0, 255]));
        }
        let out = fit(src, 16, 16);
        assert_eq!(out.dimensions(), (16, 16));
        assert!(
            !out.pixels().any(|p| p.0[0] > 200 && p.0[1] < 100),
            "the cropped-away red column survived: the frame was stretched, not cropped"
        );
    }

    /// `n` frames, so the frame cap can be tested without a huge fixture.
    fn long_gif(n: usize) -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Rgba, RgbaImage};
        let mut buf = Vec::new();
        {
            let mut enc = GifEncoder::new_with_speed(&mut buf, 30);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..n {
                let shade = (i % 2) as u8 * 255;
                let frame = RgbaImage::from_pixel(4, 4, Rgba([shade, 0, 0, 255]));
                enc.encode_frame(image::Frame::from_parts(
                    frame,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
            }
        }
        buf
    }

    #[test]
    fn a_very_long_gif_is_capped_rather_than_held_whole() {
        let tmp = std::env::temp_dir().join(format!("start-menu-long-{}.gif", std::process::id()));
        std::fs::write(&tmp, long_gif(MAX_FRAMES + 7)).unwrap();
        let path = tmp.to_string_lossy().into_owned();
        let a = load(&path, 8, 8, 0.0).expect("long gif should still decode");
        assert_eq!(a.frames.len(), MAX_FRAMES);
        assert_eq!(a.delays.len(), MAX_FRAMES);
        let _ = std::fs::remove_file(&tmp);
    }

    /// A 5000x5000 canvas holding one 1x1 frame: valid GIF, 100 MB of canvas
    /// to allocate before the frame is ever reached.
    fn big_canvas_gif() -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&5000u16.to_le_bytes()); // logical screen width
        g.extend_from_slice(&5000u16.to_le_bytes()); // height
        g.extend_from_slice(&[0x80, 0x00, 0x00]); // global colour table, 2 entries
        g.extend_from_slice(&[0, 0, 0, 255, 255, 255]); // black, white
        g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]); // 1x1 image
        g.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]); // LZW: clear, pixel, end
        g.push(0x3B); // trailer
        g
    }

    #[test]
    fn a_huge_declared_canvas_is_refused_not_allocated() {
        let bytes = big_canvas_gif();
        // The control: the bytes really do decode when the decoder is left
        // unlimited, which is how `image` starts out. So the refusal below is
        // the limits, not a broken fixture.
        let control = GifDecoder::new(std::io::Cursor::new(bytes.clone())).unwrap();
        assert_eq!(control.into_frames().count(), 1);

        let tmp = std::env::temp_dir().join(format!("start-menu-huge-{}.gif", std::process::id()));
        std::fs::write(&tmp, &bytes).unwrap();
        let path = tmp.to_string_lossy().into_owned();
        assert!(load(&path, 16, 16, 0.0).is_none());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn a_missing_file_is_graceful() {
        assert!(load("/does/not/exist.gif", 16, 16, 0.0).is_none());
        assert!(!is_animated("/does/not/exist.gif"));
    }
}
