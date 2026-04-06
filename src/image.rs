use std::collections::{HashMap, HashSet};
use std::io::{self, Cursor, Write};
use std::sync::{Arc, Mutex, mpsc};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::{DynamicImage, GenericImageView, imageops::FilterType};

// ── Image protocol detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageProtocol {
    Kitty,
    /// Kitty protocol with Unicode placeholders — works through tmux by uploading
    /// image data via DCS passthrough and using U+10EEEE placeholder characters
    /// for placement instead of direct Kitty placement commands.
    KittyUnicode,
    Iterm2,
    Sixel,
    /// Enlightenment [Terminology] terminal: file-path-based inline images.
    /// Detected via `TERMINOLOGY=1` env var when not inside tmux.
    ///
    /// [Terminology]: https://www.enlightenment.org/about-terminology
    Terminology,
    /// Universal fallback: render images using Unicode half-block characters (▀)
    /// with foreground/background colors. Works in any terminal with color support.
    HalfBlock,
}

pub fn detect_protocol() -> ImageProtocol {
    // Allow users to force a specific protocol via environment variable
    if let Ok(proto) = std::env::var("MDTERM_IMAGE_PROTOCOL") {
        match proto.to_lowercase().as_str() {
            "kitty" => return ImageProtocol::Kitty,
            "kittyunicode" | "kitty-unicode" | "kitty_unicode" => {
                return ImageProtocol::KittyUnicode;
            }
            "iterm2" => return ImageProtocol::Iterm2,
            "sixel" => return ImageProtocol::Sixel,
            "terminology" => return ImageProtocol::Terminology,
            "halfblock" => return ImageProtocol::HalfBlock,
            _ => {}
        }
    }

    // When inside tmux, standard Kitty placement commands don't work.
    // Use Unicode placeholder method instead: upload via DCS passthrough,
    // place via U+10EEEE characters that tmux treats as normal text.
    let in_tmux = std::env::var("TMUX").is_ok();
    if in_tmux {
        if let Ok(term) = std::env::var("TERM_PROGRAM") {
            match term.as_str() {
                "ghostty" | "WezTerm" => return ImageProtocol::KittyUnicode,
                "iTerm.app" => return ImageProtocol::Iterm2,
                _ => {}
            }
        }
        if let Ok(term) = std::env::var("TERM")
            && (term == "xterm-ghostty" || term == "xterm-kitty")
        {
            return ImageProtocol::KittyUnicode;
        }
        if std::env::var("KITTY_WINDOW_ID").is_ok() {
            return ImageProtocol::KittyUnicode;
        }
        if std::env::var("KONSOLE_VERSION").is_ok() {
            return ImageProtocol::KittyUnicode;
        }
    }

    // Kitty checks (more efficient: upload once, place per-frame)
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
            "foot" | "mlterm" | "mintty" | "contour" => return ImageProtocol::Sixel,
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
    // Additional Sixel-capable terminal detection
    if let Ok(term) = std::env::var("TERM")
        && (term == "foot" || term == "foot-extra" || term.starts_with("mlterm"))
    {
        return ImageProtocol::Sixel;
    }
    if std::env::var("MLTERM").is_ok() {
        return ImageProtocol::Sixel;
    }
    // Terminology: file-path-based protocol. Must not be inside tmux.
    if std::env::var("TERMINOLOGY").is_ok() && std::env::var("TMUX").is_err() {
        return ImageProtocol::Terminology;
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

// ── Kitty Unicode placeholder protocol (for tmux) ─────────────────────────

/// Combining diacritics used for row/column encoding in the Kitty Unicode
/// placeholder method. These are combining class 230 characters from Unicode
/// with no decomposition mappings.
/// Source: <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>
const DIACRITICS: [char; 256] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
    '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}',
    '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}',
    '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}',
    '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}',
    '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}',
    '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}',
    '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}',
];

/// Wrap a Kitty graphics escape sequence in tmux DCS passthrough.
/// Inner `\x1b` bytes are doubled so tmux forwards them correctly.
fn tmux_wrap(kitty_escape: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(kitty_escape.len() * 2 + 20);
    out.extend_from_slice(b"\x1bPtmux;");
    for &byte in kitty_escape {
        if byte == 0x1b {
            out.push(0x1b);
        }
        out.push(byte);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Transmit image data via tmux DCS passthrough to the outer terminal.
fn transmit_kitty_image_tmux(stdout: &mut impl Write, png_data: &[u8], id: u32) -> io::Result<()> {
    let b64 = BASE64.encode(png_data);
    let chunk_size = 4096;
    let total_chunks = b64.len().div_ceil(chunk_size);

    for (i, chunk) in b64.as_bytes().chunks(chunk_size).enumerate() {
        let more = if i < total_chunks - 1 { 1 } else { 0 };
        let mut kitty_seq = Vec::new();
        if i == 0 {
            kitty_seq.extend_from_slice(
                format!("\x1b_Ga=t,f=100,t=d,i={},q=2,m={};", id, more).as_bytes(),
            );
        } else {
            kitty_seq.extend_from_slice(format!("\x1b_Gm={};", more).as_bytes());
        }
        kitty_seq.extend_from_slice(chunk);
        kitty_seq.extend_from_slice(b"\x1b\\");
        stdout.write_all(&tmux_wrap(&kitty_seq))?;
    }
    Ok(())
}

/// Create a virtual placement (U=1) for Unicode placeholder rendering via tmux.
fn create_virtual_placement_tmux(
    stdout: &mut impl Write,
    id: u32,
    cols: usize,
    rows: usize,
) -> io::Result<()> {
    let kitty_seq = format!("\x1b_Ga=p,U=1,i={},c={},r={},q=2;\x1b\\", id, cols, rows);
    stdout.write_all(&tmux_wrap(kitty_seq.as_bytes()))
}

/// Delete all Kitty image placements via tmux DCS passthrough.
pub fn kitty_unicode_delete_all(stdout: &mut impl Write) -> io::Result<()> {
    let kitty_seq = b"\x1b_Ga=d,d=a\x1b\\";
    stdout.write_all(&tmux_wrap(kitty_seq))
}

// ── Sixel graphics protocol ─────────────────────────────────────────────────

/// Encode a `DynamicImage` as a Sixel escape sequence.
/// Alpha is blended against `bg`.
fn encode_sixel(img: &DynamicImage, bg: (u8, u8, u8)) -> String {
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return String::new();
    }

    // Convert to RGB pixels, blending alpha against the background color
    let rgba = img.to_rgba8();
    let mut rgb_pixels: Vec<(u8, u8, u8)> = Vec::with_capacity((width * height) as usize);
    for pixel in rgba.pixels() {
        let a = pixel[3] as f32 / 255.0;
        let inv = 1.0 - a;
        rgb_pixels.push((
            (pixel[0] as f32 * a + bg.0 as f32 * inv) as u8,
            (pixel[1] as f32 * a + bg.1 as f32 * inv) as u8,
            (pixel[2] as f32 * a + bg.2 as f32 * inv) as u8,
        ));
    }

    let w = width as usize;
    let h = height as usize;

    let (palette, indexed) = sixel_quantize(&rgb_pixels);
    let padded_h = h.div_ceil(6) * 6;

    let mut out = String::with_capacity(w * padded_h);

    // DCS header (P2=0: color 0 maps to terminal background)
    out.push_str("\x1bP0;0;0q");

    // Raster attributes: aspect 1:1, width, height
    out.push_str(&format!("\"1;1;{};{}", width, height));

    // Define color registers (RGB percentages 0-100)
    for (i, &(r, g, b)) in palette.iter().enumerate() {
        out.push_str(&format!(
            "#{};2;{};{};{}",
            i,
            (r as u32 * 100 + 127) / 255,
            (g as u32 * 100 + 127) / 255,
            (b as u32 * 100 + 127) / 255,
        ));
    }

    // Encode pixel data in 6-row-tall bands
    let num_bands = padded_h / 6;
    for band in 0..num_bands {
        let band_y = band * 6;

        // Determine which palette colors appear in this band
        let mut color_present = [false; 256];
        for dy in 0..6 {
            let y = band_y + dy;
            if y < h {
                for x in 0..w {
                    color_present[indexed[y * w + x]] = true;
                }
            }
        }
        let colors_in_band: Vec<usize> = (0..palette.len()).filter(|&i| color_present[i]).collect();

        for (ci_idx, &color_idx) in colors_in_band.iter().enumerate() {
            out.push_str(&format!("#{}", color_idx));

            // Build sixel characters for this color across the band
            let mut sixels: Vec<u8> = Vec::with_capacity(w);
            for x in 0..w {
                let mut val: u8 = 0;
                for dy in 0..6u8 {
                    let y = band_y + dy as usize;
                    if y < h && indexed[y * w + x] == color_idx {
                        val |= 1 << dy;
                    }
                }
                sixels.push(val + 0x3F);
            }

            sixel_rle(&sixels, &mut out);

            // Graphics carriage return between colors in the same band
            if ci_idx < colors_in_band.len() - 1 {
                out.push('$');
            }
        }

        // Graphics new line between bands
        if band + 1 < num_bands {
            out.push('-');
        }
    }

    // String Terminator
    out.push_str("\x1b\\");
    out
}

/// Run-length encode a slice of sixel characters into `out`.
fn sixel_rle(data: &[u8], out: &mut String) {
    let mut i = 0;
    while i < data.len() {
        let ch = data[i] as char;
        let mut count = 1;
        while i + count < data.len() && data[i + count] == data[i] {
            count += 1;
        }
        if count >= 3 {
            out.push_str(&format!("!{}{}", count, ch));
        } else {
            for _ in 0..count {
                out.push(ch);
            }
        }
        i += count;
    }
}

/// Quantize RGB pixels to at most 256 colours using median-cut.
/// Returns (palette, per-pixel palette indices).
fn sixel_quantize(pixels: &[(u8, u8, u8)]) -> (Vec<(u8, u8, u8)>, Vec<usize>) {
    let max_colors: usize = 256;

    // Count unique colours
    let mut counts: HashMap<(u8, u8, u8), u32> = HashMap::new();
    for &c in pixels {
        *counts.entry(c).or_insert(0) += 1;
    }

    let palette = if counts.len() <= max_colors {
        counts.keys().copied().collect()
    } else {
        sixel_median_cut(&counts, max_colors)
    };

    // Map each pixel to the nearest palette entry (cached)
    let mut cache: HashMap<(u8, u8, u8), usize> = HashMap::new();
    let indexed: Vec<usize> = pixels
        .iter()
        .map(|&c| {
            *cache.entry(c).or_insert_with(|| {
                palette
                    .iter()
                    .enumerate()
                    .min_by_key(|&(_, &(pr, pg, pb))| {
                        let dr = c.0 as i32 - pr as i32;
                        let dg = c.1 as i32 - pg as i32;
                        let db = c.2 as i32 - pb as i32;
                        dr * dr + dg * dg + db * db
                    })
                    .unwrap()
                    .0
            })
        })
        .collect();

    (palette, indexed)
}

/// Median-cut colour quantization on deduplicated colour counts.
#[allow(clippy::type_complexity)]
fn sixel_median_cut(counts: &HashMap<(u8, u8, u8), u32>, max_colors: usize) -> Vec<(u8, u8, u8)> {
    let initial: Vec<((u8, u8, u8), u32)> = counts.iter().map(|(&c, &n)| (c, n)).collect();
    let mut boxes: Vec<Vec<((u8, u8, u8), u32)>> = vec![initial];

    while boxes.len() < max_colors {
        // Find the box with the greatest range in any channel
        let split_idx = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .max_by_key(|(_, b)| {
                let (r, g, bl) = sixel_color_ranges(b);
                r.max(g).max(bl) as u32
            })
            .map(|(i, _)| i);

        let Some(split_idx) = split_idx else { break };

        let mut box_to_split = boxes.swap_remove(split_idx);
        let (r_range, g_range, b_range) = sixel_color_ranges(&box_to_split);
        let max_range = r_range.max(g_range).max(b_range);

        if max_range == r_range {
            box_to_split.sort_by_key(|&(c, _)| c.0);
        } else if max_range == g_range {
            box_to_split.sort_by_key(|&(c, _)| c.1);
        } else {
            box_to_split.sort_by_key(|&(c, _)| c.2);
        }

        let mid = box_to_split.len() / 2;
        let right = box_to_split.split_off(mid);
        boxes.push(box_to_split);
        boxes.push(right);
    }

    // Weighted average of each box → palette colour
    boxes
        .iter()
        .filter(|b| !b.is_empty())
        .map(|b| {
            let (rs, gs, bs, total) = b.iter().fold(
                (0u64, 0u64, 0u64, 0u64),
                |(rs, gs, bs, t), &((r, g, b), n)| {
                    let c = n as u64;
                    (
                        rs + r as u64 * c,
                        gs + g as u64 * c,
                        bs + b as u64 * c,
                        t + c,
                    )
                },
            );
            ((rs / total) as u8, (gs / total) as u8, (bs / total) as u8)
        })
        .collect()
}

/// Return the (R, G, B) channel ranges for a set of weighted colours.
fn sixel_color_ranges(colors: &[((u8, u8, u8), u32)]) -> (u8, u8, u8) {
    let mut r_min = 255u8;
    let mut r_max = 0u8;
    let mut g_min = 255u8;
    let mut g_max = 0u8;
    let mut b_min = 255u8;
    let mut b_max = 0u8;
    for &((r, g, b), _) in colors {
        r_min = r_min.min(r);
        r_max = r_max.max(r);
        g_min = g_min.min(g);
        g_max = g_max.max(g);
        b_min = b_min.min(b);
        b_max = b_max.max(b);
    }
    (r_max - r_min, g_max - g_min, b_max - b_min)
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

/// Pre-rendered Kitty Unicode placeholder image for tmux environments.
struct KittyUnicodeImage {
    id: u32,
    cols: usize,
    rows: usize,
    /// PNG data waiting to be transmitted; `None` once uploaded.
    pending_png: Option<Vec<u8>>,
    /// Whether the virtual placement has been created.
    placement_created: bool,
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

/// Pre-encoded Sixel image: full image + cached crop for scrolling.
struct SixelImage {
    cols: usize,
    total_rows: usize,
    cell_h_px: u32,
    /// Background colour used for alpha blending.
    bg: (u8, u8, u8),
    /// The resized image pixels (for cropping visible portions).
    resized: DynamicImage,
    /// Pre-encoded Sixel data for the full image.
    full_sixel: String,
    /// Cached crop: (first_row, num_rows, sixel_data).
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

/// Pre-rendered Terminology image: a local filesystem path Terminology will read.
/// For remote URLs a temp PNG file is written to a temporary directory.
struct TerminologyImage {
    /// Absolute path to the image file. Guaranteed to contain no NUL or `;`
    /// characters (paths failing this check are rejected at construction time).
    path: String,
    /// Display width in terminal columns (≤ 511).
    cols: u32,
    /// Display height in terminal rows (≤ 511).
    rows: u32,
    /// `true` if mdterm created this file and must delete it on cleanup.
    is_temp: bool,
    /// Pre-computed `"#".repeat(cols)` for efficient placeholder row rendering.
    hashes: String,
}

/// Result from a background pre-render thread.
enum PreRenderedResult {
    Kitty {
        id: u32,
        cols: usize,
        rows: usize,
        target_w: u32,
        target_h: u32,
        cell_h_px: u32,
        png: Vec<u8>,
    },
    KittyUnicode {
        id: u32,
        cols: usize,
        rows: usize,
        png: Vec<u8>,
    },
    Iterm2 {
        cols: usize,
        total_rows: usize,
        cell_h_px: u32,
        resized: DynamicImage,
        full_base64: String,
    },
    Sixel {
        cols: usize,
        total_rows: usize,
        cell_h_px: u32,
        bg: (u8, u8, u8),
        resized: DynamicImage,
        full_sixel: String,
    },
    HalfBlock {
        cols: usize,
        rows: usize,
        resized: DynamicImage,
    },
    Terminology {
        /// The `TerminologyImage` resolved during pre-render.
        img: TerminologyImage,
    },
}

pub struct ImageCache {
    images: HashMap<String, Option<Arc<DynamicImage>>>,
    protocol: ImageProtocol,

    // Kitty: image uploaded once, placed per-frame (None = encode failed)
    kitty_images: HashMap<String, Option<KittyImage>>,
    // Kitty Unicode placeholder: for tmux environments
    kitty_unicode_images: HashMap<String, Option<KittyUnicodeImage>>,
    next_kitty_id: u32,

    // iTerm2: pre-cropped strips cached per image (None = encode failed)
    iterm2_images: HashMap<String, Option<Iterm2Image>>,

    // Sixel: pre-encoded sixel data cached per image (None = encode failed)
    sixel_images: HashMap<String, Option<SixelImage>>,

    // Half-block: resized images for Unicode block rendering
    halfblock_images: HashMap<String, HalfBlockImage>,
    // Terminology: path-based images (None = pre-render failed)
    terminology_images: HashMap<String, Option<TerminologyImage>>,
    /// Paths of temp PNG files we created; deleted on cache clear / Drop.
    ///
    /// This is an `Arc<Mutex<...>>` so that background pre-render threads can
    /// register their temp file path *before* sending back the result.  This
    /// prevents leaks when the render channel is replaced mid-flight (e.g. on
    /// terminal resize): the thread still pushes its path into the registry even
    /// if the channel's receiver has been dropped, and `delete_temp_files` will
    /// clean it up on the next cache-clear or `Drop`.
    temp_files: Arc<Mutex<Vec<String>>>,

    last_render_width: usize,
    cell_metrics: CellMetrics,

    // Background fetch infrastructure
    sender: mpsc::Sender<(String, Option<DynamicImage>)>,
    receiver: mpsc::Receiver<(String, Option<DynamicImage>)>,
    in_flight: HashSet<String>,

    // Background pre-render infrastructure
    render_sender: mpsc::Sender<(String, usize, Option<PreRenderedResult>)>,
    render_receiver: mpsc::Receiver<(String, usize, Option<PreRenderedResult>)>,
    render_in_flight: HashSet<String>,
}

impl ImageCache {
    pub fn new() -> Self {
        let protocol = detect_protocol();
        let (sender, receiver) = mpsc::channel();
        let (render_sender, render_receiver) = mpsc::channel();
        ImageCache {
            images: HashMap::new(),
            protocol,
            kitty_images: HashMap::new(),
            kitty_unicode_images: HashMap::new(),
            // Starts at 0; wrapping_add(1) before first use ensures IDs begin at 1.
            // ID 0 is reserved in the Kitty protocol ("the last image").
            next_kitty_id: 0,
            iterm2_images: HashMap::new(),
            sixel_images: HashMap::new(),
            halfblock_images: HashMap::new(),
            terminology_images: HashMap::new(),
            temp_files: Arc::new(Mutex::new(Vec::new())),
            last_render_width: 0,
            cell_metrics: get_cell_metrics(),
            sender,
            receiver,
            in_flight: HashSet::new(),
            render_sender,
            render_receiver,
            render_in_flight: HashSet::new(),
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
            self.kitty_unicode_images.clear();
            self.iterm2_images.clear();
            self.sixel_images.clear();
            // Cancel stale in-flight pre-renders
            let (render_sender, render_receiver) = mpsc::channel();
            self.render_sender = render_sender;
            self.render_receiver = render_receiver;
            self.render_in_flight.clear();
            self.halfblock_images.clear();
            self.delete_temp_files();
            self.terminology_images.clear();
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
            self.images.insert(url, img.map(Arc::new));
            any = true;
        }
        any
    }

    /// Returns true if any fetches or pre-renders are currently in flight.
    pub fn has_in_flight(&self) -> bool {
        !self.in_flight.is_empty() || !self.render_in_flight.is_empty()
    }

    /// Number of fetches currently in flight.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    /// Cancel all in-flight fetches and pre-renders by replacing the channels.
    /// Background threads will finish but their results go to the dead channels.
    /// Already-cached images are preserved.
    pub fn cancel_in_flight(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.sender = sender;
        self.receiver = receiver;
        self.in_flight.clear();
        let (render_sender, render_receiver) = mpsc::channel();
        self.render_sender = render_sender;
        self.render_receiver = render_receiver;
        self.render_in_flight.clear();
    }

    /// Insert a pre-loaded image directly (used in tests).
    #[cfg(test)]
    fn insert(&mut self, url: &str, img: Option<image::DynamicImage>) {
        self.images.insert(url.to_string(), img.map(Arc::new));
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
        let img = fetch_image(url).map(|img| Arc::new(downscale(img, MAX_SOURCE_DIM)));
        self.images.insert(url.to_string(), img);
    }

    /// Returns true if the image has been pre-rendered and is ready for display.
    pub fn is_ready_to_render(&self, url: &str) -> bool {
        match self.protocol {
            ImageProtocol::Kitty => self.kitty_images.get(url).is_some_and(|o| o.is_some()),
            ImageProtocol::KittyUnicode => self
                .kitty_unicode_images
                .get(url)
                .is_some_and(|o| o.is_some()),
            ImageProtocol::Iterm2 => self.iterm2_images.get(url).is_some_and(|o| o.is_some()),
            ImageProtocol::Sixel => self.sixel_images.get(url).is_some_and(|o| o.is_some()),
            ImageProtocol::Terminology => self
                .terminology_images
                .get(url)
                .is_some_and(|o| o.is_some()),
            ImageProtocol::HalfBlock => self.halfblock_images.contains_key(url),
        }
    }

    /// Queue a background thread to pre-render a single image for display.
    fn queue_pre_render(&mut self, url: &str, content_width: usize, bg: (u8, u8, u8)) {
        if self.is_ready_to_render(url) || self.render_in_flight.contains(url) {
            return;
        }
        if self.render_in_flight.len() >= Self::MAX_CONCURRENT_FETCHES {
            return;
        }
        let img = match self.images.get(url).and_then(|o| o.as_ref()) {
            Some(img) => Arc::clone(img),
            None => return,
        };

        self.render_in_flight.insert(url.to_string());
        let sender = self.render_sender.clone();
        let url_owned = url.to_string();
        let protocol = self.protocol;
        let cell_metrics = self.cell_metrics;

        let kitty_id = if matches!(protocol, ImageProtocol::Kitty | ImageProtocol::KittyUnicode) {
            self.next_kitty_id = self.next_kitty_id.wrapping_add(1);
            // ID 0 is reserved; for KittyUnicode also stay within 24-bit range
            // since the image ID is encoded in the foreground color.
            if self.next_kitty_id == 0
                || (protocol == ImageProtocol::KittyUnicode && self.next_kitty_id > 0x00FF_FFFF)
            {
                self.next_kitty_id = 1;
            }
            self.next_kitty_id
        } else {
            0
        };

        let url_for_send = url_owned.clone();
        // Clone the Arc so the background thread can register temp paths even if
        // the render channel is replaced before the thread completes (M3 fix).
        let temp_files_ref = Arc::clone(&self.temp_files);
        std::thread::spawn(move || {
            // SAFETY (AssertUnwindSafe): all captured values are either owned
            // or wrapped in Arc/Mutex, which are both Send + unwind-safe at the
            // API level.  The Terminology arm acquires the mutex for a single
            // Vec::push and releases it immediately — no code between lock() and
            // the implicit drop can panic — so the closure cannot poison the
            // mutex from the inside.  Poison from other threads is recovered via
            // unwrap_or_else in the Terminology arm.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // `then` (lazy) is intentional: `TerminologyCtx` borrows
                // `temp_files_ref`, which is a local — `then_some` would
                // evaluate eagerly and fail the borrow checker.
                #[allow(clippy::unnecessary_lazy_evaluations)]
                let terminology =
                    (protocol == ImageProtocol::Terminology).then(|| TerminologyCtx {
                        url: url_owned.as_str(),
                        temp_files: &temp_files_ref,
                    });
                pre_render_image(
                    &img,
                    protocol,
                    content_width,
                    cell_metrics,
                    bg,
                    kitty_id,
                    terminology.as_ref(),
                )
            }))
            .unwrap_or(None);
            let _ = sender.send((url_for_send, content_width, result));
        });
    }

    /// Queue background pre-rendering for all loaded images that haven't been
    /// pre-rendered yet. Clears caches when `content_width` changes.
    pub fn queue_all_pre_renders(&mut self, content_width: usize, bg: (u8, u8, u8)) {
        if content_width != self.last_render_width {
            self.kitty_images.clear();
            self.kitty_unicode_images.clear();
            self.iterm2_images.clear();
            self.sixel_images.clear();
            self.halfblock_images.clear();
            self.delete_temp_files();
            self.terminology_images.clear();
            // Cancel stale in-flight pre-renders for the old width
            let (render_sender, render_receiver) = mpsc::channel();
            self.render_sender = render_sender;
            self.render_receiver = render_receiver;
            self.render_in_flight.clear();
            self.last_render_width = content_width;
        }

        let urls: Vec<String> = self
            .images
            .iter()
            .filter_map(|(url, opt)| opt.as_ref().map(|_| url.clone()))
            .collect();

        for url in urls {
            self.queue_pre_render(&url, content_width, bg);
        }
    }

    /// Poll for completed background pre-renders. Returns true if any new
    /// pre-rendered images are now ready for display.
    pub fn poll_pre_rendered(&mut self) -> bool {
        let mut any = false;
        while let Ok((url, content_width, data)) = self.render_receiver.try_recv() {
            self.render_in_flight.remove(&url);
            // Discard results for stale content widths
            if content_width != self.last_render_width {
                continue;
            }
            if let Some(data) = data {
                match data {
                    PreRenderedResult::Kitty {
                        id,
                        cols,
                        rows,
                        target_w,
                        target_h,
                        cell_h_px,
                        png,
                    } => {
                        self.kitty_images.insert(
                            url,
                            Some(KittyImage {
                                id,
                                cols,
                                rows,
                                target_w,
                                target_h,
                                cell_h_px,
                                pending_png: Some(png),
                            }),
                        );
                    }
                    PreRenderedResult::KittyUnicode {
                        id,
                        cols,
                        rows,
                        png,
                    } => {
                        self.kitty_unicode_images.insert(
                            url,
                            Some(KittyUnicodeImage {
                                id,
                                cols,
                                rows,
                                pending_png: Some(png),
                                placement_created: false,
                            }),
                        );
                    }
                    PreRenderedResult::Iterm2 {
                        cols,
                        total_rows,
                        cell_h_px,
                        resized,
                        full_base64,
                    } => {
                        self.iterm2_images.insert(
                            url,
                            Some(Iterm2Image {
                                cols,
                                total_rows,
                                cell_h_px,
                                resized,
                                full_base64,
                                crop_cache: None,
                            }),
                        );
                    }
                    PreRenderedResult::Sixel {
                        cols,
                        total_rows,
                        cell_h_px,
                        bg,
                        resized,
                        full_sixel,
                    } => {
                        self.sixel_images.insert(
                            url,
                            Some(SixelImage {
                                cols,
                                total_rows,
                                cell_h_px,
                                bg,
                                resized,
                                full_sixel,
                                crop_cache: None,
                            }),
                        );
                    }
                    PreRenderedResult::HalfBlock {
                        cols,
                        rows,
                        resized,
                    } => {
                        self.halfblock_images.insert(
                            url,
                            HalfBlockImage {
                                cols,
                                rows,
                                resized,
                            },
                        );
                    }
                    PreRenderedResult::Terminology { img } => {
                        // Note: if img.is_temp, the path was already registered in
                        // the shared temp_files registry by the background thread
                        // (before sending).  No need to push again here.
                        self.terminology_images.insert(url, Some(img));
                    }
                }
                any = true;
            }
        }
        any
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
            ImageProtocol::KittyUnicode => {
                self.render_kitty_unicode_row(stdout, url, image_row, content_width)
            }
            ImageProtocol::HalfBlock => {
                self.render_halfblock_row(stdout, url, image_row, content_width, bg)
            }
            ImageProtocol::Iterm2 | ImageProtocol::Sixel => Ok(false),
            // Terminology renders the whole block in a later pass via absolute cursor
            // positioning.  We still need to claim the row here (return true) so the
            // normal text-rendering path doesn't write styled spans into these cells —
            // those would show as dark lines around the image.  Write plain spaces so
            // the row is clean before Terminology overlays the image.
            //
            // We write exactly `content_width` spaces regardless of centering, because
            // the caller has already positioned the cursor at the start of the content
            // area.  The Terminology block overlay then positions via absolute MoveTo,
            // so the two passes are independent and covering the full row is correct.
            ImageProtocol::Terminology => {
                if self
                    .terminology_images
                    .get(url)
                    .and_then(|o| o.as_ref())
                    .is_some()
                {
                    write!(stdout, "{}", " ".repeat(content_width))?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Transmit any Kitty images that haven't been uploaded to the terminal yet.
    /// Call this once per frame, before placing images.
    pub fn transmit_pending_kitty(&mut self, stdout: &mut impl Write) -> io::Result<()> {
        for ki in self.kitty_images.values_mut().flatten() {
            if let Some(png_data) = ki.pending_png.take() {
                transmit_kitty_image(stdout, &png_data, ki.id)?;
            }
        }
        Ok(())
    }

    /// Reset all `placement_created` flags so placements are recreated on the
    /// next `transmit_pending_kitty_unicode` call. Must be called after
    /// `kitty_unicode_delete_all` which clears placements in the terminal.
    pub fn reset_kitty_unicode_placements(&mut self) {
        for ki in self.kitty_unicode_images.values_mut().flatten() {
            ki.placement_created = false;
        }
    }

    /// Transmit any Kitty Unicode placeholder images and create virtual placements.
    /// Call this once per frame, before rendering placeholder characters.
    pub fn transmit_pending_kitty_unicode(&mut self, stdout: &mut impl Write) -> io::Result<()> {
        for ki in self.kitty_unicode_images.values_mut().flatten() {
            if let Some(png_data) = ki.pending_png.take() {
                transmit_kitty_image_tmux(stdout, &png_data, ki.id)?;
            }
            if !ki.placement_created {
                create_virtual_placement_tmux(stdout, ki.id, ki.cols, ki.rows)?;
                ki.placement_created = true;
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
        let ki = match self.kitty_images.get(url).and_then(|o| o.as_ref()) {
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

    fn render_kitty_unicode_row(
        &self,
        stdout: &mut impl Write,
        url: &str,
        image_row: usize,
        content_width: usize,
    ) -> io::Result<bool> {
        let ki = match self.kitty_unicode_images.get(url).and_then(|o| o.as_ref()) {
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

        // Encode image ID as 24-bit RGB foreground color
        let r = (ki.id >> 16) & 0xFF;
        let g = (ki.id >> 8) & 0xFF;
        let b = ki.id & 0xFF;
        write!(stdout, "\x1b[38;2;{};{};{}m", r, g, b)?;

        debug_assert!(image_row < DIACRITICS.len());
        let row_diacritic = DIACRITICS[image_row];
        for &col_diacritic in &DIACRITICS[..ki.cols] {
            write!(stdout, "\u{10EEEE}{}{}", row_diacritic, col_diacritic)?;
        }

        // Reset foreground color
        write!(stdout, "\x1b[39m")?;

        let used = x_offset + ki.cols;
        if used < content_width {
            write!(stdout, "{}", " ".repeat(content_width - used))?;
        }
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
        let ii = match self.iterm2_images.get_mut(url).and_then(|o| o.as_mut()) {
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

    /// Render a visible portion of a Sixel image as a single inline block.
    /// Works like `render_iterm2_block`: positions cursor, emits Sixel data.
    pub fn render_sixel_block(
        &mut self,
        stdout: &mut impl Write,
        url: &str,
        first_row: usize,
        num_rows: usize,
        content_width: usize,
        screen_y: u16,
    ) -> io::Result<()> {
        let si = match self.sixel_images.get_mut(url).and_then(|o| o.as_mut()) {
            Some(si) => si,
            None => return Ok(()),
        };

        let x_col = 2 + content_width.saturating_sub(si.cols) / 2;

        // Pick the right Sixel payload: full image or a cached crop
        let data: &str = if first_row == 0 && num_rows == si.total_rows {
            &si.full_sixel
        } else {
            if !si
                .crop_cache
                .as_ref()
                .is_some_and(|(fr, nr, _)| *fr == first_row && *nr == num_rows)
            {
                let y =
                    (first_row as u32 * si.cell_h_px).min(si.resized.height().saturating_sub(1));
                let h = (num_rows as u32 * si.cell_h_px)
                    .min(si.resized.height().saturating_sub(y))
                    .max(1);
                let cropped = si.resized.crop_imm(0, y, si.resized.width(), h);
                si.crop_cache = Some((first_row, num_rows, encode_sixel(&cropped, si.bg)));
            }
            &si.crop_cache.as_ref().unwrap().2
        };

        // Position cursor and emit the Sixel sequence
        write!(stdout, "\x1b[{};{}H", screen_y + 1, x_col + 1)?;
        stdout.write_all(data.as_bytes())?;

        Ok(())
    }

    /// Delete all tracked temp files from disk and clear the list.
    ///
    /// This drains the `Arc<Mutex<Vec<String>>>` registry, which is also shared
    /// with background threads.  Any thread that completes after this point but
    /// before the next `delete_temp_files` call will push its path into the
    /// (now-empty) list; it will be cleaned up on the subsequent call or on Drop.
    ///
    /// Recovers from a poisoned mutex (a previous thread panicked while holding
    /// the lock) — we still want to drain and delete even in that case.
    fn delete_temp_files(&mut self) {
        let mut files = match self.temp_files.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        for path in files.drain(..) {
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Emit the [Terminology] inline image escape sequence for a visible image block.
    ///
    /// [Terminology]: https://www.enlightenment.org/about-terminology
    ///
    /// Protocol format (from Terminology's tycat.c / prnt()):
    ///   Header (NUL-terminated):  ESC } i c # <cols> ; <rows> ; <path> NUL
    ///   Per row (NUL-terminated): ESC } i b NUL  <cols × '#'>  ESC } i e NUL
    ///
    /// `\x1b}` = byte 0x1B followed by `}` (0x7D) — verified against tycat.c source
    /// (`snprintf(buf, ..., "%c}ic#%i;%i;%s", 0x1b, ...)`).
    ///
    /// `screen_y` is 0-based; cursor positioning uses 1-based ANSI coords.
    /// `content_col` is the 0-based terminal column where the content area starts
    /// (i.e. after the left gutter `│ `). `content_width` is the width of that
    /// area in columns. The image is centered horizontally within the content area,
    /// matching the behaviour of all other protocols (Kitty, iTerm2, Sixel, HalfBlock).
    /// Each placeholder row is positioned explicitly with MoveTo — no `\n` — safe inside a TUI.
    ///
    /// **Scroll limitation:** Unlike iTerm2/Sixel, Terminology does not support sub-rectangle
    /// rendering. When a large image is partially scrolled off-screen, this method always
    /// renders from the image top. The caller is responsible for only calling this method
    /// when the full image block is visible (or partially visible starting from row 0).
    pub fn render_terminology_block(
        &self,
        stdout: &mut impl Write,
        url: &str,
        screen_y: u16,
        content_col: u16,
        content_width: usize,
    ) -> io::Result<()> {
        let ti = match self.terminology_images.get(url).and_then(|o| o.as_ref()) {
            Some(ti) => ti,
            None => return Ok(()),
        };

        // Center the image within the content area, same as all other protocols.
        let x_offset = content_width.saturating_sub(ti.cols as usize) / 2;
        let start_col = content_col as usize + x_offset;

        // Position at first row, centered column (+1 for the title bar row, +1 for 1-based cols).
        write!(
            stdout,
            "\x1b[{};{}H",
            (screen_y as u32).saturating_add(1),
            (start_col as u32).saturating_add(1)
        )?;

        // Header: ESC } i c # <cols> ; <rows> ; <path> NUL
        // Path is guaranteed to contain no NUL or ';' (checked at construction time).
        stdout.write_all(b"\x1b}ic#")?;
        write!(stdout, "{};{};{}", ti.cols, ti.rows, ti.path)?;
        stdout.write_all(b"\0")?;

        // Placeholder rows: one MoveTo per row so we never rely on \n inside the TUI.
        // Terminology replaces the '#' cells with image pixels in-place.
        // Use pre-computed hashes string from TerminologyImage to avoid repeated allocation.
        debug_assert_eq!(
            ti.hashes.len(),
            ti.cols as usize,
            "TerminologyImage hashes field out of sync with cols"
        );
        for r in 0..ti.rows {
            write!(
                stdout,
                "\x1b[{};{}H",
                (screen_y as u32).saturating_add(1).saturating_add(r),
                (start_col as u32).saturating_add(1)
            )?;
            stdout.write_all(b"\x1b}ib\0")?;
            stdout.write_all(ti.hashes.as_bytes())?;
            stdout.write_all(b"\x1b}ie\0")?;
        }

        Ok(())
    }

    /// Render a visible block of an image using the current protocol (iTerm2, Sixel, or Terminology).
    /// Dispatches to the protocol-specific method.
    ///
    /// `content_col` is the 0-based terminal column where content starts (after the left gutter).
    /// Only used by the Terminology protocol.
    #[allow(clippy::too_many_arguments)]
    pub fn render_block_image(
        &mut self,
        stdout: &mut impl Write,
        url: &str,
        first_row: usize,
        num_rows: usize,
        content_width: usize,
        screen_y: u16,
        content_col: u16,
    ) -> io::Result<()> {
        match self.protocol {
            ImageProtocol::Iterm2 => {
                self.render_iterm2_block(stdout, url, first_row, num_rows, content_width, screen_y)
            }
            ImageProtocol::Sixel => {
                self.render_sixel_block(stdout, url, first_row, num_rows, content_width, screen_y)
            }
            ImageProtocol::Terminology => {
                self.render_terminology_block(stdout, url, screen_y, content_col, content_width)
            }
            _ => Ok(()),
        }
    }
}

impl Drop for ImageCache {
    fn drop(&mut self) {
        self.delete_temp_files();
    }
}

// ── Half-block helpers ──────────────────────────────────────────────────────

pub fn color_to_rgb(c: crossterm::style::Color) -> (u8, u8, u8) {
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

/// Extra context required by the [`ImageProtocol::Terminology`] pre-render
/// arm.  Pass `None` for all other protocols; the fields are only accessed
/// inside the `Terminology` match arm.
struct TerminologyCtx<'a> {
    /// Original image URL (or file path), used to derive a stable path for
    /// the temporary file that Terminology reads.
    url: &'a str,
    /// Shared registry of temporary-file paths; entries are cleaned up on
    /// resize, theme change, or process exit.
    temp_files: &'a Arc<Mutex<Vec<String>>>,
}

/// Pre-render an image for a specific protocol on a background thread.
fn pre_render_image(
    img: &DynamicImage,
    protocol: ImageProtocol,
    content_width: usize,
    cell_metrics: CellMetrics,
    bg: (u8, u8, u8),
    kitty_id: u32,
    terminology: Option<&TerminologyCtx<'_>>,
) -> Option<PreRenderedResult> {
    let (img_w, img_h) = img.dimensions();
    let (cols, rows) = calc_display_cells(
        img_w,
        img_h,
        content_width,
        MAX_IMAGE_ROWS,
        cell_metrics.aspect,
    );

    match protocol {
        ImageProtocol::Kitty => {
            let target_w = (cols as u32 * cell_metrics.cell_w_px).max(1);
            let target_h = (rows as u32 * cell_metrics.cell_h_px).max(1);
            let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
            let png = encode_png(&resized)?;
            Some(PreRenderedResult::Kitty {
                id: kitty_id,
                cols,
                rows,
                target_w,
                target_h,
                cell_h_px: cell_metrics.cell_h_px,
                png,
            })
        }
        ImageProtocol::KittyUnicode => {
            // Clamp cols and rows to DIACRITICS table size — each column/row
            // needs a combining diacritic and we only have 256 entries.
            let cols = cols.min(DIACRITICS.len());
            let rows = rows.min(DIACRITICS.len());
            let target_w = (cols as u32 * cell_metrics.cell_w_px).max(1);
            let target_h = (rows as u32 * cell_metrics.cell_h_px).max(1);
            let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
            let png = encode_png(&resized)?;
            Some(PreRenderedResult::KittyUnicode {
                id: kitty_id,
                cols,
                rows,
                png,
            })
        }
        ImageProtocol::Iterm2 => {
            let target_w = (cols as u32 * cell_metrics.cell_w_px).max(1);
            let target_h = (rows as u32 * cell_metrics.cell_h_px).max(1);
            let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
            let png = encode_png(&resized)?;
            let full_base64 = BASE64.encode(png);
            Some(PreRenderedResult::Iterm2 {
                cols,
                total_rows: rows,
                cell_h_px: cell_metrics.cell_h_px,
                resized,
                full_base64,
            })
        }
        ImageProtocol::Sixel => {
            let target_w = (cols as u32 * cell_metrics.cell_w_px).max(1);
            let target_h = (rows as u32 * cell_metrics.cell_h_px).max(1);
            let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
            let full_sixel = encode_sixel(&resized, bg);
            Some(PreRenderedResult::Sixel {
                cols,
                total_rows: rows,
                cell_h_px: cell_metrics.cell_h_px,
                bg,
                resized,
                full_sixel,
            })
        }
        ImageProtocol::HalfBlock => {
            let target_w = (cols as u32).max(1);
            let target_h = (rows as u32 * 2).max(1);
            let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
            Some(PreRenderedResult::HalfBlock {
                cols,
                rows,
                resized,
            })
        }
        ImageProtocol::Terminology => {
            let ctx = terminology
                .expect("pre_render_image: Terminology protocol requires a TerminologyCtx");
            pre_render_terminology(img, ctx.url, content_width, cell_metrics).map(|ti| {
                if ti.is_temp {
                    // Register the temp path in the shared registry *before* wrapping
                    // the result, so it is cleaned up even if the render channel is
                    // replaced before the result is polled (e.g. on a resize event).
                    //
                    // SAFETY (AssertUnwindSafe): the MutexGuard `files` is held only
                    // for the single `push` call and is immediately dropped.  No code
                    // between `lock()` and the implicit drop can panic, so this
                    // closure cannot poison the mutex.  Poison from other threads is
                    // handled by `unwrap_or_else`: Vec::push either completes or has
                    // not run yet when a panic occurs, so the contents are always
                    // consistent.
                    let mut files = ctx.temp_files.lock().unwrap_or_else(|e| e.into_inner());
                    files.push(ti.path.to_owned());
                }
                PreRenderedResult::Terminology { img: ti }
            })
        }
    }
}

/// Validate that a path intended for the Terminology protocol escape sequence
/// contains no bytes that would corrupt or inject into the
/// `ESC } i c # <cols> ; <rows> ; <path> NUL` framing.
///
/// Blocked categories:
/// - **Control bytes (0x00–0x1F):** NUL truncates the sequence; ESC (`\x1b`) could
///   start a second injected escape sequence; CR/LF could confuse line-oriented
///   terminal parsers.
/// - **DEL (0x7F):** a control character on most systems.
/// - **Semicolon (`;`):** field separator in the protocol header — would split
///   `<cols>;<rows>;<path>` at the wrong position, redirecting Terminology to a
///   different file.
///
/// All other bytes (printable ASCII + valid UTF-8 multibyte sequences) are allowed.
/// Since this function receives a `&str`, the input is guaranteed to be valid UTF-8
/// with no interior NUL bytes; the `b == 0` case is kept for explicitness.
fn terminology_path_safe(path: &str) -> bool {
    path.bytes().all(|b| b >= 0x20 && b != 0x7f && b != b';')
}

/// Generate a random suffix from 8 random bytes encoded as a 16-character hex
/// string, using `/dev/urandom` on Unix.
/// Falls back to PID + timestamp if `/dev/urandom` is unavailable.
fn random_hex_suffix() -> String {
    let mut buf = [0u8; 8];

    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom")
            && f.read_exact(&mut buf).is_ok()
        {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }

    // Fallback: mix PID, counter, and nanosecond timestamp for uniqueness.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:08x}{:08x}", std::process::id() ^ ts, n)
}

/// Pre-render step for the Terminology protocol.
/// Resolves the image to a local filesystem path.
/// - If `url` is a local path that currently exists on disk, returns it as-is
///   (after canonicalization to an absolute path), provided the resolved path
///   stays within the current working directory.
/// - Otherwise, resizes `img` to the display pixel dimensions and writes it
///   atomically to a temporary PNG file in a per-process private temp directory.
///
/// Returns `None` if the resolved path would be unsafe to embed in the escape
/// sequence, escapes the working directory, or if any I/O operation fails.
fn pre_render_terminology(
    img: &DynamicImage,
    url: &str,
    content_width: usize,
    cell_metrics: CellMetrics,
) -> Option<TerminologyImage> {
    let (img_w, img_h) = img.dimensions();
    let (cols, rows) = calc_display_cells(
        img_w,
        img_h,
        content_width,
        MAX_IMAGE_ROWS,
        cell_metrics.aspect,
    );
    // Terminology hard limit: both width and height must be < 512 (from tycat.c:
    // `if ((w >= 512) || (h >= 512)) return;`). Also ensure neither is zero.
    let cols = (cols as u32).clamp(1, 511);
    let rows = (rows as u32).clamp(1, 511);

    // Treat http://, https://, file://, and data: URLs as remote/non-local.
    // file:// and data: are not valid filesystem paths; canonicalize would fail
    // for them, so they fall through to the temp-file path.
    let is_remote = url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("file://")
        || url.starts_with("data:");

    // Local file path that still exists on disk — reuse directly, no I/O needed.
    // Terminology requires an absolute path, so canonicalize before passing.
    if !is_remote && let Ok(abs) = std::fs::canonicalize(url) {
        // SEC: Reject if the resolved path escapes the working directory.
        // This prevents symlinks like `./img.png -> /etc/passwd` from
        // passing an arbitrary system path to Terminology.
        // Canonicalize the CWD too so both paths use the same prefix form
        // (important on Windows where canonicalize adds the `\\?\` prefix
        // but current_dir() does not, causing starts_with to always fail).
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|d| std::fs::canonicalize(d).ok());
        if cwd.is_some_and(|cwd| !abs.starts_with(&cwd)) {
            return None;
        }

        // SEC: Use to_str() (not to_string_lossy()) to reject non-UTF-8 paths.
        // to_string_lossy() would replace invalid bytes with U+FFFD, meaning
        // the safety check runs on a modified string, not the actual bytes.
        let path = abs.to_str()?.to_owned();

        // SEC: Reject paths that would corrupt the escape sequence framing
        // (control bytes, DEL, semicolons). See terminology_path_safe docs.
        if !terminology_path_safe(&path) {
            return None;
        }

        let hashes = "#".repeat(cols as usize);
        return Some(TerminologyImage {
            path,
            cols,
            rows,
            is_temp: false,
            hashes,
        });
    }

    // Remote image (or local path no longer on disk): resize and write a temp file.
    let target_w = (cols * cell_metrics.cell_w_px).max(1);
    let target_h = (rows * cell_metrics.cell_h_px).max(1);
    let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);

    // Create a per-process private temp directory (mode 0700 on Unix) so that
    // other local users cannot observe, replace, or symlink-attack our temp files.
    // This also isolates us from TMPDIR poisoning by an adversarial environment.
    let tmp_base = std::env::temp_dir();
    let priv_dir = tmp_base.join(format!("mdterm-{}", std::process::id()));
    if std::fs::create_dir_all(&priv_dir).is_err() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SEC: Verify we own the directory before using it. `create_dir_all`
        // succeeds silently if the directory already exists — even if it was
        // pre-created by a local attacker. An ownership mismatch means we must
        // not write temp files there (the attacker could read or replace them).
        // Use symlink_metadata (not metadata) so a symlink planted at the path
        // is detected as non-dir and rejected, closing a symlink-substitution vector.
        let meta = std::fs::symlink_metadata(&priv_dir).ok()?;
        if !meta.is_dir() {
            return None;
        }
        if meta.uid() != unsafe { libc::getuid() } {
            return None;
        }
        let _ = std::fs::set_permissions(&priv_dir, std::fs::Permissions::from_mode(0o700));
    }

    // Build a non-predictable temp path using random entropy (SEC-3).
    let path = priv_dir
        .join(format!("img-{}.png", random_hex_suffix()))
        .to_str()?
        .to_owned();

    // SEC: Reject if the generated temp path somehow contains unsafe characters
    // (defensive — ';' in TMPDIR would break framing).
    if !terminology_path_safe(&path) {
        return None;
    }

    // Write PNG atomically using O_CREAT | O_EXCL so we never silently overwrite
    // an existing file (SEC-2). If the file already exists (race), this returns
    // None and the caller retries on the next render cycle.
    {
        use std::io::{BufWriter, Write as IoWrite};
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true) // O_CREAT | O_EXCL — fails if path already exists
            .open(&path)
            .ok()?;
        let mut writer = BufWriter::new(file);
        let mut png_buf = std::io::Cursor::new(Vec::new());
        resized
            .write_to(&mut png_buf, image::ImageFormat::Png)
            .ok()?;
        writer.write_all(png_buf.get_ref()).ok()?;
        writer.flush().ok()?;
    }

    let hashes = "#".repeat(cols as usize);
    Some(TerminologyImage {
        path,
        cols,
        rows,
        is_temp: true,
        hashes,
    })
}

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
        "[0:0:0:0:0:0:0:1]",
        "0:0:0:0:0:0:0:1",
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

    // Shared mutex for tests that manipulate process-global env vars.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    #[test]
    fn is_blocked_host_blocks_ipv6_expanded() {
        assert!(is_blocked_host("[0:0:0:0:0:0:0:1]"));
        assert!(is_blocked_host("0:0:0:0:0:0:0:1"));
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

    // ── Sixel encoding ─────────────────────────────────────────────────────

    #[test]
    fn sixel_encode_empty_image_returns_empty() {
        let img = DynamicImage::new_rgb8(0, 0);
        assert!(encode_sixel(&img, (0, 0, 0)).is_empty());
    }

    #[test]
    fn sixel_encode_has_dcs_and_st() {
        let img = DynamicImage::new_rgb8(2, 2);
        let data = encode_sixel(&img, (0, 0, 0));
        assert!(data.starts_with("\x1bP"), "should start with DCS");
        assert!(data.ends_with("\x1b\\"), "should end with ST");
    }

    #[test]
    fn sixel_encode_contains_raster_attributes() {
        let img = DynamicImage::new_rgb8(4, 3);
        let data = encode_sixel(&img, (0, 0, 0));
        assert!(
            data.contains("\"1;1;4;3"),
            "should contain raster attributes"
        );
    }

    #[test]
    fn sixel_encode_contains_color_definitions() {
        let img = DynamicImage::new_rgb8(1, 1);
        let data = encode_sixel(&img, (0, 0, 0));
        // At least one color register should be defined
        assert!(data.contains("#0;2;"), "should define at least one color");
    }

    #[test]
    fn sixel_rle_compresses_repeated_chars() {
        let mut out = String::new();
        sixel_rle(&[0x7E, 0x7E, 0x7E, 0x7E, 0x7E], &mut out);
        assert_eq!(out, "!5~", "5 repeated '~' should be RLE-compressed");
    }

    #[test]
    fn sixel_rle_short_runs_not_compressed() {
        let mut out = String::new();
        sixel_rle(&[0x3F, 0x3F], &mut out);
        assert_eq!(out, "??", "2 repeated chars should not use RLE");
    }

    #[test]
    fn sixel_encode_alpha_blending() {
        // A 1×1 image with 50% alpha red, blended against white bg
        let mut img = DynamicImage::new_rgba8(1, 1);
        img.as_mut_rgba8()
            .unwrap()
            .put_pixel(0, 0, image::Rgba([255, 0, 0, 128]));
        let data = encode_sixel(&img, (255, 255, 255));
        // Should produce valid Sixel output (not empty)
        assert!(data.starts_with("\x1bP"));
        assert!(data.ends_with("\x1b\\"));
        // The blended colour should be roughly (255, 127, 127) → ~(100%, 50%, 50%)
        // Verify the color register is close: expect #0;2;100;50;50 (±1 from rounding)
        let color_def = data
            .split('#')
            .find(|s| s.starts_with("0;2;"))
            .expect("should have a color definition");
        let parts: Vec<u32> = color_def
            .split(';')
            .skip(2)
            .filter_map(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0] >= 99, "R should be ~100, got {}", parts[0]);
        assert!(
            (49..=51).contains(&parts[1]),
            "G should be ~50, got {}",
            parts[1]
        );
        assert!(
            (49..=51).contains(&parts[2]),
            "B should be ~50, got {}",
            parts[2]
        );
    }

    #[test]
    fn sixel_quantize_few_colors_preserves_all() {
        // Image with only 3 unique colors — should not quantize
        let pixels = vec![(255, 0, 0), (0, 255, 0), (0, 0, 255)];
        let (palette, indexed) = sixel_quantize(&pixels);
        assert_eq!(palette.len(), 3);
        assert_eq!(indexed.len(), 3);
        // Each pixel should map to a unique index
        let mut indices: Vec<usize> = indexed.clone();
        indices.sort();
        indices.dedup();
        assert_eq!(indices.len(), 3);
    }

    #[test]
    fn sixel_quantize_many_colors_limited_to_256() {
        // Create >256 unique colors
        let mut pixels = Vec::new();
        for r in 0..8 {
            for g in 0..8 {
                for b in 0..8 {
                    pixels.push((r * 32, g * 32, b * 32));
                }
            }
        }
        assert!(pixels.len() > 256);
        let (palette, indexed) = sixel_quantize(&pixels);
        assert!(palette.len() <= 256);
        assert_eq!(indexed.len(), pixels.len());
    }

    // ── tmux_wrap ───────────────────────────────────────────────────────────

    #[test]
    fn tmux_wrap_doubles_esc_bytes() {
        // A minimal Kitty sequence: ESC _ G ... ESC \
        let input = b"\x1b_Ga=d,d=a\x1b\\";
        let wrapped = tmux_wrap(input);
        // Should be: ESC P t m u x ; ESC ESC _ G a = d , d = a ESC ESC \ ESC \
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\x1b_Ga=d,d=a\x1b\x1b\\\x1b\\");
    }

    #[test]
    fn tmux_wrap_no_esc_passthrough() {
        // Input with no ESC bytes should just get the DCS wrapper.
        let input = b"hello";
        let wrapped = tmux_wrap(input);
        assert_eq!(wrapped, b"\x1bPtmux;hello\x1b\\");
    }

    // ── Terminology protocol ────────────────────────────────────────────────

    /// `detect_protocol()` must return `Terminology` when `TERMINOLOGY=1` and
    /// `TMUX` is unset, regardless of other env vars.
    #[test]
    fn detect_terminology_protocol() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::new(&[
            "TERMINOLOGY",
            "TMUX",
            "KITTY_WINDOW_ID",
            "TERM_PROGRAM",
            "TERM",
            "LC_TERMINAL",
            "MLTERM",
            "KONSOLE_VERSION",
            "MDTERM_IMAGE_PROTOCOL",
        ]);

        // Safety: test holds ENV_LOCK mutex, preventing concurrent env mutation.
        unsafe {
            std::env::remove_var("TMUX");
            std::env::remove_var("KITTY_WINDOW_ID");
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("TERM");
            std::env::remove_var("LC_TERMINAL");
            std::env::remove_var("MLTERM");
            std::env::remove_var("KONSOLE_VERSION");
            std::env::remove_var("MDTERM_IMAGE_PROTOCOL");
            std::env::set_var("TERMINOLOGY", "1");
        }

        let proto = detect_protocol();
        assert_eq!(proto, ImageProtocol::Terminology);
        // _env restores all saved vars on drop.
    }

    /// When `TERMINOLOGY=1` AND `TMUX` is also set, the `ESC }` protocol cannot
    /// pass through tmux DCS — fall back to `HalfBlock`.
    #[test]
    fn detect_terminology_blocked_in_tmux() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::new(&[
            "TERMINOLOGY",
            "TMUX",
            "KITTY_WINDOW_ID",
            "TERM_PROGRAM",
            "TERM",
            "LC_TERMINAL",
            "MLTERM",
            "KONSOLE_VERSION",
            "MDTERM_IMAGE_PROTOCOL",
        ]);

        // Safety: test holds ENV_LOCK mutex, preventing concurrent env mutation.
        unsafe {
            std::env::remove_var("KITTY_WINDOW_ID");
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("TERM");
            std::env::remove_var("LC_TERMINAL");
            std::env::remove_var("MLTERM");
            std::env::remove_var("KONSOLE_VERSION");
            std::env::remove_var("MDTERM_IMAGE_PROTOCOL");
            std::env::set_var("TERMINOLOGY", "1");
            std::env::set_var("TMUX", "/tmp/tmux-1000/default,12345,0");
        }

        let proto = detect_protocol();

        // TMUX present → KittyUnicode *only* if TERM_PROGRAM is ghostty/WezTerm etc.
        // Since we cleared those, and TMUX is set, the tmux branch runs first but
        // none of its arms match → falls through entirely to HalfBlock.
        assert_eq!(proto, ImageProtocol::HalfBlock);
        // _env restores all saved vars on drop.
    }

    /// The Terminology escape sequence bytes must be well-formed:
    /// - Header: `ESC } i c # <cols> ; <rows> ; <path> NUL`
    /// - Each placeholder row: `ESC } i b NUL <cols × '#'> ESC } i e NUL`
    ///   (rows do NOT end with LF — each row is positioned via MoveTo instead)
    #[test]
    fn terminology_escape_bytes() {
        let mut cache = ImageCache::new();
        let url = "test://dummy.png";
        cache.terminology_images.insert(
            url.to_string(),
            Some(TerminologyImage {
                path: "/tmp/test-img.png".to_string(),
                cols: 3,
                rows: 2,
                is_temp: false,
                hashes: "###".to_string(),
            }),
        );

        let mut buf: Vec<u8> = Vec::new();
        cache
            .render_terminology_block(&mut buf, url, 0, 2, 80)
            .unwrap();

        // ── Check header ──────────────────────────────────────────────────
        let header_prefix = b"\x1b}ic#3;2;/tmp/test-img.png\0";
        assert!(
            buf.windows(header_prefix.len()).any(|w| w == header_prefix),
            "header not found in output.\nGot (hex): {:?}",
            &buf
        );

        // ── Check placeholder rows ────────────────────────────────────────
        // Rows no longer end with LF — each row is positioned via MoveTo instead.
        let row_begin = b"\x1b}ib\0";
        let row_hashes = b"###";
        let row_end = b"\x1b}ie\0";

        let begin_count = buf
            .windows(row_begin.len())
            .filter(|w| *w == row_begin)
            .count();
        let end_count = buf.windows(row_end.len()).filter(|w| *w == row_end).count();

        assert_eq!(
            begin_count, 2,
            "expected 2 row-begin markers (rows=2), got {begin_count}"
        );
        assert_eq!(
            end_count, 2,
            "expected 2 row-end+LF markers (rows=2), got {end_count}"
        );

        let mut pos = 0;
        let mut rows_seen = 0;
        while pos + row_begin.len() <= buf.len() {
            if &buf[pos..pos + row_begin.len()] == row_begin {
                let hash_start = pos + row_begin.len();
                let hash_end = hash_start + 3; // cols=3
                assert!(
                    hash_end <= buf.len(),
                    "buffer too short after row-begin at pos {pos}"
                );
                assert_eq!(
                    &buf[hash_start..hash_end],
                    row_hashes,
                    "row body at pos {pos} is not exactly 3 '#' chars"
                );
                rows_seen += 1;
                pos = hash_end;
            } else {
                pos += 1;
            }
        }
        assert_eq!(rows_seen, 2, "expected to find 2 complete rows");
    }

    /// Local-path images (within the current working directory) must reuse the
    /// original path (`is_temp = false`) and must NOT create a new temp file.
    ///
    /// The fixture is written into the current working directory (project root
    /// during `cargo test`) so that the `canonicalize` result passes the
    /// `starts_with(cwd)` security check.
    #[test]
    fn terminology_no_temp_for_local_path() {
        // Write the fixture PNG inside cwd so it passes the cwd-confinement check.
        let cwd = std::env::current_dir().expect("cannot get cwd");
        let source_path = cwd.join(format!("mdterm-test-source-{}.png", std::process::id()));
        let source_path_str = source_path.to_str().expect("cwd path is not UTF-8");
        {
            let img = image::DynamicImage::new_rgb8(8, 8);
            img.save_with_format(source_path_str, image::ImageFormat::Png)
                .expect("failed to write source fixture PNG");
        }

        let img = image::open(source_path_str).expect("failed to open fixture PNG");
        let metrics = CellMetrics {
            aspect: 2.0,
            cell_w_px: 8,
            cell_h_px: 16,
        };

        let priv_dir = std::env::temp_dir().join(format!("mdterm-{}", std::process::id()));
        let before_count = std::fs::read_dir(&priv_dir)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);

        let result = pre_render_terminology(&img, source_path_str, 80, metrics)
            .expect("pre_render_terminology returned None for a local path within cwd");

        let after_count = std::fs::read_dir(&priv_dir)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);

        // canonicalize may resolve symlinks (e.g. on macOS the cwd itself may
        // have a symlinked component). Compute canonical form for the assertion.
        let canonical_source =
            std::fs::canonicalize(source_path_str).unwrap_or_else(|_| source_path.clone());
        let _ = std::fs::remove_file(source_path_str);

        assert!(!result.is_temp, "local path must not set is_temp=true");
        assert_eq!(
            result.path,
            canonical_source
                .to_str()
                .expect("canonical path is not UTF-8"),
            "path must be the canonicalized local path"
        );
        assert_eq!(
            before_count, after_count,
            "pre_render_terminology must not create new temp files for a local path"
        );
    }

    // ── terminology_path_safe ────────────────────────────────────────────────

    /// Normal absolute paths that a real filesystem would produce must be accepted.
    #[test]
    fn path_safe_accepts_normal_paths() {
        assert!(terminology_path_safe("/home/user/images/photo.png"));
        assert!(terminology_path_safe("/tmp/mdterm-1234/img-abc123.png"));
        assert!(terminology_path_safe("/Users/alice/Documents/my-image.jpg"));
        // Multibyte UTF-8 is fine (e.g. non-ASCII directory names)
        assert!(terminology_path_safe("/home/用户/图片/photo.png"));
    }

    /// NUL byte must be rejected — it terminates the Terminology escape sequence early.
    /// Note: Rust `str` cannot contain interior NUL, so this tests the explicit check.
    #[test]
    fn path_safe_rejects_nul_via_control_check() {
        // \x01 is a control byte (< 0x20) — same category as NUL; if blocked, NUL is too.
        assert!(!terminology_path_safe("/tmp/foo\x01bar.png"));
    }

    /// ESC byte must be rejected — it could inject a second Terminology header.
    #[test]
    fn path_safe_rejects_esc() {
        assert!(!terminology_path_safe("/tmp/foo\x1bbar.png"));
        assert!(!terminology_path_safe("\x1b}ic#1;1;/etc/passwd"));
    }

    /// CR and LF must be rejected — could corrupt line-oriented terminal parsers.
    #[test]
    fn path_safe_rejects_cr_lf() {
        assert!(!terminology_path_safe("/tmp/foo\rbar.png"));
        assert!(!terminology_path_safe("/tmp/foo\nbar.png"));
    }

    /// DEL (0x7F) must be rejected.
    #[test]
    fn path_safe_rejects_del() {
        assert!(!terminology_path_safe("/tmp/foo\x7fbar.png"));
    }

    /// Semicolons corrupt the `<cols>;<rows>;<path>` framing.
    #[test]
    fn path_safe_rejects_semicolon() {
        assert!(!terminology_path_safe("/tmp/foo;bar.png"));
        assert!(!terminology_path_safe(";/etc/passwd"));
        assert!(!terminology_path_safe("/tmp/evil;0;/etc/passwd"));
    }

    /// All control bytes 0x01–0x1F must be rejected.
    #[test]
    fn path_safe_rejects_all_control_bytes() {
        for b in 0x01u8..0x20 {
            let path = format!("/tmp/foo{}bar.png", b as char);
            assert!(
                !terminology_path_safe(&path),
                "control byte 0x{b:02x} should be rejected"
            );
        }
    }

    // ── env save/restore helpers (used by Terminology detection tests) ─────────

    /// RAII guard that saves a set of environment variables on construction and
    /// restores them on drop. Callers must hold `ENV_LOCK` for the duration.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let saved = keys.iter().map(|&k| (k, std::env::var(k).ok())).collect();
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // Safety: callers hold ENV_LOCK mutex, preventing concurrent env mutation.
            unsafe {
                for (key, val) in &self.saved {
                    match val {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
}
