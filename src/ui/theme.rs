//! Colors, glyphs, and small formatting helpers shared across the UI.

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use tokei::LanguageType;

use crate::model::Stats;

// Tree glyphs (Unicode, renders everywhere).
pub const GLYPH_EXPANDED: &str = "▾";
pub const GLYPH_COLLAPSED: &str = "▸";
pub const GLYPH_FILE: &str = "•";

/// Braille spinner frames for the loading screen.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// Semantic palette (Catppuccin-ish).
pub const ACCENT: Color = Color::Rgb(137, 180, 250);
pub const DIR: Color = Color::Rgb(137, 180, 250);
pub const MUTED: Color = Color::Rgb(127, 132, 156);
pub const SELECTION_BG: Color = Color::Rgb(49, 50, 68);
pub const WARN: Color = Color::Rgb(249, 226, 175);
pub const BADGE_FG: Color = Color::Rgb(17, 17, 27);

// Composition-bar segment colors.
pub const CODE: Color = Color::Rgb(166, 209, 137);
pub const COMMENTS: Color = Color::Rgb(125, 166, 255);
pub const BLANKS: Color = Color::Rgb(88, 91, 112);

/// A recognizable color for popular languages; a stable hashed color otherwise.
pub fn language_color(lang: LanguageType) -> Color {
    use LanguageType as L;
    match lang {
        L::Rust => Color::Rgb(222, 165, 132),
        L::Python => Color::Rgb(75, 139, 190),
        L::JavaScript | L::Jsx => Color::Rgb(240, 219, 79),
        L::TypeScript | L::Tsx => Color::Rgb(49, 120, 198),
        L::Go => Color::Rgb(0, 173, 216),
        L::C | L::CHeader => Color::Rgb(120, 140, 165),
        L::Cpp | L::CppHeader => Color::Rgb(243, 75, 125),
        L::CSharp => Color::Rgb(150, 90, 190),
        L::Java => Color::Rgb(176, 114, 25),
        L::Kotlin => Color::Rgb(169, 123, 255),
        L::Ruby => Color::Rgb(204, 52, 45),
        L::Php => Color::Rgb(119, 123, 180),
        L::Html => Color::Rgb(228, 77, 38),
        L::Css | L::Sass | L::Less => Color::Rgb(150, 120, 200),
        L::Swift => Color::Rgb(240, 81, 56),
        L::Scala => Color::Rgb(199, 42, 32),
        L::Haskell => Color::Rgb(140, 120, 180),
        L::OCaml => Color::Rgb(238, 106, 26),
        L::Elixir => Color::Rgb(150, 116, 170),
        L::Erlang => Color::Rgb(168, 47, 55),
        L::Clojure => Color::Rgb(99, 179, 60),
        L::Lua => Color::Rgb(120, 120, 220),
        L::Markdown => Color::Rgb(150, 150, 150),
        L::Json | L::Yaml | L::Toml | L::Xml => Color::Rgb(180, 142, 173),
        L::Sql => Color::Rgb(227, 131, 38),
        L::Sh | L::Bash | L::Zsh => Color::Rgb(137, 224, 81),
        L::Dockerfile => Color::Rgb(60, 150, 220),
        L::Vue => Color::Rgb(65, 184, 131),
        L::Svelte => Color::Rgb(255, 62, 0),
        L::Zig => Color::Rgb(247, 164, 29),
        L::Dart => Color::Rgb(0, 180, 171),
        L::R => Color::Rgb(100, 150, 220),
        L::Julia => Color::Rgb(150, 110, 200),
        _ => hashed_color(lang.name()),
    }
}

fn hashed_color(name: &str) -> Color {
    // FNV-1a over the language name → a cell in the 6×6×6 color cube, skipping
    // the darkest rows so text stays legible.
    let mut hash: u32 = 2_166_136_261;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    let index = 17 + (hash % (231 - 17));
    Color::Indexed(index as u8)
}

/// A human-friendly display name (tokei's `name()` is SCREAMING-cased).
pub fn language_label(lang: LanguageType) -> String {
    use LanguageType as L;
    let curated = match lang {
        L::Rust => "Rust",
        L::Python => "Python",
        L::JavaScript => "JavaScript",
        L::Jsx => "JSX",
        L::TypeScript => "TypeScript",
        L::Tsx => "TSX",
        L::Go => "Go",
        L::C => "C",
        L::CHeader => "C Header",
        L::Cpp => "C++",
        L::CppHeader => "C++ Header",
        L::CSharp => "C#",
        L::Java => "Java",
        L::Kotlin => "Kotlin",
        L::Ruby => "Ruby",
        L::Php => "PHP",
        L::Html => "HTML",
        L::Css => "CSS",
        L::Sass => "Sass",
        L::Less => "Less",
        L::Swift => "Swift",
        L::Scala => "Scala",
        L::Haskell => "Haskell",
        L::OCaml => "OCaml",
        L::Elixir => "Elixir",
        L::Erlang => "Erlang",
        L::Clojure => "Clojure",
        L::Lua => "Lua",
        L::Markdown => "Markdown",
        L::Json => "JSON",
        L::Yaml => "YAML",
        L::Toml => "TOML",
        L::Xml => "XML",
        L::Sql => "SQL",
        L::Sh => "Shell",
        L::Bash => "Bash",
        L::Zsh => "Zsh",
        L::Dockerfile => "Dockerfile",
        L::Vue => "Vue",
        L::Svelte => "Svelte",
        L::Zig => "Zig",
        L::Dart => "Dart",
        _ => return prettify(lang.name()),
    };
    curated.to_string()
}

/// Title-case a SCREAMING name like `"AWK"` → `"Awk"` for uncurated languages.
fn prettify(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// Group an integer with thousands separators: `12345` → `"12,345"`.
pub fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

/// A `width`-cell horizontal bar showing the code/comment/blank composition.
pub fn composition_bar(stats: &Stats, width: usize) -> Vec<Span<'static>> {
    let total = stats.lines();
    if total == 0 || width == 0 {
        return vec![Span::styled("░".repeat(width), Style::default().fg(MUTED))];
    }
    let code = (stats.code * width / total).min(width);
    let comments = (stats.comments * width / total).min(width - code);
    let blanks = width - code - comments;

    let mut spans = Vec::new();
    let mut push = |count: usize, color: Color| {
        if count > 0 {
            spans.push(Span::styled("█".repeat(count), Style::default().fg(color)));
        }
    };
    push(code, CODE);
    push(comments, COMMENTS);
    push(blanks, BLANKS);
    spans
}
