use std::collections::HashMap;
use std::io::{self, Cursor, Write};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{DynamicImage, GenericImageView, imageops::FilterType};

// ── Image protocol detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageProtocol {
    Kitty,
    Iterm2,
    /// Universal fallback: render images using Unicode half-block characters (▀)
    /// with foreground/background colors. Works in any terminal with color support.
    HalfBlock,
}

pub fn detect_protocol() -> ImageProtocol {
    // Kitty checks first (more efficient: upload once, place per-frame)
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return ImageProtocol::Kitty;
    }
    if std::env::var("TERM").ok().as_deref() == Some("xterm-kitty") {
        return ImageProtocol::Kitty;
    }
    if let Ok(term) = std::env::var("TERM_PROGRAM") {
        match term.as_str() {
            "WezTerm" | "ghostty" => return ImageProtocol::Kitty,
            "iTerm.app" => return ImageProtocol::Iterm2,
            _ => {}
        }
    }
    if std::env::var("TERM").ok().as_deref() == Some("xterm-ghostty") {
        return ImageProtocol::Kitty;
    }
    // Konsole supports the Kitty graphics protocol since version 22.04
    if std::env::var("KONSOLE_VERSION").is_ok() {
        return ImageProtocol::Kitty;
    }
    if std::env::var("LC_TERMINAL").ok().as_deref() == Some("iTerm2") {
        return ImageProtocol::Iterm2;
    }
    ImageProtocol::HalfBlock
}

// ── Cell metrics ────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct CellMetrics {
    pub aspect: f64,
    pub cell_w_px: u32,
    pub cell_h_px: u32,
}

impl Default for CellMetrics {
    fn default() -> Self {
        CellMetrics {
            aspect: 2.0,
            cell_w_px: 8,
            cell_h_px: 16,
        }
    }
}

pub fn get_cell_metrics() -> CellMetrics {
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
                return CellMetrics {
                    aspect: cell_h / cell_w,
                    cell_w_px: cell_w.round() as u32,
                    cell_h_px: cell_h.round() as u32,
                };
            }
        }
    }
    CellMetrics::default()
}

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
    let scale_w = max_cols as f64 / img_w as f64;
    let scale_h = (max_rows as f64 * cell_aspect) / img_h as f64;
    let scale = scale_w.min(scale_h);

    let display_cols = (img_w as f64 * scale).round().max(1.0) as usize;
    let display_rows = (img_h as f64 * scale / cell_aspect).round().max(1.0) as usize;

    (display_cols.min(max_cols), display_rows.min(max_rows))
}

// ── PNG encoding helper ─────────────────────────────────────────────────────

fn encode_png(img: &DynamicImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("PNG encoding failed");
    bytes
}

// ── Kitty graphics protocol ─────────────────────────────────────────────────

/// Transmit image data to the terminal with an ID (no display). Uses q=2 to
/// suppress terminal responses.
fn transmit_kitty_image(stdout: &mut impl Write, png_data: &[u8], id: u32) -> io::Result<()> {
    let b64 = BASE64.encode(png_data);
    let chunk_size = 4096;
    let total_chunks = b64.len().div_ceil(chunk_size);

    for (i, chunk) in b64.as_bytes().chunks(chunk_size).enumerate() {
        let more = if i < total_chunks - 1 { 1 } else { 0 };
        if i == 0 {
            stdout.write_all(format!("\x1b_Ga=t,f=100,t=d,i={},q=2,m={};", id, more).as_bytes())?;
        } else {
            stdout.write_all(format!("\x1b_Gm={};", more).as_bytes())?;
        }
        stdout.write_all(chunk)?;
        stdout.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// Place an already-transmitted Kitty image (or a sub-rectangle of it).
fn place_kitty_image(
    stdout: &mut impl Write,
    id: u32,
    cols: usize,
    src_y: u32,
    src_w: u32,
    src_h: u32,
) -> io::Result<()> {
    write!(
        stdout,
        "\x1b_Ga=p,i={},q=2,x=0,y={},w={},h={},c={},r=1;\x1b\\",
        id, src_y, src_w, src_h, cols
    )?;
    Ok(())
}

/// Delete all Kitty image placements on screen.
pub fn kitty_delete_all(stdout: &mut impl Write) -> io::Result<()> {
    stdout.write_all(b"\x1b_Ga=d,d=a\x1b\\")?;
    Ok(())
}

// ── Image cache ─────────────────────────────────────────────────────────────

/// Default placeholder rows when image dimensions are unknown
pub const IMAGE_ROWS: usize = 8;

/// Maximum image rows to allow (all protocols)
const MAX_IMAGE_ROWS: usize = 20;

/// Max source dimension before downscaling
const MAX_SOURCE_DIM: u32 = 2000;

// ── Image cache ─────────────────────────────────────────────────────────────

/// Pre-computed Kitty image: uploaded once via `a=t`, placed per-frame via `a=p`.
struct KittyImage {
    id: u32,
    cols: usize,
    rows: usize,
    target_w: u32,
    target_h: u32,
    cell_h_px: u32,
    /// PNG data waiting to be transmitted; `None` once uploaded to terminal.
    pending_png: Option<Vec<u8>>,
}

/// Pre-rendered iTerm2 image: full image cached, crops computed on demand.
struct Iterm2Image {
    cols: usize,
    total_rows: usize,
    cell_h_px: u32,
    /// The resized image pixels (for cropping visible portions).
    resized: DynamicImage,
    /// Base64-encoded PNG of the full image.
    full_base64: String,
    /// Cached crop: (first_row, num_rows, base64_data).
    crop_cache: Option<(usize, usize, String)>,
}

/// Pre-rendered half-block image: uses Unicode ▀ with fg/bg colors to render
/// two vertical pixels per terminal cell. Works in any terminal.
struct HalfBlockImage {
    cols: usize,
    rows: usize,
    /// Image resized to cols × (rows * 2) pixels for half-block rendering.
    resized: DynamicImage,
}

pub struct ImageCache {
    images: HashMap<String, Option<DynamicImage>>,
    protocol: ImageProtocol,

    // Kitty: image uploaded once, placed per-frame
    kitty_images: HashMap<String, KittyImage>,
    next_kitty_id: u32,

    // iTerm2: pre-cropped strips cached per image
    iterm2_images: HashMap<String, Iterm2Image>,

    // Half-block: resized images for Unicode block rendering
    halfblock_images: HashMap<String, HalfBlockImage>,

    last_render_width: usize,
    cell_metrics: CellMetrics,
}

impl ImageCache {
    pub fn new() -> Self {
        let protocol = detect_protocol();
        ImageCache {
            images: HashMap::new(),
            protocol,
            kitty_images: HashMap::new(),
            next_kitty_id: 0,
            iterm2_images: HashMap::new(),
            halfblock_images: HashMap::new(),
            last_render_width: 0,
            cell_metrics: get_cell_metrics(),
        }
    }

    pub fn protocol(&self) -> ImageProtocol {
        self.protocol
    }

    pub fn update_cell_aspect(&mut self) {
        let new = get_cell_metrics();
        if (new.aspect - self.cell_metrics.aspect).abs() > 0.01
            || new.cell_w_px != self.cell_metrics.cell_w_px
            || new.cell_h_px != self.cell_metrics.cell_h_px
        {
            self.cell_metrics = new;
            self.kitty_images.clear();
            self.iterm2_images.clear();
            self.halfblock_images.clear();
        } else {
            self.cell_metrics = new;
        }
    }

    pub fn has_image(&self, url: &str) -> bool {
        self.images.get(url).is_some_and(|o| o.is_some())
    }

    /// Returns true if a fetch has already been attempted for this URL
    /// (regardless of whether it succeeded), so we don't re-queue it.
    pub fn has_attempted(&self, url: &str) -> bool {
        self.images.contains_key(url)
    }

    /// Insert a pre-loaded image directly (used in tests).
    #[cfg(test)]
    fn insert(&mut self, url: &str, img: Option<image::DynamicImage>) {
        self.images.insert(url.to_string(), img);
    }

    pub fn image_dimensions(&self, url: &str) -> Option<(u32, u32)> {
        self.images.get(url)?.as_ref().map(|img| img.dimensions())
    }

    pub fn display_size(
        &self,
        url: &str,
        max_cols: usize,
        max_rows: usize,
    ) -> Option<(usize, usize)> {
        let (w, h) = self.image_dimensions(url)?;
        Some(calc_display_cells(
            w,
            h,
            max_cols,
            max_rows,
            self.cell_metrics.aspect,
        ))
    }

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

    /// Pre-render images for the current protocol and content width.
    pub fn pre_render(&mut self, content_width: usize) {
        if content_width != self.last_render_width {
            self.kitty_images.clear();
            self.iterm2_images.clear();
            self.halfblock_images.clear();
            self.last_render_width = content_width;
        }

        let urls: Vec<String> = self
            .images
            .iter()
            .filter_map(|(url, opt)| opt.as_ref().map(|_| url.clone()))
            .collect();

        let cell_aspect = self.cell_metrics.aspect;
        let cell_w_px = self.cell_metrics.cell_w_px;
        let cell_h_px = self.cell_metrics.cell_h_px;

        for url in urls {
            let img = self.images.get(&url).unwrap().as_ref().unwrap();

            match self.protocol {
                ImageProtocol::Kitty => {
                    self.kitty_images.entry(url).or_insert_with(|| {
                        let (img_w, img_h) = img.dimensions();
                        let (cols, rows) = calc_display_cells(
                            img_w,
                            img_h,
                            content_width,
                            MAX_IMAGE_ROWS,
                            cell_aspect,
                        );
                        let target_w = (cols as u32 * cell_w_px).max(1);
                        let target_h = (rows as u32 * cell_h_px).max(1);
                        let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
                        let png = encode_png(&resized);
                        self.next_kitty_id += 1;
                        KittyImage {
                            id: self.next_kitty_id,
                            cols,
                            rows,
                            target_w,
                            target_h,
                            cell_h_px,
                            pending_png: Some(png),
                        }
                    });
                }
                ImageProtocol::Iterm2 => {
                    self.iterm2_images.entry(url).or_insert_with(|| {
                        let (img_w, img_h) = img.dimensions();
                        let (cols, rows) = calc_display_cells(
                            img_w,
                            img_h,
                            content_width,
                            MAX_IMAGE_ROWS,
                            cell_aspect,
                        );
                        let target_w = (cols as u32 * cell_w_px).max(1);
                        let target_h = (rows as u32 * cell_h_px).max(1);
                        let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
                        let full_base64 = BASE64.encode(encode_png(&resized));

                        Iterm2Image {
                            cols,
                            total_rows: rows,
                            cell_h_px,
                            resized,
                            full_base64,
                            crop_cache: None,
                        }
                    });
                }
                ImageProtocol::HalfBlock => {
                    self.halfblock_images.entry(url).or_insert_with(|| {
                        let (img_w, img_h) = img.dimensions();
                        let (cols, rows) = calc_display_cells(
                            img_w,
                            img_h,
                            content_width,
                            MAX_IMAGE_ROWS,
                            cell_aspect,
                        );
                        // Half-block: each cell = 1 column wide, 2 vertical pixels
                        let target_w = (cols as u32).max(1);
                        let target_h = (rows as u32 * 2).max(1);
                        let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
                        HalfBlockImage {
                            cols,
                            rows,
                            resized,
                        }
                    });
                }
            }
        }
    }

    /// Render a single image row. Returns true if the row was rendered inline.
    /// iTerm2 uses `render_iterm2_block` instead (called separately in a second pass).
    pub fn render_image_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        content_width: usize,
        bg: crossterm::style::Color,
    ) -> io::Result<bool> {
        match self.protocol {
            ImageProtocol::Kitty => self.render_kitty_row(stdout, url, image_row, content_width),
            ImageProtocol::HalfBlock => {
                self.render_halfblock_row(stdout, url, image_row, content_width, bg)
            }
            ImageProtocol::Iterm2 => Ok(false),
        }
    }

    /// Transmit any Kitty images that haven't been uploaded to the terminal yet.
    /// Call this once per frame, before placing images.
    pub fn transmit_pending_kitty(&mut self, stdout: &mut impl Write) -> io::Result<()> {
        for ki in self.kitty_images.values_mut() {
            if let Some(png_data) = ki.pending_png.take() {
                transmit_kitty_image(stdout, &png_data, ki.id)?;
            }
        }
        Ok(())
    }

    fn render_kitty_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        content_width: usize,
    ) -> io::Result<bool> {
        let ki = match self.kitty_images.get(url) {
            Some(ki) => ki,
            None => return Ok(false),
        };
        if image_row >= ki.rows {
            return Ok(false);
        }

        let x_offset = content_width.saturating_sub(ki.cols) / 2;
        if x_offset > 0 {
            write!(stdout, "{}", " ".repeat(x_offset))?;
        }
        // Place a sub-rectangle of the already-uploaded image
        let src_y = image_row as u32 * ki.cell_h_px;
        let src_h = ki.cell_h_px.min(ki.target_h.saturating_sub(src_y)).max(1);
        place_kitty_image(stdout, ki.id, ki.cols, src_y, ki.target_w, src_h)?;
        // Kitty doesn't advance cursor — write spaces to fill the content width
        write!(stdout, "{}", " ".repeat(content_width - x_offset))?;
        Ok(true)
    }

    fn render_halfblock_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        content_width: usize,
        bg: crossterm::style::Color,
    ) -> io::Result<bool> {
        let hb = match self.halfblock_images.get(url) {
            Some(hb) => hb,
            None => return Ok(false),
        };
        if image_row >= hb.rows {
            return Ok(false);
        }

        let bg_rgb = color_to_rgb(bg);
        let x_offset = content_width.saturating_sub(hb.cols) / 2;
        if x_offset > 0 {
            write!(stdout, "{}", " ".repeat(x_offset))?;
        }

        let top_y = (image_row * 2) as u32;
        let bot_y = top_y + 1;

        for col in 0..hb.cols as u32 {
            let tp = hb.resized.get_pixel(col, top_y);
            let (tr, tg, tb) = blend_alpha(tp, bg_rgb);

            let (br, bkg, bb) = if bot_y < hb.resized.height() {
                let bp = hb.resized.get_pixel(col, bot_y);
                blend_alpha(bp, bg_rgb)
            } else {
                bg_rgb
            };

            write!(
                stdout,
                "\x1b[38;2;{};{};{};48;2;{};{};{}m\u{2580}",
                tr, tg, tb, br, bkg, bb
            )?;
        }

        // Restore background color and fill remaining space
        write!(
            stdout,
            "\x1b[0m\x1b[48;2;{};{};{}m",
            bg_rgb.0, bg_rgb.1, bg_rgb.2
        )?;
        let used = x_offset + hb.cols;
        if used < content_width {
            write!(stdout, "{}", " ".repeat(content_width - used))?;
        }

        Ok(true)
    }

    /// Render a visible portion of an iTerm2 image as a single inline image.
    /// `first_row`/`num_rows` describe which rows of the image are visible;
    /// `screen_y` is the 0-based terminal row for the first visible image row.
    pub fn render_iterm2_block(
        &mut self,
        stdout: &mut impl Write,
        url: &str,
        first_row: usize,
        num_rows: usize,
        content_width: usize,
        screen_y: u16,
    ) -> io::Result<()> {
        let ii = match self.iterm2_images.get_mut(url) {
            Some(ii) => ii,
            None => return Ok(()),
        };

        let x_col = 2 + content_width.saturating_sub(ii.cols) / 2;

        // Pick the right base64 payload: full image or a cached crop
        let data: &str = if first_row == 0 && num_rows == ii.total_rows {
            &ii.full_base64
        } else {
            if !ii
                .crop_cache
                .as_ref()
                .is_some_and(|(fr, nr, _)| *fr == first_row && *nr == num_rows)
            {
                let y = first_row as u32 * ii.cell_h_px;
                let h = (num_rows as u32 * ii.cell_h_px)
                    .min(ii.resized.height().saturating_sub(y))
                    .max(1);
                let cropped = ii.resized.crop_imm(0, y, ii.resized.width(), h);
                ii.crop_cache = Some((first_row, num_rows, BASE64.encode(encode_png(&cropped))));
            }
            &ii.crop_cache.as_ref().unwrap().2
        };

        // Position cursor and emit a single iTerm2 inline image
        write!(stdout, "\x1b[{};{}H", screen_y + 1, x_col + 1)?; // 1-based ANSI coords
        write!(
            stdout,
            "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=0:{}\x07",
            ii.cols, num_rows, data
        )?;

        Ok(())
    }
}

// ── Half-block helpers ──────────────────────────────────────────────────────

fn color_to_rgb(c: crossterm::style::Color) -> (u8, u8, u8) {
    match c {
        crossterm::style::Color::Rgb { r, g, b } => (r, g, b),
        _ => (0, 0, 0),
    }
}

fn blend_alpha(pixel: image::Rgba<u8>, bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let a = pixel[3] as f32 / 255.0;
    if a >= 1.0 {
        return (pixel[0], pixel[1], pixel[2]);
    }
    let r = (pixel[0] as f32 * a + bg.0 as f32 * (1.0 - a)) as u8;
    let g = (pixel[1] as f32 * a + bg.1 as f32 * (1.0 - a)) as u8;
    let b = (pixel[2] as f32 * a + bg.2 as f32 * (1.0 - a)) as u8;
    (r, g, b)
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
    img.resize(new_w, new_h, FilterType::Lanczos3)
}

fn fetch_image(url: &str) -> Option<DynamicImage> {
    if url.starts_with("http://") || url.starts_with("https://") {
        fetch_image_http(url)
    } else {
        // Only allow relative paths and paths under the current directory;
        // reject absolute paths to prevent reading arbitrary local files.
        let path = std::path::Path::new(url);
        if path.is_absolute() {
            return None;
        }
        // Reject paths that escape the working directory via ".."
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return None;
            }
        }
        image::open(url).ok()
    }
}

fn fetch_image_http(url: &str) -> Option<DynamicImage> {
    // Block requests to private/loopback/link-local/metadata IPs to prevent SSRF
    if let Some(host) = extract_host(url) {
        let blocked = [
            "localhost",
            "127.0.0.1",
            "::1",
            "[::1]",
            "0.0.0.0",
            "169.254.169.254",
            "metadata.google.internal",
        ];
        let host_lower = host.to_lowercase();
        if blocked.iter().any(|b| host_lower == *b)
            || host_lower.starts_with("10.")
            || host_lower.starts_with("192.168.")
            || host_lower.starts_with("172.16.")
            || host_lower.starts_with("172.17.")
            || host_lower.starts_with("172.18.")
            || host_lower.starts_with("172.19.")
            || host_lower.starts_with("172.2")
            || host_lower.starts_with("172.30.")
            || host_lower.starts_with("172.31.")
        {
            return None;
        }
    }
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

/// Extract the host portion from an HTTP(S) URL.
fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url
        .strip_prefix("https://")
        .or(url.strip_prefix("http://"))?;
    let authority = after_scheme.split('/').next()?;
    // Strip optional userinfo (user:pass@)
    let host_port = authority.rsplit('@').next()?;
    // Strip port
    Some(if host_port.starts_with('[') {
        // IPv6: [::1]:port
        host_port.split(']').next().map(|s| &s[1..])?
    } else {
        host_port.split(':').next()?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── has_attempted / has_image ────────────────────────────────────────────

    #[test]
    fn has_attempted_false_for_unknown_url() {
        let cache = ImageCache::new();
        assert!(!cache.has_attempted("http://example.com/img.png"));
    }

    #[test]
    fn has_attempted_true_after_failed_fetch() {
        // A None entry means the fetch ran but produced no image.
        let mut cache = ImageCache::new();
        cache.insert("http://example.com/img.png", None);
        assert!(cache.has_attempted("http://example.com/img.png"));
    }

    #[test]
    fn has_attempted_true_after_successful_fetch() {
        let mut cache = ImageCache::new();
        let img = image::DynamicImage::new_rgb8(4, 4);
        cache.insert("http://example.com/img.png", Some(img));
        assert!(cache.has_attempted("http://example.com/img.png"));
    }

    #[test]
    fn has_image_false_for_unknown_url() {
        let cache = ImageCache::new();
        assert!(!cache.has_image("http://example.com/img.png"));
    }

    #[test]
    fn has_image_false_after_failed_fetch() {
        let mut cache = ImageCache::new();
        cache.insert("http://example.com/img.png", None);
        // Attempted but failed — has_image must stay false.
        assert!(!cache.has_image("http://example.com/img.png"));
    }

    #[test]
    fn has_image_true_after_successful_fetch() {
        let mut cache = ImageCache::new();
        let img = image::DynamicImage::new_rgb8(4, 4);
        cache.insert("http://example.com/img.png", Some(img));
        assert!(cache.has_image("http://example.com/img.png"));
    }

    #[test]
    fn has_attempted_and_has_image_are_independent() {
        // has_attempted subsumes has_image: any URL where has_image is true
        // must also satisfy has_attempted, but not vice-versa.
        let mut cache = ImageCache::new();
        let img = image::DynamicImage::new_rgb8(1, 1);
        cache.insert("ok", Some(img));
        cache.insert("fail", None);

        assert!(cache.has_attempted("ok"));
        assert!(cache.has_image("ok"));

        assert!(cache.has_attempted("fail"));
        assert!(!cache.has_image("fail"));
    }

    // ── fetch_if_missing idempotency ─────────────────────────────────────────

    #[test]
    fn fetch_if_missing_does_not_overwrite_existing_entry() {
        // If a URL is already in the cache (even as None), fetch_if_missing
        // must leave it untouched — it must not issue a second fetch.
        let mut cache = ImageCache::new();
        cache.insert("local_nonexistent.png", None);
        cache.fetch_if_missing("local_nonexistent.png");
        // Still None — was not replaced by a fresh (failed) attempt.
        assert!(!cache.has_image("local_nonexistent.png"));
        assert!(cache.has_attempted("local_nonexistent.png"));
    }

    // ── extract_host ─────────────────────────────────────────────────────────

    #[test]
    fn extract_host_https() {
        assert_eq!(
            extract_host("https://example.com/path"),
            Some("example.com")
        );
    }

    #[test]
    fn extract_host_http_with_port() {
        assert_eq!(
            extract_host("http://example.com:8080/path"),
            Some("example.com")
        );
    }

    #[test]
    fn extract_host_strips_userinfo() {
        assert_eq!(
            extract_host("https://user:pass@example.com/path"),
            Some("example.com")
        );
    }

    #[test]
    fn extract_host_ipv6() {
        assert_eq!(extract_host("http://[::1]/path"), Some("::1"));
    }

    #[test]
    fn extract_host_returns_none_for_non_http() {
        assert_eq!(extract_host("ftp://example.com/file"), None);
    }
}
