mod markdown;
mod style;
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

    let lines = markdown::render(&content);

    if std::io::stdout().is_terminal() {
        if let Err(e) = viewer::run(lines, path) {
            eprintln!("Viewer error: {}", e);
            process::exit(1);
        }
    } else {
        viewer::print_lines(&lines);
    }
}
