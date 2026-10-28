//! Prints a QR code in the terminal, two modules per character cell.
//!
//! Phone cameras want dark modules on a light background. Where the terminal takes ANSI colours the
//! code is drawn black on white whatever the theme; otherwise it assumes a dark terminal and draws
//! the light modules, so the terminal's own background shows through as the dark ones.

use qrcode::{Color, QrCode};

const QUIET_ZONE: usize = 2;

pub fn render(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + QUIET_ZONE * 2;
    let dark = |x: usize, y: usize| -> bool {
        if x < QUIET_ZONE || y < QUIET_ZONE || x >= width + QUIET_ZONE || y >= width + QUIET_ZONE {
            return false;
        }
        colors[(y - QUIET_ZONE) * width + (x - QUIET_ZONE)] == Color::Dark
    };

    let ansi = ansi_supported();
    let mut out = String::new();
    for y in (0..size).step_by(2) {
        out.push_str("  ");
        if ansi {
            out.push_str("\x1b[30;107m");
        }
        for x in 0..size {
            let top = dark(x, y);
            let bottom = y + 1 < size && dark(x, y + 1);
            // With colours, ink is the dark modules; without, ink is the light ones
            let (top, bottom) = if ansi {
                (top, bottom)
            } else {
                (!top, !(bottom || y + 1 >= size))
            };
            out.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        if ansi {
            out.push_str("\x1b[0m");
        }
        out.push('\n');
    }
    Some(out)
}

fn ansi_supported() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if cfg!(windows) {
        // Windows Terminal and VS Code take ANSI; the old console host may not
        return std::env::var_os("WT_SESSION").is_some()
            || std::env::var("TERM_PROGRAM")
                .map(|t| t == "vscode")
                .unwrap_or(false);
    }
    std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_a_square() {
        let qr = super::render("https://claudecodenotifier.com/c/XtibiarAdIlBbjZ31LaqsA").unwrap();
        let lines: Vec<&str> = qr.lines().collect();
        assert!(lines.len() > 10);
    }
}
