use std::collections::HashMap;
use std::io::{self, Write};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor};
use image::{DynamicImage, GenericImageView, RgbaImage, imageops::FilterType};

// ── Protocol detection ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ImageProtocol {
    Kitty,
    ITerm2,
    HalfBlock,
}

pub fn detect_protocol() -> ImageProtocol {
    // Kitty and Ghostty support the Kitty graphics protocol
    if std::env::var("KITTY_WINDOW_ID").is_ok()
        || std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || std::env::var("TERM")
            .map(|t| t.contains("kitty") || t.contains("ghostty"))
            .unwrap_or(false)
    {
        return ImageProtocol::Kitty;
    }
    // iTerm2 and WezTerm support the iTerm2 inline image protocol
    if let Ok(prog) = std::env::var("TERM_PROGRAM")
        && (prog == "iTerm.app" || prog == "WezTerm")
    {
        return ImageProtocol::ITerm2;
    }
    ImageProtocol::HalfBlock
}

// ── Cell aspect ratio detection ─────────────────────────────────────────────

/// Get the terminal cell aspect ratio (cell_height / cell_width).
/// Uses TIOCGWINSZ ioctl to get pixel dimensions, falls back to 2.0.
pub fn get_cell_aspect_ratio() -> f64 {
    #[cfg(unix)]
    {
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
                && ws.ws_xpixel > 0
                && ws.ws_ypixel > 0
                && ws.ws_col > 0
                && ws.ws_row > 0
            {
                let cell_w = ws.ws_xpixel as f64 / ws.ws_col as f64;
                let cell_h = ws.ws_ypixel as f64 / ws.ws_row as f64;
                return cell_h / cell_w;
            }
        }
    }
    2.0 // Default: most terminal fonts have ~2:1 cell aspect ratio
}

/// Calculate display cell dimensions (cols, rows) that preserve the image's
/// aspect ratio within the given bounds, accounting for terminal cell shape.
fn calc_display_cells(
    img_w: u32,
    img_h: u32,
    max_cols: usize,
    max_rows: usize,
    cell_aspect: f64,
) -> (usize, usize) {
    if img_w == 0 || img_h == 0 || max_cols == 0 || max_rows == 0 {
        return (1, 1);
    }
    // In a common unit (cell_width = 1):
    //   available width  = max_cols
    //   available height = max_rows * cell_aspect
    // Scale image to fit, then convert back to cell dimensions.
    let scale_w = max_cols as f64 / img_w as f64;
    let scale_h = (max_rows as f64 * cell_aspect) / img_h as f64;
    let scale = scale_w.min(scale_h);

    let display_cols = (img_w as f64 * scale).round().max(1.0) as usize;
    let display_rows = (img_h as f64 * scale / cell_aspect).round().max(1.0) as usize;

    (display_cols.min(max_cols), display_rows.min(max_rows))
}

// ── Image cache ─────────────────────────────────────────────────────────────

/// Maximum pixel dimension for source images (downscaled on fetch)
const MAX_SOURCE_DIM: u32 = 800;

/// Default placeholder rows when image dimensions are unknown
pub const IMAGE_ROWS: usize = 12;

/// Maximum image rows to allow
pub const MAX_IMAGE_ROWS: usize = 20;

struct EncodedPng {
    base64: String,
    png_size: usize,
}

pub struct ImageCache {
    images: HashMap<String, Option<DynamicImage>>,
    protocol: ImageProtocol,
    /// Pre-encoded base64 PNG data per url (Kitty/iTerm2)
    encoded: HashMap<String, EncodedPng>,
    /// Pre-resized RGBA pixel data per url (HalfBlock)
    resized: HashMap<String, RgbaImage>,
    /// Content width used for last pre_render (to detect resize)
    last_render_width: usize,
    /// Terminal cell aspect ratio (cell_height / cell_width)
    cell_aspect: f64,
    /// Kitty protocol: mapping from URL to image ID (for transmit-once, place-many)
    kitty_ids: HashMap<String, u32>,
    /// Kitty protocol: next image ID to assign
    next_kitty_id: u32,
}

impl ImageCache {
    pub fn new() -> Self {
        ImageCache {
            images: HashMap::new(),
            protocol: detect_protocol(),
            encoded: HashMap::new(),
            resized: HashMap::new(),
            last_render_width: 0,
            cell_aspect: get_cell_aspect_ratio(),
            kitty_ids: HashMap::new(),
            next_kitty_id: 0,
        }
    }

    #[allow(dead_code)]
    pub fn protocol(&self) -> ImageProtocol {
        self.protocol
    }

    /// Refresh the cached cell aspect ratio (call on terminal resize).
    pub fn update_cell_aspect(&mut self) {
        let new = get_cell_aspect_ratio();
        if (new - self.cell_aspect).abs() > 0.01 {
            self.cell_aspect = new;
            // Invalidate HalfBlock cache since pixel scaling depends on aspect
            self.resized.clear();
        }
    }

    pub fn has_image(&self, url: &str) -> bool {
        self.images.get(url).is_some_and(|o| o.is_some())
    }

    /// Get the pixel dimensions of a cached image.
    pub fn image_dimensions(&self, url: &str) -> Option<(u32, u32)> {
        self.images.get(url)?.as_ref().map(|img| img.dimensions())
    }

    /// Calculate display cell dimensions (cols, rows) that preserve
    /// the image's aspect ratio within the given bounds.
    pub fn display_size(&self, url: &str, max_cols: usize, max_rows: usize) -> Option<(usize, usize)> {
        let (w, h) = self.image_dimensions(url)?;
        Some(calc_display_cells(w, h, max_cols, max_rows, self.cell_aspect))
    }

    /// Calculate the ideal number of rows for an image at the given width.
    pub fn ideal_rows(&self, url: &str, content_width: usize) -> Option<usize> {
        let (_, rows) = self.display_size(url, content_width, MAX_IMAGE_ROWS)?;
        Some(rows)
    }

    pub fn fetch_if_missing(&mut self, url: &str) {
        if self.images.contains_key(url) {
            return;
        }
        let img = fetch_image(url).map(|img| downscale(img, MAX_SOURCE_DIM));
        self.images.insert(url.to_string(), img);
    }

    /// Pre-encode/resize all fetched images for the current display dimensions.
    /// Call after fetching images or on terminal resize.
    pub fn pre_render(&mut self, content_width: usize) {
        // On width change, invalidate halfblock cache (Kitty/iTerm2 cache is size-independent)
        if content_width != self.last_render_width {
            self.resized.clear();
            self.last_render_width = content_width;
        }

        let urls: Vec<String> = self
            .images
            .iter()
            .filter_map(|(url, opt)| opt.as_ref().map(|_| url.clone()))
            .collect();

        let cell_aspect = self.cell_aspect;

        for url in urls {
            let img = self.images.get(&url).unwrap().as_ref().unwrap();
            match self.protocol {
                ImageProtocol::Kitty | ImageProtocol::ITerm2 => {
                    if let std::collections::hash_map::Entry::Vacant(e) = self.encoded.entry(url)
                        && let Ok(png_data) = encode_png(img)
                    {
                        let base64 = BASE64.encode(&png_data);
                        e.insert(EncodedPng {
                            png_size: png_data.len(),
                            base64,
                        });
                    }
                }
                ImageProtocol::HalfBlock => {
                    self.resized.entry(url).or_insert_with(|| {
                        let (img_w, img_h) = img.dimensions();
                        let max_half_rows = (MAX_IMAGE_ROWS * 2) as f64;
                        let max_cols = content_width as f64;

                        // Account for cell aspect ratio:
                        // Each half-block pixel is 1 col wide × 0.5 rows tall.
                        // On-screen aspect of one pixel: cell_w / (cell_h/2) = 2/cell_aspect.
                        // Target resize ratio to preserve image aspect:
                        //   new_w/new_h = (img_w/img_h) * cell_aspect / 2
                        let target_ratio = (img_w as f64 * cell_aspect) / (img_h as f64 * 2.0);

                        let h_if_w = max_cols / target_ratio;
                        let w_if_h = max_half_rows * target_ratio;

                        let (new_w, new_h) = if h_if_w <= max_half_rows {
                            // Width-constrained
                            (max_cols.round() as u32, h_if_w.round().max(1.0) as u32)
                        } else {
                            // Height-constrained
                            (w_if_h.round().max(1.0) as u32, max_half_rows as u32)
                        };

                        img.resize_exact(new_w.max(1), new_h.max(1), FilterType::Triangle)
                            .to_rgba8()
                    });
                }
            }
        }
    }

    /// For Kitty protocol: transmit any images not yet sent to the terminal.
    /// Images are stored with an ID so subsequent frames only need small
    /// placement commands instead of re-sending full image data.
    pub fn ensure_kitty_transmitted(&mut self, stdout: &mut impl Write) -> io::Result<()> {
        if self.protocol != ImageProtocol::Kitty {
            return Ok(());
        }
        let urls: Vec<String> = self
            .encoded
            .keys()
            .filter(|url| !self.kitty_ids.contains_key(*url))
            .cloned()
            .collect();
        for url in urls {
            let enc = &self.encoded[&url];
            self.next_kitty_id += 1;
            let id = self.next_kitty_id;
            transmit_kitty_image(stdout, &enc.base64, id)?;
            self.kitty_ids.insert(url, id);
        }
        Ok(())
    }

    /// For Kitty protocol: delete all visible image placements.
    /// Image data remains in memory (keyed by ID) for re-placement.
    pub fn clear_kitty_placements(&self, stdout: &mut impl Write) -> io::Result<()> {
        if self.protocol == ImageProtocol::Kitty {
            write!(stdout, "\x1b_Ga=d,d=a,q=2\x1b\\")?;
        }
        Ok(())
    }

    /// Render a cached image to stdout at the given position.
    /// The image is rendered within the available area, preserving aspect ratio
    /// and centered horizontally.
    #[allow(clippy::too_many_arguments)]
    pub fn render_to(
        &self,
        stdout: &mut impl Write,
        url: &str,
        x: u16,
        y: u16,
        available_width: usize,
        available_height: usize,
        bg: Color,
    ) -> io::Result<()> {
        match self.protocol {
            ImageProtocol::Kitty => {
                if let Some(&id) = self.kitty_ids.get(url) {
                    let (cols, rows) = self
                        .display_size(url, available_width, available_height)
                        .unwrap_or((available_width, available_height));
                    let x_off = (available_width.saturating_sub(cols)) / 2;
                    render_kitty_placement(
                        stdout,
                        id,
                        x + x_off as u16,
                        y,
                        cols,
                        rows,
                    )?;
                }
            }
            ImageProtocol::ITerm2 => {
                if let Some(enc) = self.encoded.get(url) {
                    let (cols, rows) = self
                        .display_size(url, available_width, available_height)
                        .unwrap_or((available_width, available_height));
                    let x_off = (available_width.saturating_sub(cols)) / 2;
                    render_iterm2_cached(
                        stdout,
                        &enc.base64,
                        enc.png_size,
                        x + x_off as u16,
                        y,
                        cols,
                        rows,
                    )?;
                }
            }
            ImageProtocol::HalfBlock => {
                if let Some(resized) = self.resized.get(url) {
                    render_halfblock_cached(
                        stdout,
                        resized,
                        x,
                        y,
                        available_width,
                        available_height,
                        bg,
                    )?;
                }
            }
        }
        Ok(())
    }
}

// ── Fetching ────────────────────────────────────────────────────────────────

fn downscale(img: DynamicImage, max_dim: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= max_dim && h <= max_dim {
        return img;
    }
    let scale = max_dim as f64 / w.max(h) as f64;
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    img.resize(new_w, new_h, FilterType::Triangle)
}

fn fetch_image(url: &str) -> Option<DynamicImage> {
    if url.starts_with("http://") || url.starts_with("https://") {
        fetch_image_http(url)
    } else {
        image::open(url).ok()
    }
}

fn fetch_image_http(url: &str) -> Option<DynamicImage> {
    let output = std::process::Command::new("curl")
        .args(["-sL", "--max-time", "10", "--max-filesize", "10485760", url])
        .output()
        .ok()?;
    if output.status.success() && !output.stdout.is_empty() {
        image::load_from_memory(&output.stdout).ok()
    } else {
        None
    }
}

// ── Encoding ────────────────────────────────────────────────────────────────

fn encode_png(img: &DynamicImage) -> io::Result<Vec<u8>> {
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    let rgba = img.to_rgba8();
    let mut png_data = Vec::new();
    PngEncoder::new(&mut png_data)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(io::Error::other)?;
    Ok(png_data)
}

// ── Cached rendering ────────────────────────────────────────────────────────

/// Transmit image data to the terminal with a Kitty image ID (no display).
fn transmit_kitty_image(stdout: &mut impl Write, b64: &str, id: u32) -> io::Result<()> {
    let chunk_size = 4096;
    let b64_bytes = b64.as_bytes();

    if b64_bytes.len() <= chunk_size {
        write!(
            stdout,
            "\x1b_Gf=100,a=t,t=d,i={},q=2;{}\x1b\\",
            id, b64
        )?;
    } else {
        let mut offset = 0;
        let mut first = true;
        while offset < b64_bytes.len() {
            let end = (offset + chunk_size).min(b64_bytes.len());
            let chunk = std::str::from_utf8(&b64_bytes[offset..end]).unwrap_or("");
            let more = if end < b64_bytes.len() { 1 } else { 0 };

            if first {
                write!(
                    stdout,
                    "\x1b_Gf=100,a=t,t=d,i={},q=2,m={};{}\x1b\\",
                    id, more, chunk
                )?;
                first = false;
            } else {
                write!(stdout, "\x1b_Gm={};{}\x1b\\", more, chunk)?;
            }
            offset = end;
        }
    }

    Ok(())
}

/// Place a previously transmitted Kitty image at a screen position.
fn render_kitty_placement(
    stdout: &mut impl Write,
    id: u32,
    x: u16,
    y: u16,
    cols: usize,
    rows: usize,
) -> io::Result<()> {
    queue!(stdout, MoveTo(x, y))?;
    write!(
        stdout,
        "\x1b_Ga=p,i={},c={},r={},q=2\x1b\\",
        id, cols, rows
    )?;
    Ok(())
}

fn render_iterm2_cached(
    stdout: &mut impl Write,
    b64: &str,
    png_size: usize,
    x: u16,
    y: u16,
    width: usize,
    height: usize,
) -> io::Result<()> {
    queue!(stdout, MoveTo(x, y))?;
    write!(
        stdout,
        "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=1:{}\x07",
        png_size, width, height, b64
    )?;

    Ok(())
}

fn render_halfblock_cached(
    stdout: &mut impl Write,
    resized: &RgbaImage,
    x: u16,
    y: u16,
    width: usize,
    height: usize,
    bg: Color,
) -> io::Result<()> {
    let new_w = resized.width() as usize;
    let new_h = resized.height() as usize;

    // Center the image horizontally
    let x_offset = width.saturating_sub(new_w) / 2;

    let (bg_r, bg_g, bg_b) = match bg {
        Color::Rgb { r, g, b } => (r, g, b),
        _ => (30, 30, 46),
    };

    for row in 0..height {
        queue!(stdout, MoveTo(x, y + row as u16))?;
        let py_top = (row * 2) as u32;
        let py_bot = (row * 2 + 1) as u32;

        for col in 0..width {
            // Map display column to image pixel column (centered)
            let in_image = col >= x_offset && col < x_offset + new_w;
            let cx = if in_image {
                (col - x_offset) as u32
            } else {
                0
            };

            let (tr, tg, tb) = if in_image && py_top < new_h as u32 {
                let p = resized.get_pixel(cx, py_top);
                let a = p[3] as f64 / 255.0;
                (
                    (p[0] as f64 * a + bg_r as f64 * (1.0 - a)) as u8,
                    (p[1] as f64 * a + bg_g as f64 * (1.0 - a)) as u8,
                    (p[2] as f64 * a + bg_b as f64 * (1.0 - a)) as u8,
                )
            } else {
                (bg_r, bg_g, bg_b)
            };

            let (br, bg_c, bb) = if in_image && py_bot < new_h as u32 {
                let p = resized.get_pixel(cx, py_bot);
                let a = p[3] as f64 / 255.0;
                (
                    (p[0] as f64 * a + bg_r as f64 * (1.0 - a)) as u8,
                    (p[1] as f64 * a + bg_g as f64 * (1.0 - a)) as u8,
                    (p[2] as f64 * a + bg_b as f64 * (1.0 - a)) as u8,
                )
            } else {
                (bg_r, bg_g, bg_b)
            };

            queue!(
                stdout,
                SetForegroundColor(Color::Rgb {
                    r: tr,
                    g: tg,
                    b: tb,
                }),
                SetBackgroundColor(Color::Rgb {
                    r: br,
                    g: bg_c,
                    b: bb,
                }),
                Print("▀"),
            )?;
        }
        queue!(stdout, SetAttribute(Attribute::Reset))?;
    }

    Ok(())
}
