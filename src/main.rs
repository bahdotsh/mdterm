mod markdown;
mod style;
mod theme;
mod viewer;

use std::io::IsTerminal;
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: mdterm <file.md>");
        process::exit(1);
    }

    let path = &args[1];
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading '{}': {}", path, e);
            process::exit(1);
        }
    };

    if std::io::stdout().is_terminal() {
        if let Err(e) = viewer::run(&content, path) {
            eprintln!("Viewer error: {}", e);
            process::exit(1);
        }
    } else {
        let width = crossterm::terminal::size()
            .map(|(c, _)| c as usize)
            .unwrap_or(80);
        let t = theme::Theme::dark();
        let lines = markdown::render(&content, width, &t);
        viewer::print_lines(&lines);
    }
}
