mod config;
mod diagram;
mod export;
mod image;
mod markdown;
mod style;
mod theme;
mod viewer;

use std::io::{self, IsTerminal, Read};
use std::{fs, process};

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "mdterm",
    version,
    about = "Terminal Markdown viewer with style"
)]
struct Cli {
    /// Markdown file(s) to view
    files: Vec<String>,

    /// Theme: dark or light
    #[arg(long, short = 'T')]
    theme: Option<String>,

    /// Display width override (0 = auto)
    #[arg(long, short = 'w', default_value = "0")]
    width: usize,

    /// Slide mode (horizontal rules become slide separators)
    #[arg(long, short = 's')]
    slides: bool,

    /// Watch file for changes and auto-reload
    #[arg(long, short = 'f')]
    follow: bool,

    /// Show line numbers in code blocks
    #[arg(long, short = 'l')]
    line_numbers: bool,

    /// Export format instead of interactive view (html)
    #[arg(long)]
    export: Option<String>,

    /// Disable colors
    #[arg(long)]
    no_color: bool,
}

fn main() {
    let cli = Cli::parse();
    let config = config::Config::load();

    // Determine theme
    let theme_name = cli.theme.as_deref().unwrap_or(&config.theme);
    let initial_theme = match theme_name {
        "light" => theme::Theme::light(),
        _ => theme::Theme::dark(),
    };

    let line_numbers = cli.line_numbers || config.line_numbers;
    let width = if cli.width > 0 {
        cli.width
    } else if config.width > 0 {
        config.width
    } else {
        0
    };

    // Read content: stdin or file(s)
    let (content, filename) = if cli.files.is_empty() {
        if io::stdin().is_terminal() {
            eprintln!("Usage: mdterm [OPTIONS] <FILE>...");
            eprintln!("       command | mdterm");
            eprintln!();
            eprintln!("Try 'mdterm --help' for more information.");
            process::exit(1);
        }
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf).unwrap_or_else(|e| {
            eprintln!("Error reading stdin: {}", e);
            process::exit(1);
        });
        (buf, "<stdin>".to_string())
    } else {
        let path = &cli.files[0];
        let c = fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Error reading '{}': {}", path, e);
            process::exit(1);
        });
        (c, path.clone())
    };

    // Export mode
    if let Some(ref fmt) = cli.export {
        match fmt.as_str() {
            "html" => {
                let w = if width > 0 { width } else { 80 };
                export::to_html(&content, w, &initial_theme);
            }
            _ => {
                eprintln!("Unknown export format '{}'. Supported: html", fmt);
                process::exit(1);
            }
        }
        return;
    }

    // Interactive or piped
    if io::stdout().is_terminal() && !cli.no_color {
        let opts = viewer::ViewerOptions {
            files: cli.files,
            initial_content: content,
            filename,
            theme: initial_theme,
            slide_mode: cli.slides,
            follow_mode: cli.follow,
            line_numbers,
            width_override: if width > 0 { Some(width) } else { None },
        };
        if let Err(e) = viewer::run(opts) {
            eprintln!("Viewer error: {}", e);
            process::exit(1);
        }
    } else {
        let w = if width > 0 {
            width
        } else {
            crossterm::terminal::size()
                .map(|(c, _)| c as usize)
                .unwrap_or(80)
        };
        let (lines, _) = markdown::render(&content, w, &initial_theme, line_numbers);
        let wrapped = style::wrap_lines(&lines, w);
        if cli.no_color {
            viewer::print_lines_plain(&wrapped);
        } else {
            viewer::print_lines(&wrapped);
        }
    }
}
