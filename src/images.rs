//! Inline images for the preview.
//!
//! If the terminal speaks a graphics protocol (kitty — so Ghostty — iTerm2 or
//! sixel), `![alt](path)` lines are drawn as pictures. If it doesn't, or the
//! file isn't there, the preview keeps the styled `🖼 alt (path)` line it
//! already has. Nothing here is allowed to fail loudly.

use image::GenericImageView;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Tallest an inline image may get, in terminal rows.
const MAX_ROWS: u16 = 20;

/// A decoded image, already scaled to the size it is drawn at. `None` means we
/// tried and it isn't drawable.
///
/// The protocol state is built from the *fitted* picture rather than the file's
/// own pixels, which is what lets a partially visible band be drawn with
/// [`ratatui_image::Resize::Crop`]: cropping takes pixels straight off the
/// source, so the source has to already be the size the whole band would be.
struct Cached {
    protocol: StatefulProtocol,
    /// The file as decoded, kept so a re-fit on a new page width is a scale
    /// and not a read of the file from disk in the middle of a window drag.
    source: image::DynamicImage,
    /// The original file's pixel size, for re-fitting when the page resizes.
    natural: (u32, u32),
    /// The pixel size the protocol was built at.
    fit: (u32, u32),
    /// Rows the fitted picture occupies.
    rows: u16,
}

/// What [`Images::zoom`] hands back: cells occupied, natural pixel size, and
/// the protocol to draw with.
pub type Zoom<'a> = ((u16, u16), (u32, u32), &'a mut StatefulProtocol);

/// The one picture drawn over the whole terminal, when there is one. Kept
/// apart from the band cache because it is fitted to the window rather than
/// the page, and re-fitted when the window changes.
struct Zoomed {
    path: PathBuf,
    /// The terminal area it was fitted to, in cells.
    area: (u16, u16),
    /// Cells the fitted picture occupies, centred inside that area.
    size: (u16, u16),
    natural: (u32, u32),
    protocol: StatefulProtocol,
}

#[derive(Default)]
pub struct Images {
    picker: Option<Picker>,
    zoomed: Option<Zoomed>,
    /// The configured attachments directory, searched after the note's folder.
    attachments: PathBuf,
    /// Resolved path → decoded image; a `None` entry keeps the fallback line.
    cache: HashMap<PathBuf, Option<Cached>>,
}

impl Images {
    pub fn new(attachments: PathBuf) -> Images {
        Images {
            attachments,
            ..Default::default()
        }
    }

    /// Point at a new attachments directory, keeping the probed graphics
    /// support (the terminal is only asked once, at startup) and dropping the
    /// cache, since the same file name may now resolve elsewhere.
    pub fn set_attachments(&mut self, attachments: PathBuf) {
        self.attachments = attachments;
        self.cache.clear();
    }

    /// Ask the terminal what it supports. Call once, with the terminal in raw
    /// mode; a failure just means no inline images.
    ///
    /// Skipped when stdin isn't a terminal (there is nobody to answer the
    /// query) and under tmux, where the query can be answered through
    /// passthrough while the graphics payload itself is not wrapped — the
    /// `🖼 alt (path)` fallback line is the honest thing to draw there.
    pub fn probe(&mut self) {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || std::env::var_os("TMUX").is_some() {
            return;
        }
        self.picker = Picker::from_query_stdio().ok().filter(|p| {
            !matches!(
                p.protocol_type(),
                ratatui_image::picker::ProtocolType::Halfblocks
            )
        });
    }

    /// Where an image reference points: beside the note, under the note's own
    /// `attachments/`, or in the configured attachments directory.
    pub fn resolve(&self, url: &str, note_dir: &Path) -> Option<PathBuf> {
        resolve_in(url, note_dir, &self.attachments)
    }

    /// Height in rows for an image drawn into a page `cols` columns wide,
    /// loading and encoding it the first time. `None` when it can't be drawn.
    ///
    /// An image wider than the page is scaled down to fit, so the rows it needs
    /// shrink with it — reserving its full pixel height would leave a band of
    /// blank rows under a wide screenshot.
    ///
    /// `max_px` is a width the note asked for (Obsidian's `![[a.png|300]]`),
    /// in pixels: the picture is fitted to that many columns, or the page
    /// when the page is narrower.
    pub fn rows(&mut self, url: &str, note_dir: &Path, cols: u16, max_px: Option<u32>) -> Option<u16> {
        let picker = self.picker.as_ref()?;
        let path = self.resolve(url, note_dir)?;
        let (font_w, font_h) = picker.font_size();
        let cols = match max_px {
            Some(px) => cols.min(px_to_cols(px, font_w)),
            None => cols,
        };

        // decode once
        if !self.cache.contains_key(&path) {
            let decoded = image::ImageReader::open(&path)
                .ok()
                .and_then(|r| r.with_guessed_format().ok())
                .and_then(|r| r.decode().ok());
            let entry = decoded.map(|img| {
                let natural = img.dimensions();
                let (fit, rows) = fit_px(natural.0, natural.1, font_w, font_h, cols);
                Cached {
                    protocol: picker.new_resize_protocol(scaled(&img, fit)),
                    source: img,
                    natural,
                    fit,
                    rows,
                }
            });
            self.cache.insert(path.clone(), entry);
        }

        let entry = self.cache.get_mut(&path)?.as_mut()?;
        let (fit, rows) = fit_px(entry.natural.0, entry.natural.1, font_w, font_h, cols);
        if fit != entry.fit {
            // the page changed width: re-fit, so a crop still takes its pixels
            // from a picture that is exactly the size of the whole band
            entry.protocol = picker.new_resize_protocol(scaled(&entry.source, fit));
            entry.fit = fit;
            entry.rows = rows;
        }
        Some(entry.rows)
    }

    /// The protocol state to render with, once [`Images::rows`] said yes.
    pub fn protocol(&mut self, url: &str, note_dir: &Path) -> Option<&mut StatefulProtocol> {
        let path = self.resolve(url, note_dir)?;
        self.cache.get_mut(&path)?.as_mut().map(|c| &mut c.protocol)
    }

    /// The picture fitted to a whole terminal area — as large as the area
    /// allows, scaled up if it is smaller than the window — as the rectangle
    /// it should be drawn in, centred, its natural pixel size, and the
    /// protocol to draw it with. `None` when the terminal cannot draw
    /// pictures or the file is not one.
    pub fn zoom(&mut self, url: &str, note_dir: &Path, area: (u16, u16)) -> Option<Zoom<'_>> {
        let picker = self.picker.as_ref()?;
        let path = self.resolve(url, note_dir)?;
        let fresh = self
            .zoomed
            .as_ref()
            .is_some_and(|z| z.path == path && z.area == area);
        if !fresh {
            // the band cache already holds the decoded file when the page has
            // drawn it; only a picture never shown inline is read from disk
            let cached = self.cache.get(&path).and_then(|c| c.as_ref());
            let img = match cached {
                Some(c) => c.source.clone(),
                None => image::ImageReader::open(&path)
                    .ok()
                    .and_then(|r| r.with_guessed_format().ok())
                    .and_then(|r| r.decode().ok())?,
            };
            let natural = img.dimensions();
            let (font_w, font_h) = picker.font_size();
            let (px, size) = fill_px(natural.0, natural.1, font_w, font_h, area);
            self.zoomed = Some(Zoomed {
                path,
                area,
                size,
                natural,
                protocol: picker.new_resize_protocol(scaled(&img, px)),
            });
        }
        let z = self.zoomed.as_mut()?;
        Some((z.size, z.natural, &mut z.protocol))
    }

    /// Whether pictures go through kitty's protocol, where a picture is sent
    /// once and placed by the cells that name its rows — which is what lets a
    /// band be drawn part by part without sending it again.
    pub fn is_kitty(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|p| p.protocol_type() == ratatui_image::picker::ProtocolType::Kitty)
    }

    /// Drop the whole-terminal picture; the band cache is untouched.
    pub fn unzoom(&mut self) {
        self.zoomed = None;
    }
}

fn scaled(img: &image::DynamicImage, (w, h): (u32, u32)) -> image::DynamicImage {
    if img.dimensions() == (w, h) {
        img.clone()
    } else {
        img.resize_exact(w.max(1), h.max(1), image::imageops::FilterType::Triangle)
    }
}

/// The columns `px` pixels cover, at `font_w` pixels a cell: at least one.
fn px_to_cols(px: u32, font_w: u16) -> u16 {
    let cols = (px as f32 / font_w.max(1) as f32).ceil();
    cols.clamp(1.0, u16::MAX as f32) as u16
}

/// The pixel size an image of `w`x`h` is drawn at on a page `cols` columns
/// wide, and the rows it then occupies.
///
/// An image wider than the page is scaled down to fit, so the rows it needs
/// shrink with it, and one taller than [`MAX_ROWS`] is scaled again so it fits
/// that cap — otherwise the reserved band and the picture would disagree.
fn fit_px(w: u32, h: u32, font_w: u16, font_h: u16, cols: u16) -> ((u32, u32), u16) {
    let font_w = font_w.max(1) as f32;
    let font_h = font_h.max(1) as f32;
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let mut scale = (cols.max(1) as f32 * font_w / wf).min(1.0);
    let cap = MAX_ROWS as f32 * font_h;
    if hf * scale > cap {
        scale = cap / hf;
    }
    let rows = ((hf * scale / font_h).ceil() as u16).clamp(1, MAX_ROWS);
    let px = (
        (wf * scale).round().max(1.0) as u32,
        (hf * scale).round().max(1.0) as u32,
    );
    (px, rows)
}

/// The rows an image band actually occupies on a page `viewport` rows tall.
///
/// The natural height comes from [`fit_px`] and the *full* page width, so it
/// never changes with the scroll; the only clamp is the viewport itself, so a
/// band can always be shown whole at some scroll position.
pub fn band_rows(natural: u16, viewport: u16) -> u16 {
    natural.min(viewport.max(1)).max(1)
}

/// The slice of an image band that is on screen.
///
/// A band of `rows` rows starts at page row `start`; the viewport shows
/// `[top, top + height)`. The answer is where to draw (offset from the top of
/// the viewport), how many rows of the picture that rect covers, and which end
/// of the picture was cut off.
///
/// The picture is scaled once, to the size of the *whole* band (see [`fit_px`]),
/// and a partly visible band is drawn with `Resize::Crop`, which takes pixels
/// straight off that already-fitted picture. So the visible slice is exactly the
/// part of the picture that belongs on those rows — it scrolls rather than
/// growing, and the rows the band reserves never change.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct BandSlice {
    /// Rows from the top of the viewport to the first drawn row.
    pub offset: u16,
    /// Height of the drawn rect.
    pub rows: u16,
    /// The band runs off the top: keep the picture's bottom `rows` rows.
    pub clip_top: bool,
}

pub fn band_slice(start: usize, rows: u16, top: usize, height: u16) -> Option<BandSlice> {
    if rows == 0 || height == 0 {
        return None;
    }
    let end = start.checked_add(rows as usize)?;
    if end <= top || start >= top + height as usize {
        return None; // wholly off screen
    }
    if start >= top {
        // starts on screen; the bottom may run off the end of the viewport
        let offset = (start - top) as u16;
        let visible = (height - offset).min(rows);
        Some(BandSlice {
            offset,
            rows: visible,
            clip_top: false,
        })
    } else {
        // the top is scrolled off: show the bottom of the picture, flush with
        // the top of the viewport
        let hidden = (top - start) as u16;
        let visible = (rows - hidden).min(height);
        Some(BandSlice {
            offset: 0,
            rows: visible,
            clip_top: true,
        })
    }
}

pub fn resolve_in(url: &str, note_dir: &Path, attachments: &Path) -> Option<PathBuf> {
    if url.contains("://") {
        return None; // remote images are not fetched
    }
    let raw = PathBuf::from(shellexpand(url));
    let candidates = [
        raw.clone(),
        note_dir.join(&raw),
        note_dir.join("attachments").join(&raw),
        attachments.join(raw.file_name().unwrap_or(raw.as_os_str())),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// The pixel size that fills `area` cells as nearly as a picture's shape
/// allows, up or down, and the cells that size occupies. Unlike [`fit_px`]
/// this never keeps a small picture small: the point of the zoom is to see
/// it as large as the window can show it.
fn fill_px(w: u32, h: u32, font_w: u16, font_h: u16, area: (u16, u16)) -> ((u32, u32), (u16, u16)) {
    let font_w = font_w.max(1) as f32;
    let font_h = font_h.max(1) as f32;
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let (cols, rows) = (area.0.max(1) as f32, area.1.max(1) as f32);
    let scale = (cols * font_w / wf).min(rows * font_h / hf);
    let px = (
        (wf * scale).round().max(1.0) as u32,
        (hf * scale).round().max(1.0) as u32,
    );
    let size = (
        ((px.0 as f32 / font_w).ceil() as u16).clamp(1, area.0.max(1)),
        ((px.1 as f32 / font_h).ceil() as u16).clamp(1, area.1.max(1)),
    );
    (px, size)
}

/// Expand a leading `~/`; everything else is left alone.
fn shellexpand(url: &str) -> String {
    match url.strip_prefix("~/") {
        Some(rest) => std::env::home_dir()
            .map(|h| h.join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| url.to_string()),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_band_is_only_ever_clamped_by_the_viewport() {
        // the natural height stands whatever the scroll
        assert_eq!(band_rows(8, 30), 8);
        // taller than the page: it takes the page, and no more
        assert_eq!(band_rows(20, 6), 6);
        assert_eq!(band_rows(20, 0), 1);
    }

    fn slice(offset: u16, rows: u16, clip_top: bool) -> Option<BandSlice> {
        Some(BandSlice {
            offset,
            rows,
            clip_top,
        })
    }

    #[test]
    fn a_wholly_visible_band_is_drawn_at_its_full_height() {
        // page rows 10..18 of a 20-row viewport scrolled to 5
        assert_eq!(band_slice(10, 8, 5, 20), slice(5, 8, false));
        // exactly filling the viewport
        assert_eq!(band_slice(10, 8, 5, 13), slice(5, 8, false));
        // flush with the top of the viewport
        assert_eq!(band_slice(10, 8, 10, 8), slice(0, 8, false));
    }

    #[test]
    fn a_band_running_off_the_bottom_draws_the_rows_that_fit() {
        // rows 10..18, viewport 5..17: five rows of picture are on screen
        assert_eq!(band_slice(10, 8, 5, 12), slice(5, 7, false));
        // only its first row is on screen
        assert_eq!(band_slice(10, 8, 5, 6), slice(5, 1, false));
    }

    #[test]
    fn a_band_scrolled_off_the_top_draws_its_bottom_slice() {
        // scrolled one row past the band's start: seven rows left, at the top
        assert_eq!(band_slice(10, 8, 11, 20), slice(0, 7, true));
        // its very last row
        assert_eq!(band_slice(10, 8, 17, 20), slice(0, 1, true));
    }

    #[test]
    fn a_band_wholly_off_screen_is_not_drawn() {
        assert_eq!(band_slice(10, 8, 18, 20), None); // just past the end
        assert_eq!(band_slice(30, 8, 5, 20), None); // below the viewport
        assert_eq!(band_slice(10, 0, 5, 20), None);
        assert_eq!(band_slice(10, 8, 5, 0), None);
    }

    #[test]
    fn the_fitted_pixel_size_matches_the_rows_reserved() {
        // 1000x500 at 10x20 cells on a 20-column page: half size, five rows
        assert_eq!(fit_px(1000, 500, 10, 20, 20), ((200, 100), 5));
        // small enough to draw untouched
        assert_eq!(fit_px(100, 200, 10, 20, 20), ((100, 200), 10));
        // taller than MAX_ROWS: scaled again so the picture fits its band
        let ((w, h), rows) = fit_px(10, 10_000, 10, 20, 20);
        assert_eq!(rows, MAX_ROWS);
        assert!(h <= MAX_ROWS as u32 * 20, "{h} px in {MAX_ROWS} rows");
        assert!(w >= 1);
    }

    #[test]
    fn a_zoomed_picture_fills_the_window_on_its_long_side_and_is_scaled_up_if_small() {
        // 10×10 cells, 10×20 px each: a 200×100 picture is width-bound
        let (px, size) = fill_px(200, 100, 10, 20, (10, 10));
        assert_eq!(px, (100, 50));
        assert_eq!(size, (10, 3));
        // a tall one is height-bound
        let (px, size) = fill_px(100, 400, 10, 20, (10, 10));
        assert_eq!(px, (50, 200));
        assert_eq!(size, (5, 10));
        // a tiny one grows to fill, which the band fit never does
        let (px, _) = fill_px(20, 10, 10, 20, (10, 10));
        assert_eq!(px, (100, 50));
        assert_eq!(fit_px(20, 10, 10, 20, 10).0, (20, 10));
    }

    #[test]
    fn resolves_beside_the_note_and_in_attachments() {
        let dir = std::env::temp_dir().join("catcher-images-test");
        let att = dir.join("attachments");
        std::fs::create_dir_all(&att).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        std::fs::write(att.join("b.png"), b"x").unwrap();
        let images = Images::new(att.clone());
        assert_eq!(images.resolve("a.png", &dir), Some(dir.join("a.png")));
        assert_eq!(images.resolve("b.png", &dir), Some(att.join("b.png")));
        assert_eq!(images.resolve("missing.png", &dir), None);
        assert_eq!(images.resolve("https://x.y/z.png", &dir), None);

        // a note stored elsewhere still finds the configured attachments dir
        let elsewhere = dir.join("sub");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert_eq!(
            images.resolve("attachments/b.png", &elsewhere),
            Some(att.join("b.png"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
