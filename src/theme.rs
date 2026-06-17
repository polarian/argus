//! Color themes. Gathers hardcoded colors into a semantic palette so presets can be swapped in.
//!
//! Selected via `theme = "<name>"` in the config file. Unknown names fall back to default.

use ratatui::style::Color;

/// Semantic color palette. All UI colors reference these fields.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Normal text (bold when emphasized).
    pub text: Color,
    /// Meta/dim text, unfocused borders.
    pub muted: Color,
    /// Focus, titles, numbers, primary emphasis.
    pub accent: Color,
    /// Success/done (✓), LIVE.
    pub success: Color,
    /// In progress/pending/warning.
    pub warning: Color,
    /// Failure/error (✗).
    pub error: Color,
    /// New/changed markers, labels.
    pub highlight: Color,
    /// Commit sha.
    pub sha: Color,
    /// Markdown links.
    pub link: Color,
    /// Code (inline/block).
    pub code: Color,
    /// Selected row background tint.
    pub sel: Color,
}

/// Selectable theme names (for help/validation).
pub const NAMES: [&str; 5] = [
    "default",
    "nord",
    "catppuccin-mocha",
    "dracula",
    "tokyo-night",
];

/// `0xRRGGBB` → `Color::Rgb`.
const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_theme()
    }
}

impl Theme {
    /// Pick a theme by name. Falls back to default if unknown.
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_lowercase().replace('_', "-").as_str() {
            "nord" => Self::nord(),
            "catppuccin" | "catppuccin-mocha" | "mocha" => Self::catppuccin_mocha(),
            "dracula" => Self::dracula(),
            "tokyo-night" | "tokyonight" => Self::tokyo_night(),
            _ => Self::default_theme(),
        }
    }

    /// Terminal default 16 colors — fits any terminal theme (keeps current colors).
    pub fn default_theme() -> Self {
        Self {
            text: Color::White,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            highlight: Color::Magenta,
            sha: Color::Yellow,
            link: Color::Blue,
            code: rgb(0xaaaaaa),
            sel: rgb(0x303030),
        }
    }

    fn nord() -> Self {
        Self {
            text: rgb(0xd8dee9),
            muted: rgb(0x4c566a),
            accent: rgb(0x88c0d0),
            success: rgb(0xa3be8c),
            warning: rgb(0xebcb8b),
            error: rgb(0xbf616a),
            highlight: rgb(0xb48ead),
            sha: rgb(0xd08770),
            link: rgb(0x81a1c1),
            code: rgb(0x8fbcbb),
            sel: rgb(0x3b4252),
        }
    }

    fn catppuccin_mocha() -> Self {
        Self {
            text: rgb(0xcdd6f4),
            muted: rgb(0x6c7086),
            accent: rgb(0x89dceb),
            success: rgb(0xa6e3a1),
            warning: rgb(0xf9e2af),
            error: rgb(0xf38ba8),
            highlight: rgb(0xcba6f7),
            sha: rgb(0xfab387),
            link: rgb(0x89b4fa),
            code: rgb(0x94e2d5),
            sel: rgb(0x313244),
        }
    }

    fn dracula() -> Self {
        Self {
            text: rgb(0xf8f8f2),
            muted: rgb(0x6272a4),
            accent: rgb(0x8be9fd),
            success: rgb(0x50fa7b),
            warning: rgb(0xf1fa8c),
            error: rgb(0xff5555),
            highlight: rgb(0xbd93f9),
            sha: rgb(0xffb86c),
            link: rgb(0x8be9fd),
            code: rgb(0x50fa7b),
            sel: rgb(0x44475a),
        }
    }

    fn tokyo_night() -> Self {
        Self {
            text: rgb(0xc0caf5),
            muted: rgb(0x565f89),
            accent: rgb(0x7dcfff),
            success: rgb(0x9ece6a),
            warning: rgb(0xe0af68),
            error: rgb(0xf7768e),
            highlight: rgb(0xbb9af7),
            sha: rgb(0xff9e64),
            link: rgb(0x7aa2f7),
            code: rgb(0x73daca),
            sel: rgb(0x292e42),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_resolve() {
        // All registered names resolve without panicking.
        for n in NAMES {
            let _ = Theme::from_name(n);
        }
    }

    #[test]
    fn unknown_falls_back_to_default() {
        let d = Theme::default_theme();
        let f = Theme::from_name("nonexistent");
        assert_eq!(d.accent, f.accent);
        assert_eq!(d.error, f.error);
    }

    #[test]
    fn aliases_and_case_insensitive() {
        // Case, aliases, and underscores are allowed.
        let a = Theme::from_name("Tokyo_Night");
        let b = Theme::from_name("tokyonight");
        assert_eq!(a.accent, b.accent);
    }
}
