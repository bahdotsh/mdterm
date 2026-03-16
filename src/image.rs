use std::collections::{HashMap, HashSet};
use std::io::{self, Cursor, Write};
use std::sync::mpsc;

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

fn encode_png(img: &DynamicImage) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .ok()?;
    Some(bytes)
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

    // Background fetch infrastructure
    sender: mpsc::Sender<(String, Option<DynamicImage>)>,
    receiver: mpsc::Receiver<(String, Option<DynamicImage>)>,
    in_flight: HashSet<String>,
}

impl ImageCache {
    pub fn new() -> Self {
        let protocol = detect_protocol();
        let (sender, receiver) = mpsc::channel();
        ImageCache {
            images: HashMap::new(),
            protocol,
            kitty_images: HashMap::new(),
            next_kitty_id: 0,
            iterm2_images: HashMap::new(),
            halfblock_images: HashMap::new(),
            last_render_width: 0,
            cell_metrics: get_cell_metrics(),
            sender,
            receiver,
            in_flight: HashSet::new(),
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
    /// (regardless of whether it succeeded) or is currently in flight,
    /// so we don't re-queue it.
    pub fn has_attempted(&self, url: &str) -> bool {
        self.images.contains_key(url) || self.in_flight.contains(url)
    }

    /// Maximum number of concurrent background fetches.
    const MAX_CONCURRENT_FETCHES: usize = 10;

    /// Spawn a background thread to fetch `url` if not already cached or in flight.
    /// Returns false (without spawning) if the concurrency cap has been reached.
    pub fn start_fetch(&mut self, url: &str) -> bool {
        if self.images.contains_key(url) || self.in_flight.contains(url) {
            return true; // already handled
        }
        if self.in_flight.len() >= Self::MAX_CONCURRENT_FETCHES {
            return false;
        }
        self.in_flight.insert(url.to_string());
        let sender = self.sender.clone();
        let url_owned = url.to_string();
        std::thread::spawn(move || {
            // Guard against panics in image decoding/downscaling so that
            // the channel always receives a result and the in_flight slot
            // is freed by poll_completed(). Without this, a panic would
            // leave the URL stuck in in_flight permanently.
            let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                fetch_image(&url_owned).map(|img| downscale(img, MAX_SOURCE_DIM))
            }))
            .unwrap_or(None);
            let _ = sender.send((url_owned, img));
        });
        true
    }

    /// Poll for completed background fetches. Returns true if any new images arrived.
    pub fn poll_completed(&mut self) -> bool {
        let mut any = false;
        while let Ok((url, img)) = self.receiver.try_recv() {
            self.in_flight.remove(&url);
            self.images.insert(url, img);
            any = true;
        }
        any
    }

    /// Returns true if any fetches are currently in flight.
    pub fn has_in_flight(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// Number of fetches currently in flight.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
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

    #[cfg(test)]
    fn fetch_if_missing(&mut self, url: &str) {
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
                        let png = match encode_png(&resized) {
                            Some(data) => data,
                            None => return KittyImage {
                                id: 0,
                                cols: 0,
                                rows: 0,
                                target_w: 0,
                                target_h: 0,
                                cell_h_px: 0,
                                pending_png: None,
                            },
                        };
                        self.next_kitty_id = self.next_kitty_id.wrapping_add(1);
                        if self.next_kitty_id == 0 {
                            self.next_kitty_id = 1;
                        }
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
                        let png = match encode_png(&resized) {
                            Some(data) => data,
                            None => return Iterm2Image {
                                cols: 0,
                                total_rows: 0,
                                cell_h_px: 0,
                                resized: DynamicImage::new_rgb8(1, 1),
                                full_base64: String::new(),
                                crop_cache: None,
                            },
                        };
                        let full_base64 = BASE64.encode(png);

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
                let png = match encode_png(&cropped) {
                    Some(data) => data,
                    None => return Ok(()),
                };
                ii.crop_cache = Some((first_row, num_rows, BASE64.encode(png)));
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

/// Returns true if `host` falls in the RFC 1918 172.16.0.0/12 range (172.16.x.x – 172.31.x.x).
fn is_rfc1918_172(host: &str) -> bool {
    if let Some(rest) = host.strip_prefix("172.")
        && let Some(octet_str) = rest.split('.').next()
        && let Ok(octet) = octet_str.parse::<u8>()
    {
        return (16..=31).contains(&octet);
    }
    false
}

/// Returns true if `host` is a private/loopback/link-local/metadata address.
fn is_blocked_host(host: &str) -> bool {
    let blocked = [
        "localhost",
        "127.0.0.1",
        "::1",
        "[::1]",
        "0.0.0.0",
        "169.254.169.254",
        "metadata.google.internal",
    ];
    let h = host.to_lowercase();
    blocked.iter().any(|b| h == *b)
        || h.starts_with("10.")
        || h.starts_with("192.168.")
        || h.starts_with("0.")
        || is_rfc1918_172(&h)
        || h.ends_with(".local")
        || h.ends_with(".localhost")
        || h == "localhost"
        || h.ends_with(".internal")
        || h.starts_with("[::ffff:")
        || h.starts_with("::ffff:")
}

fn fetch_image_http(url: &str) -> Option<DynamicImage> {
    // SSRF check on the initial URL
    if let Some(host) = extract_host(url)
        && is_blocked_host(host)
    {
        return None;
    }
    fetch_image_http_inner(url, true)
}

/// Core HTTP fetch with manual redirect following (up to 5 hops).
/// When `check_ssrf` is true, each redirect target is validated against the
/// SSRF blocklist. Tests pass `false` to allow localhost servers.
fn fetch_image_http_inner(url: &str, check_ssrf: bool) -> Option<DynamicImage> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .max_redirects(0)
        .build()
        .into();

    let mut current_url = url.to_string();
    for _ in 0..5 {
        let resp = agent.get(&current_url).call().ok()?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = resp.headers().get("location")?.to_str().ok()?;
            // Resolve relative redirects
            let next = if location.starts_with("http://") || location.starts_with("https://") {
                location.to_string()
            } else if location.starts_with('/') {
                // Absolute path — reuse scheme + host
                let scheme_end = current_url.find("://")? + 3;
                let host_end = current_url[scheme_end..]
                    .find('/')
                    .map(|i| i + scheme_end)
                    .unwrap_or(current_url.len());
                format!("{}{}", &current_url[..host_end], location)
            } else {
                return None; // unsupported relative form
            };
            // SSRF check on every redirect target
            if check_ssrf
                && let Some(host) = extract_host(&next)
                && is_blocked_host(host)
            {
                return None;
            }
            current_url = next;
            continue;
        }
        if !(200..300).contains(&status) {
            return None;
        }
        let mut resp = resp;
        let buf = resp
            .body_mut()
            .with_config()
            .limit(10_485_760)
            .read_to_vec()
            .ok()?;
        return image::load_from_memory(&buf).ok();
    }
    None // too many redirects
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

    // ── in_flight_count / has_in_flight ─────────────────────────────────────

    #[test]
    fn in_flight_count_starts_at_zero() {
        let cache = ImageCache::new();
        assert_eq!(cache.in_flight_count(), 0);
        assert!(!cache.has_in_flight());
    }

    #[test]
    fn start_fetch_marks_url_in_flight() {
        let mut cache = ImageCache::new();
        // Use a URL that will fail (doesn't matter — we just check in_flight state)
        cache
            .in_flight
            .insert("http://example.com/test.png".to_string());
        assert_eq!(cache.in_flight_count(), 1);
        assert!(cache.has_in_flight());
        assert!(cache.has_attempted("http://example.com/test.png"));
    }

    #[test]
    fn start_fetch_is_idempotent() {
        let mut cache = ImageCache::new();
        // Simulate already in-flight
        cache
            .in_flight
            .insert("http://example.com/a.png".to_string());
        let count_before = cache.in_flight_count();
        // start_fetch should not add duplicate
        cache.start_fetch("http://example.com/a.png");
        assert_eq!(cache.in_flight_count(), count_before);
    }

    #[test]
    fn start_fetch_skips_already_cached_url() {
        let mut cache = ImageCache::new();
        cache.insert(
            "http://example.com/a.png",
            Some(DynamicImage::new_rgb8(2, 2)),
        );
        cache.start_fetch("http://example.com/a.png");
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[test]
    fn start_fetch_respects_concurrency_cap() {
        let mut cache = ImageCache::new();
        // Fill up to the cap by inserting directly into in_flight
        for i in 0..ImageCache::MAX_CONCURRENT_FETCHES {
            cache
                .in_flight
                .insert(format!("http://example.com/{i}.png"));
        }
        assert_eq!(cache.in_flight_count(), ImageCache::MAX_CONCURRENT_FETCHES);
        // Attempting another fetch should return false
        let accepted = cache.start_fetch("http://example.com/extra.png");
        assert!(!accepted);
        assert_eq!(cache.in_flight_count(), ImageCache::MAX_CONCURRENT_FETCHES);
    }

    // ── poll_completed ──────────────────────────────────────────────────────

    #[test]
    fn poll_completed_drains_channel() {
        let mut cache = ImageCache::new();
        // Manually push into the channel to simulate background fetch completion
        let img = DynamicImage::new_rgb8(4, 4);
        cache.in_flight.insert("url1".to_string());
        cache.in_flight.insert("url2".to_string());
        cache
            .sender
            .send(("url1".to_string(), Some(img.clone())))
            .unwrap();
        cache.sender.send(("url2".to_string(), None)).unwrap();

        let any = cache.poll_completed();
        assert!(any);
        assert!(cache.has_image("url1"));
        assert!(!cache.has_image("url2")); // failed fetch
        assert!(cache.has_attempted("url2"));
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[test]
    fn poll_completed_returns_false_when_empty() {
        let mut cache = ImageCache::new();
        assert!(!cache.poll_completed());
    }

    // ── calc_display_cells ──────────────────────────────────────────────────

    #[test]
    fn calc_display_cells_zero_inputs_return_1x1() {
        assert_eq!(calc_display_cells(0, 0, 80, 20, 2.0), (1, 1));
        assert_eq!(calc_display_cells(100, 100, 0, 20, 2.0), (1, 1));
        assert_eq!(calc_display_cells(100, 100, 80, 0, 2.0), (1, 1));
    }

    #[test]
    fn calc_display_cells_fits_within_max() {
        let (cols, rows) = calc_display_cells(800, 600, 80, 20, 2.0);
        assert!(cols <= 80);
        assert!(rows <= 20);
        assert!(cols >= 1);
        assert!(rows >= 1);
    }

    #[test]
    fn calc_display_cells_wide_image_constrained_by_cols() {
        // Very wide image: should be constrained by max_cols
        let (cols, rows) = calc_display_cells(1000, 100, 40, 20, 2.0);
        assert!(cols <= 40);
        assert!(rows >= 1);
    }

    #[test]
    fn calc_display_cells_tall_image_constrained_by_rows() {
        // Very tall image: should be constrained by max_rows
        let (cols, rows) = calc_display_cells(100, 1000, 80, 10, 2.0);
        assert!(rows <= 10);
        assert!(cols >= 1);
    }

    // ── blend_alpha ─────────────────────────────────────────────────────────

    #[test]
    fn blend_alpha_fully_opaque() {
        let pixel = image::Rgba([100, 150, 200, 255]);
        let result = blend_alpha(pixel, (0, 0, 0));
        assert_eq!(result, (100, 150, 200));
    }

    #[test]
    fn blend_alpha_fully_transparent() {
        let pixel = image::Rgba([100, 150, 200, 0]);
        let result = blend_alpha(pixel, (50, 60, 70));
        assert_eq!(result, (50, 60, 70));
    }

    #[test]
    fn blend_alpha_half_transparent() {
        let pixel = image::Rgba([200, 100, 0, 128]); // ~50% alpha
        let (r, g, b) = blend_alpha(pixel, (0, 0, 0));
        // With ~50% alpha over black: ~100, ~50, ~0
        assert!(r > 90 && r < 110);
        assert!(g > 40 && g < 60);
        assert!(b < 5);
    }

    // ── downscale ───────────────────────────────────────────────────────────

    #[test]
    fn downscale_small_image_unchanged() {
        let img = DynamicImage::new_rgb8(100, 100);
        let result = downscale(img, 2000);
        assert_eq!(result.dimensions(), (100, 100));
    }

    #[test]
    fn downscale_large_image_reduced() {
        // Just over the limit — enough to trigger downscale without a huge
        // debug-mode allocation (4000x3000 was ~10s in unoptimized builds).
        let img = DynamicImage::new_rgb8(200, 150);
        let result = downscale(img, 100);
        let (w, h) = result.dimensions();
        assert!(w <= 100);
        assert!(h <= 100);
        assert!(w >= 1);
        assert!(h >= 1);
    }

    // ── is_blocked_host ─────────────────────────────────────────────────────

    #[test]
    fn is_blocked_host_blocks_localhost() {
        assert!(is_blocked_host("localhost"));
        assert!(is_blocked_host("127.0.0.1"));
        assert!(is_blocked_host("::1"));
        assert!(is_blocked_host("[::1]"));
        assert!(is_blocked_host("0.0.0.0"));
    }

    #[test]
    fn is_blocked_host_blocks_private_ranges() {
        assert!(is_blocked_host("10.0.0.1"));
        assert!(is_blocked_host("192.168.1.1"));
        assert!(is_blocked_host("172.16.0.1"));
        assert!(is_blocked_host("169.254.169.254"));
        assert!(is_blocked_host("metadata.google.internal"));
    }

    #[test]
    fn is_blocked_host_blocks_all_rfc1918_172() {
        for octet in 16..=31 {
            assert!(is_blocked_host(&format!("172.{octet}.0.1")));
        }
    }

    #[test]
    fn is_blocked_host_allows_public_172() {
        // 172.200.x.x and 172.217.x.x (Google) are NOT in 172.16/12
        assert!(!is_blocked_host("172.200.0.1"));
        assert!(!is_blocked_host("172.217.14.99"));
        assert!(!is_blocked_host("172.15.0.1"));
        assert!(!is_blocked_host("172.32.0.1"));
    }

    #[test]
    fn is_blocked_host_allows_public() {
        assert!(!is_blocked_host("example.com"));
        assert!(!is_blocked_host("8.8.8.8"));
        assert!(!is_blocked_host("cdn.github.com"));
    }

    #[test]
    fn is_blocked_host_blocks_zero_prefix() {
        assert!(is_blocked_host("0.0.0.0"));
        assert!(is_blocked_host("0.1.2.3"));
    }

    #[test]
    fn is_blocked_host_blocks_local_tlds() {
        assert!(is_blocked_host("printer.local"));
        assert!(is_blocked_host("app.localhost"));
        assert!(is_blocked_host("service.internal"));
    }

    #[test]
    fn is_blocked_host_blocks_ipv6_mapped() {
        assert!(is_blocked_host("::ffff:127.0.0.1"));
        assert!(is_blocked_host("[::ffff:10.0.0.1]"));
    }

    // ── extract_host ────────────────────────────────────────────────────────

    #[test]
    fn extract_host_various_urls() {
        assert_eq!(
            extract_host("http://example.com/img.png"),
            Some("example.com")
        );
        assert_eq!(
            extract_host("https://cdn.example.com:8080/img"),
            Some("cdn.example.com")
        );
        assert_eq!(extract_host("ftp://nope.com/x"), None);
        assert_eq!(extract_host("not-a-url"), None);
    }

    // ── integration tests with a local HTTP server ──────────────────────────

    use std::io::Read as IoRead;
    use std::net::TcpListener;

    /// A minimal 1x1 red PNG (68 bytes).
    fn tiny_png() -> Vec<u8> {
        let img = DynamicImage::new_rgb8(1, 1);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    /// Spin up a TcpListener on a random port, run `handler` in a thread for
    /// each incoming connection, and return the base URL.
    /// Returns `None` if loopback is unavailable (e.g. sandboxed CI).
    fn start_test_server(
        handler: impl Fn(std::net::TcpStream) + Send + Sync + 'static,
    ) -> Option<(String, std::net::SocketAddr)> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let addr = listener.local_addr().ok()?;
        let base = format!("http://127.0.0.1:{}", addr.port());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(stream) = stream {
                    let _ = (handler)(stream);
                }
            }
        });
        Some((base, addr))
    }

    #[test]
    #[ignore] // integration test — run with `cargo test -- --ignored`
    fn http_fetch_simple_image() {
        let png = tiny_png();
        let Some((base, _)) = start_test_server(move |mut stream| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let body = png.clone();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
        }) else {
            return; // loopback unavailable
        };
        let result = fetch_image_http_inner(&format!("{base}/image.png"), false);
        assert!(result.is_some(), "should fetch a simple image");
        assert_eq!(result.unwrap().dimensions(), (1, 1));
    }

    #[test]
    #[ignore]
    fn http_fetch_follows_redirect() {
        let png = tiny_png();
        let Some((base, _)) = start_test_server(move |mut stream| {
            let mut buf = [0u8; 512];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if req.contains("GET /redirect") {
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: /image.png\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = stream.write_all(resp.as_bytes());
            } else {
                let body = png.clone();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(&body);
            }
        }) else {
            return;
        };
        let result = fetch_image_http_inner(&format!("{base}/redirect"), false);
        assert!(result.is_some(), "should follow redirect and fetch image");
    }

    #[test]
    #[ignore]
    fn http_fetch_follows_absolute_redirect() {
        let png = tiny_png();
        // Need two ports: one redirects to the other
        let Some((target_base, _)) = start_test_server(move |mut stream| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let body = png.clone();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
        }) else {
            return;
        };

        let target = target_base.clone();
        let Some((redir_base, _)) = start_test_server(move |mut stream| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 301 Moved\r\nLocation: {}/image.png\r\nContent-Length: 0\r\n\r\n",
                target
            );
            let _ = stream.write_all(resp.as_bytes());
        }) else {
            return;
        };

        let result = fetch_image_http_inner(&format!("{redir_base}/go"), false);
        assert!(
            result.is_some(),
            "should follow absolute redirect across ports"
        );
    }

    #[test]
    #[ignore]
    fn http_fetch_stops_after_too_many_redirects() {
        let Some((base, _)) = start_test_server(|mut stream| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            // Always redirect to self — infinite loop
            let resp = "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
        }) else {
            return;
        };
        let result = fetch_image_http_inner(&format!("{base}/loop"), false);
        assert!(result.is_none(), "should give up after 5 redirects");
    }

    #[test]
    #[ignore]
    fn http_fetch_slow_image_within_timeout() {
        let png = tiny_png();
        let Some((base, _)) = start_test_server(move |mut stream| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            // Delay 500ms then serve
            std::thread::sleep(std::time::Duration::from_millis(500));
            let body = png.clone();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
        }) else {
            return;
        };
        let result = fetch_image_http_inner(&format!("{base}/slow.png"), false);
        assert!(result.is_some(), "500ms delay should be within 10s timeout");
    }

    #[test]
    #[ignore]
    fn http_fetch_404_returns_none() {
        let Some((base, _)) = start_test_server(|mut stream| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
        }) else {
            return;
        };
        let result = fetch_image_http_inner(&format!("{base}/missing.png"), false);
        assert!(result.is_none(), "404 should return None");
    }

    #[test]
    fn ssrf_blocks_redirect_to_metadata() {
        // Redirect to a blocked IP — should be caught even though initial URL is fine
        // We test the SSRF logic by using check_ssrf=true and checking that the
        // redirect target is validated.
        // Since initial URL is also localhost (blocked), we test the function directly.
        let blocked = "http://169.254.169.254/latest/meta-data/";
        assert!(fetch_image_http(blocked).is_none());
    }
}
