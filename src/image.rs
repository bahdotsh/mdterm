use std::collections::HashMap;
use std::io::{self, Cursor, Write};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use crossterm::style::Color;
use image::{DynamicImage, GenericImageView, RgbaImage, imageops::FilterType};

// ── Image protocol detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageProtocol {
    Kitty,
    ITerm2,
    HalfBlock,
}

pub fn detect_protocol() -> ImageProtocol {
    // Kitty terminal
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return ImageProtocol::Kitty;
    }
    if std::env::var("TERM").ok().as_deref() == Some("xterm-kitty") {
        return ImageProtocol::Kitty;
    }
    // Check TERM_PROGRAM for various terminals
    if let Ok(term) = std::env::var("TERM_PROGRAM") {
        match term.as_str() {
            "WezTerm" | "ghostty" => return ImageProtocol::Kitty,
            "iTerm.app" => return ImageProtocol::ITerm2,
            _ => {}
        }
    }
    // Ghostty via TERM
    if std::env::var("TERM").ok().as_deref() == Some("xterm-ghostty") {
        return ImageProtocol::Kitty;
    }
    // iTerm2 via LC_TERMINAL
    if std::env::var("LC_TERMINAL").ok().as_deref() == Some("iTerm2") {
        return ImageProtocol::ITerm2;
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
fn transmit_kitty_image(
    stdout: &mut impl Write,
    png_data: &[u8],
    id: u32,
) -> io::Result<()> {
    let b64 = BASE64.encode(png_data);
    let chunk_size = 4096;
    let total_chunks = b64.len().div_ceil(chunk_size);

    for (i, chunk) in b64.as_bytes().chunks(chunk_size).enumerate() {
        let more = if i < total_chunks - 1 { 1 } else { 0 };
        if i == 0 {
            stdout.write_all(
                format!(
                    "\x1b_Ga=t,f=100,t=d,i={},q=2,m={};",
                    id, more
                )
                .as_bytes(),
            )?;
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

// ── iTerm2 inline image protocol ────────────────────────────────────────────

fn encode_iterm2_strip(png_data: &[u8], cols: usize) -> Vec<u8> {
    let b64 = BASE64.encode(png_data);
    format!(
        "\x1b]1337;File=inline=1;width={};height=1;preserveAspectRatio=0:{}\x07",
        cols, b64
    )
    .into_bytes()
}

// ── Image cache ─────────────────────────────────────────────────────────────

/// Default placeholder rows when image dimensions are unknown
pub const IMAGE_ROWS: usize = 8;

/// Maximum image rows to allow
pub const MAX_IMAGE_ROWS: usize = 20;

/// Maximum columns for image display
const MAX_IMAGE_COLS: usize = 80;

/// Max source dimension for halfblock (low-res fallback)
const MAX_SOURCE_DIM_HALFBLOCK: u32 = 800;

/// Max source dimension for Kitty/iTerm2 (higher quality)
const MAX_SOURCE_DIM_NATIVE: u32 = 2000;

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

pub struct ImageCache {
    images: HashMap<String, Option<DynamicImage>>,
    protocol: ImageProtocol,

    // HalfBlock: pre-resized RGBA pixel data
    resized: HashMap<String, RgbaImage>,

    // iTerm2: pre-encoded escape sequences per URL (one per display row)
    iterm2_strips: HashMap<String, Vec<Vec<u8>>>,

    // Kitty: image uploaded once, placed per-frame
    kitty_images: HashMap<String, KittyImage>,
    next_kitty_id: u32,

    last_render_width: usize,
    cell_metrics: CellMetrics,
}

impl ImageCache {
    pub fn new() -> Self {
        let protocol = detect_protocol();
        ImageCache {
            images: HashMap::new(),
            protocol,
            resized: HashMap::new(),
            iterm2_strips: HashMap::new(),
            kitty_images: HashMap::new(),
            next_kitty_id: 0,
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
            self.resized.clear();
            self.iterm2_strips.clear();
            self.kitty_images.clear();
        } else {
            self.cell_metrics = new;
        }
    }

    pub fn has_image(&self, url: &str) -> bool {
        self.images.get(url).is_some_and(|o| o.is_some())
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
        let capped_cols = max_cols.min(MAX_IMAGE_COLS);
        Some(calc_display_cells(
            w,
            h,
            capped_cols,
            max_rows,
            self.cell_metrics.aspect,
        ))
    }

    pub fn ideal_rows(&self, url: &str, content_width: usize) -> Option<usize> {
        let capped_width = content_width.min(MAX_IMAGE_COLS);
        let (_, rows) = self.display_size(url, capped_width, MAX_IMAGE_ROWS)?;
        Some(rows)
    }

    pub fn fetch_if_missing(&mut self, url: &str) {
        if self.images.contains_key(url) {
            return;
        }
        let max_dim = match self.protocol {
            ImageProtocol::HalfBlock => MAX_SOURCE_DIM_HALFBLOCK,
            _ => MAX_SOURCE_DIM_NATIVE,
        };
        let img = fetch_image(url).map(|img| downscale(img, max_dim));
        self.images.insert(url.to_string(), img);
    }

    /// Pre-render images for the current protocol and content width.
    pub fn pre_render(&mut self, content_width: usize) {
        if content_width != self.last_render_width {
            self.resized.clear();
            self.iterm2_strips.clear();
            self.kitty_images.clear();
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
                        let capped_cols = content_width.min(MAX_IMAGE_COLS);
                        let (cols, rows) = calc_display_cells(
                            img_w, img_h, capped_cols, MAX_IMAGE_ROWS, cell_aspect,
                        );
                        let target_w = (cols as u32 * cell_w_px).max(1);
                        let target_h = (rows as u32 * cell_h_px).max(1);
                        let resized =
                            img.resize_exact(target_w, target_h, FilterType::Lanczos3);
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

                ImageProtocol::ITerm2 => {
                    self.iterm2_strips.entry(url).or_insert_with(|| {
                        let (img_w, img_h) = img.dimensions();
                        let capped_cols = content_width.min(MAX_IMAGE_COLS);
                        let (cols, rows) = calc_display_cells(
                            img_w, img_h, capped_cols, MAX_IMAGE_ROWS, cell_aspect,
                        );
                        let target_w = (cols as u32 * cell_w_px).max(1);
                        let target_h = (rows as u32 * cell_h_px).max(1);
                        let resized =
                            img.resize_exact(target_w, target_h, FilterType::Lanczos3);
                        let strip_h = cell_h_px;
                        let mut strips = Vec::with_capacity(rows);
                        for r in 0..rows {
                            let y = r as u32 * strip_h;
                            let h = strip_h.min(target_h.saturating_sub(y)).max(1);
                            let strip = resized.crop_imm(0, y, target_w, h);
                            let png = encode_png(&strip);
                            strips.push(encode_iterm2_strip(&png, cols));
                        }
                        strips
                    });
                }

                ImageProtocol::HalfBlock => {
                    self.resized.entry(url).or_insert_with(|| {
                        let (img_w, img_h) = img.dimensions();
                        let max_half_rows = (MAX_IMAGE_ROWS * 2) as f64;
                        let max_cols = content_width.min(MAX_IMAGE_COLS) as f64;

                        let target_ratio =
                            (img_w as f64 * cell_aspect) / (img_h as f64 * 2.0);

                        let h_if_w = max_cols / target_ratio;
                        let w_if_h = max_half_rows * target_ratio;

                        let (new_w, new_h) = if h_if_w <= max_half_rows {
                            (max_cols.round() as u32, h_if_w.round().max(1.0) as u32)
                        } else {
                            (w_if_h.round().max(1.0) as u32, max_half_rows as u32)
                        };

                        img.resize_exact(new_w.max(1), new_h.max(1), FilterType::Lanczos3)
                            .to_rgba8()
                    });
                }
            }
        }
    }

    /// Render a single image row. Returns true if the row was rendered.
    pub fn render_image_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        content_width: usize,
        bg: Color,
    ) -> io::Result<bool> {
        match self.protocol {
            ImageProtocol::Kitty => {
                self.render_kitty_row(stdout, url, image_row, content_width)
            }
            ImageProtocol::ITerm2 => {
                self.render_iterm2_row(stdout, url, image_row, content_width)
            }
            ImageProtocol::HalfBlock => {
                self.render_halfblock_row(stdout, url, image_row, content_width, bg)
            }
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

    fn render_iterm2_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        content_width: usize,
    ) -> io::Result<bool> {
        let strips = match self.iterm2_strips.get(url) {
            Some(s) => s,
            None => return Ok(false),
        };
        let strip_seq = match strips.get(image_row) {
            Some(d) => d,
            None => return Ok(false),
        };
        let (cols, _rows) = match self.display_size(url, content_width, MAX_IMAGE_ROWS) {
            Some(s) => s,
            None => return Ok(false),
        };

        let x_offset = content_width.saturating_sub(cols) / 2;
        if x_offset > 0 {
            write!(stdout, "{}", " ".repeat(x_offset))?;
        }
        // Write pre-encoded escape sequence directly
        stdout.write_all(strip_seq)?;
        let filled = x_offset + cols;
        if filled < content_width {
            write!(stdout, "{}", " ".repeat(content_width - filled))?;
        }
        Ok(true)
    }

    fn render_halfblock_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        available_width: usize,
        bg: Color,
    ) -> io::Result<bool> {
        let resized = match self.resized.get(url) {
            Some(r) => r,
            None => return Ok(false),
        };

        let new_w = resized.width() as usize;
        let new_h = resized.height() as usize;
        let x_offset = available_width.saturating_sub(new_w) / 2;

        let (bg_r, bg_g, bg_b) = match bg {
            Color::Rgb { r, g, b } => (r, g, b),
            _ => (30, 30, 46),
        };

        let py_top = (image_row * 2) as u32;
        let py_bot = (image_row * 2 + 1) as u32;

        use std::fmt::Write as FmtWrite;
        let mut buf = String::with_capacity(available_width * 30);
        let mut cur_fg: (u8, u8, u8) = (0, 0, 0);
        let mut cur_bg_c: (u8, u8, u8) = (0, 0, 0);
        let mut first = true;

        for col in 0..available_width {
            let in_image = col >= x_offset && col < x_offset + new_w;
            let cx = if in_image {
                (col - x_offset) as u32
            } else {
                0
            };

            let fg = if in_image && py_top < new_h as u32 {
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

            let bg_col = if in_image && py_bot < new_h as u32 {
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

            if first || fg != cur_fg {
                let _ = write!(buf, "\x1b[38;2;{};{};{}m", fg.0, fg.1, fg.2);
                cur_fg = fg;
            }
            if first || bg_col != cur_bg_c {
                let _ = write!(buf, "\x1b[48;2;{};{};{}m", bg_col.0, bg_col.1, bg_col.2);
                cur_bg_c = bg_col;
            }
            first = false;
            buf.push('▀');
        }
        stdout.write_all(buf.as_bytes())?;
        Ok(true)
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
    img.resize(new_w, new_h, FilterType::Lanczos3)
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
