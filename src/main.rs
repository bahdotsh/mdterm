mod config;
mod diagram;
mod export;
mod file_picker;
mod image;
mod json;
mod markdown;
mod style;
mod theme;
mod viewer;

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
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

    /// Deprecated: file watching is now always active
    #[arg(long, short = 'f', hide = true)]
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
    let stdout_is_terminal = io::stdout().is_terminal();
    let stdin_is_terminal = io::stdin().is_terminal();
    let interactive = stdout_is_terminal && !cli.no_color;

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

    // Read content: stdin, file(s), or prepare an interactive directory picker.
    let mut picker_root: Option<PathBuf> = None;
    let mut start_in_picker = false;
    let (content, filename, files) = if cli.files.is_empty() {
        if stdin_is_terminal {
            if interactive && cli.export.is_none() {
                let root = current_dir_or_exit();
                let filename = file_picker::display_path(&root);
                picker_root = Some(root);
                start_in_picker = true;
                (String::new(), filename, Vec::new())
            } else {
                eprintln!("Usage: mdterm [OPTIONS] <FILE|DIRECTORY>...");
                eprintln!("       command | mdterm");
                eprintln!();
                eprintln!("Try 'mdterm --help' for more information.");
                process::exit(1);
            }
        } else {
            let content = read_stdin_or_exit();
            (content, "<stdin>".to_string(), Vec::new())
        }
    } else if Path::new(&cli.files[0]).is_dir() {
        if !interactive || cli.export.is_some() {
            eprintln!("Error: directory picker requires an interactive terminal");
            process::exit(1);
        }
        let root = Path::new(&cli.files[0])
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&cli.files[0]));
        let filename = file_picker::display_path(&root);
        picker_root = Some(root);
        start_in_picker = true;
        (String::new(), filename, Vec::new())
    } else {
        let path = &cli.files[0];
        let c = fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Error reading '{}': {}", path, e);
            process::exit(1);
        });
        (c, path.clone(), cli.files.clone())
    };

    let is_json = filename.ends_with(".json");

    // Export mode
    if let Some(ref fmt) = cli.export {
        match fmt.as_str() {
            "html" => {
                let w = if width > 0 { width } else { 80 };
                export::to_html(&content, w, &initial_theme, &filename);
            }
            _ => {
                eprintln!("Unknown export format '{}'. Supported: html", fmt);
                process::exit(1);
            }
        }
        return;
    }

    // Interactive or piped
    if interactive {
        let opts = viewer::ViewerOptions {
            files,
            initial_content: content,
            filename,
            theme: initial_theme,
            slide_mode: cli.slides,
            line_numbers,
            width_override: if width > 0 { Some(width) } else { None },
            picker_root,
            start_in_picker,
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
        let (lines, _) = if is_json {
            match json::render(&content, w, &initial_theme) {
                Ok(result) => result,
                Err(e) => {
                    eprintln!("JSON parse error: {}", e);
                    process::exit(1);
                }
            }
        } else {
            markdown::render(&content, w, &initial_theme, line_numbers)
        };
        let wrapped = style::wrap_lines(&lines, w);
        if cli.no_color {
            viewer::print_lines_plain(&wrapped);
        } else {
            viewer::print_lines(&wrapped);
        }
    }
}

fn read_stdin_or_exit() -> String {
    const MAX_STDIN_BYTES: u64 = 100 * 1024 * 1024; // 100 MB
    let mut buf = String::new();
    let n = io::stdin()
        .take(MAX_STDIN_BYTES + 1)
        .read_to_string(&mut buf)
        .unwrap_or_else(|e| {
            eprintln!("Error reading stdin: {}", e);
            process::exit(1);
        });
    if n as u64 > MAX_STDIN_BYTES {
        eprintln!("Error: stdin input exceeds 100 MB limit");
        process::exit(1);
    }
    buf
}

fn current_dir_or_exit() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error reading current directory: {}", e);
        process::exit(1);
    })
}
