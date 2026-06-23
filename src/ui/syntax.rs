//! Syntax highlighting for the text preview (bat-like).
//!
//! Uses syntect with its pure-Rust `fancy-regex` backend, so there is no C
//! oniguruma dependency. The syntax and theme sets are loaded once and cached.

use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

struct Assets {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

/// The default syntax + theme assets, loaded once on first use.
fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| Assets {
        syntaxes: SyntaxSet::load_defaults_newlines(),
        themes: ThemeSet::load_defaults(),
    })
}

/// Highlight `text` (already truncated by the caller) into owned ratatui lines.
///
/// The syntax is chosen from `path`'s extension, falling back to the first line,
/// then to plain text. Any per-line highlighting error degrades that line to
/// unstyled text rather than failing the whole preview.
pub fn highlight(text: &str, path: &Path) -> Vec<Line<'static>> {
    let assets = assets();
    let theme = &assets.themes.themes["base16-ocean.dark"];
    let syntax = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| assets.syntaxes.find_syntax_by_extension(ext))
        .or_else(|| {
            text.lines()
                .next()
                .and_then(|first| assets.syntaxes.find_syntax_by_first_line(first))
        })
        .unwrap_or_else(|| assets.syntaxes.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    LinesWithEndings::from(text)
        .map(
            |line| match highlighter.highlight_line(line, &assets.syntaxes) {
                Ok(ranges) => to_line(&ranges),
                Err(_) => Line::from(line.trim_end_matches(['\n', '\r']).to_string()),
            },
        )
        .collect()
}

/// Convert one highlighted line's `(style, slice)` ranges into a ratatui line,
/// dropping the trailing newline that `LinesWithEndings` keeps.
fn to_line(ranges: &[(SynStyle, &str)]) -> Line<'static> {
    let spans: Vec<Span<'static>> = ranges
        .iter()
        .map(|(style, piece)| {
            let text = piece.trim_end_matches(['\n', '\r']).to_string();
            Span::styled(text, to_style(*style))
        })
        .collect();
    Line::from(spans)
}

/// Map a syntect style to a ratatui style (foreground RGB + font modifiers).
fn to_style(style: SynStyle) -> Style {
    let fg = style.foreground;
    let mut out = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_into_styled_spans() {
        let lines = highlight("fn main() {}\n", Path::new("main.rs"));
        assert_eq!(lines.len(), 1);
        // syntect assigns the keyword/identifier distinct foreground colors.
        let colors: Vec<_> = lines[0].spans.iter().filter_map(|s| s.style.fg).collect();
        assert!(colors.len() > 1, "expected multiple colors, got {colors:?}");
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        let lines = highlight("hello world\n", Path::new("notes.unknownext"));
        assert_eq!(lines.len(), 1);
    }
}
