//! Overlay appearance: the app's theme and accent, mapped to the colours the
//! native overlay card draws with.
//!
//! The values mirror `src/theme/tokens.css` (a test below fails when the two
//! drift), so the overlay always matches the scheme chosen in Settings. This
//! module is toolkit-neutral: it produces plain colours.

/// Newest words of a live partial that are drawn at full strength.
pub(crate) const LIVE_TAIL_WORDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Scheme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Accent {
    #[default]
    Mono,
    Blue,
    Gold,
}

/// What the app window reports: the resolved theme and the accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct Appearance {
    pub(crate) scheme: Scheme,
    pub(crate) accent: Accent,
}

impl Appearance {
    /// Parse the two values the frontend sends. Anything else is refused, so
    /// a page can never inject style text into the overlay.
    pub(crate) fn parse(theme: &str, accent: &str) -> Option<Self> {
        let scheme = match theme {
            "dark" => Scheme::Dark,
            "light" => Scheme::Light,
            _ => return None,
        };
        let accent = match accent {
            "mono" => Accent::Mono,
            "blue" => Accent::Blue,
            "gold" => Accent::Gold,
            _ => return None,
        };
        Some(Self { scheme, accent })
    }

    pub(crate) fn palette(self) -> Palette {
        // The accent and its strong end: the two ends of the logo's gradient.
        let (accent, accent_strong) = match (self.scheme, self.accent) {
            (Scheme::Dark, Accent::Mono) => ("#f2f2f2", "#ffffff"),
            (Scheme::Dark, Accent::Blue) => ("#5b93ff", "#80abff"),
            (Scheme::Dark, Accent::Gold) => ("#d6b46a", "#e7c985"),
            (Scheme::Light, Accent::Mono) => ("#111111", "#000000"),
            (Scheme::Light, Accent::Blue) => ("#2459e0", "#1b47bd"),
            (Scheme::Light, Accent::Gold) => ("#866523", "#6e5219"),
        };
        match self.scheme {
            Scheme::Dark => Palette {
                surface_top: "#1a1a1a",
                surface_bottom: "#141414",
                border: "rgba(255, 255, 255, 0.14)",
                highlight: "rgba(255, 255, 255, 0.06)",
                shadow: "rgba(0, 0, 0, 0.50)",
                text_strong: "#ffffff",
                muted_strong: "#c4c4c4",
                muted: "#9a9a9a",
                accent,
                accent_strong,
                live: "#ff6b5e",
                err: "#ff7a74",
            },
            Scheme::Light => Palette {
                surface_top: "#ffffff",
                surface_bottom: "#f7f6f3",
                border: "rgba(10, 10, 10, 0.15)",
                highlight: "rgba(255, 255, 255, 0.70)",
                shadow: "rgba(10, 10, 10, 0.18)",
                text_strong: "#0a0a0a",
                muted_strong: "#3f3f3f",
                muted: "#6b6b6b",
                accent,
                accent_strong,
                live: "#cc2d20",
                err: "#c2392f",
            },
        }
    }
}

/// Where on the screen the overlay sits. Bottom centre unless the user picks
/// another place in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Horizontal {
    Left,
    Center,
    Right,
}

impl Position {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "top-left" => Self::TopLeft,
            "top-center" => Self::TopCenter,
            "top-right" => Self::TopRight,
            "bottom-left" => Self::BottomLeft,
            "bottom-center" => Self::BottomCenter,
            "bottom-right" => Self::BottomRight,
            _ => return None,
        })
    }

    pub(crate) fn at_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopCenter | Self::TopRight)
    }

    pub(crate) fn horizontal(self) -> Horizontal {
        match self {
            Self::TopLeft | Self::BottomLeft => Horizontal::Left,
            Self::TopCenter | Self::BottomCenter => Horizontal::Center,
            Self::TopRight | Self::BottomRight => Horizontal::Right,
        }
    }
}

/// Colours for one appearance. Values are `#rrggbb` or `rgba(r, g, b, a)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Palette {
    pub(crate) surface_top: &'static str,
    pub(crate) surface_bottom: &'static str,
    pub(crate) border: &'static str,
    pub(crate) highlight: &'static str,
    pub(crate) shadow: &'static str,
    pub(crate) text_strong: &'static str,
    pub(crate) muted_strong: &'static str,
    pub(crate) muted: &'static str,
    pub(crate) accent: &'static str,
    pub(crate) accent_strong: &'static str,
    pub(crate) live: &'static str,
    pub(crate) err: &'static str,
}

/// A palette colour as cairo components with alpha: `#rrggbb` is opaque,
/// `rgba(r, g, b, a)` carries its own alpha. A malformed value draws
/// mid-grey rather than failing: the palette is static and covered by tests.
pub(crate) fn rgba(value: &str) -> (f64, f64, f64, f64) {
    if let Some(inner) = value
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<f64> = inner
            .split(',')
            .filter_map(|part| part.trim().parse::<f64>().ok())
            .collect();
        if let [red, green, blue, alpha] = parts[..] {
            return (red / 255.0, green / 255.0, blue / 255.0, alpha);
        }
        return (0.5, 0.5, 0.5, 1.0);
    }
    let (red, green, blue) = rgb(value);
    (red, green, blue, 1.0)
}

/// `#rrggbb` as cairo components. A malformed value draws mid-grey rather
/// than failing: the palette is static and covered by tests.
pub(crate) fn rgb(hex: &str) -> (f64, f64, f64) {
    let channel = |at: usize| {
        hex.get(at..at + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .map_or(0.5, |value| f64::from(value) / 255.0)
    };
    (channel(1), channel(3), channel(5))
}

/// Byte offset where the last `words` words of `text` begin (0 when the text
/// has no more than that many). Used to draw the newest words at full
/// strength.
pub(crate) fn tail_start(text: &str, words: usize) -> usize {
    let mut seen = 0;
    let mut in_word = false;
    for (index, character) in text.char_indices().rev() {
        if character.is_whitespace() {
            if in_word {
                seen += 1;
                if seen == words {
                    return index + character.len_utf8();
                }
                in_word = false;
            }
        } else {
            in_word = true;
        }
    }
    0
}

/// `m:ss` for the listening timer.
pub(crate) fn elapsed_label(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKENS: &str = include_str!("../../src/theme/tokens.css");

    /// The text from `start` up to the next `end`; empty when either is
    /// missing, which fails the assertions below.
    fn block(start: &str, end: &str) -> &'static str {
        let rest = TOKENS.find(start).map_or("", |from| &TOKENS[from..]);
        rest.find(end).map_or("", |to| &rest[..to])
    }

    #[test]
    fn palette_matches_the_app_design_tokens() {
        let dark = block(":root,", "html.light {");
        let light = block("html.light {", "/* Accent choices.");
        let cases = [
            (
                Scheme::Dark,
                dark,
                "html.dark[data-accent='blue']",
                "html.dark[data-accent='gold']",
            ),
            (
                Scheme::Light,
                light,
                "html.light[data-accent='blue']",
                "html.light[data-accent='gold']",
            ),
        ];
        for (scheme, base, blue, gold) in cases {
            let palette = Appearance {
                scheme,
                accent: Accent::Mono,
            }
            .palette();
            for (token, value) in [
                ("--surface-raised", palette.surface_top),
                ("--text-strong", palette.text_strong),
                ("--muted-strong", palette.muted_strong),
                ("--muted", palette.muted),
                ("--accent", palette.accent),
                ("--accent-strong", palette.accent_strong),
                ("--live", palette.live),
                ("--err", palette.err),
                ("--border-strong", palette.border),
            ] {
                assert!(
                    base.contains(&format!("{token}: {value};")),
                    "{scheme:?} {token} differs from tokens.css"
                );
            }
            for (accent, selector) in [(Accent::Blue, blue), (Accent::Gold, gold)] {
                let palette = Appearance { scheme, accent }.palette();
                let rules = block(selector, "}");
                assert!(
                    rules.contains(&format!("--accent: {};", palette.accent)),
                    "{scheme:?} {accent:?} accent differs from tokens.css"
                );
                assert!(
                    rules.contains(&format!("--accent-strong: {};", palette.accent_strong)),
                    "{scheme:?} {accent:?} strong accent differs from tokens.css"
                );
            }
        }
    }

    #[test]
    fn appearance_accepts_only_known_values() {
        assert_eq!(
            Appearance::parse("light", "gold"),
            Some(Appearance {
                scheme: Scheme::Light,
                accent: Accent::Gold
            })
        );
        assert_eq!(
            Appearance::parse("dark", "mono"),
            Some(Appearance::default())
        );
        assert_eq!(Appearance::parse("system", "mono"), None);
        assert_eq!(Appearance::parse("dark", "red; } * { color: red"), None);
    }

    #[test]
    fn position_defaults_to_bottom_centre_and_parses_the_six_places() {
        assert_eq!(Position::default(), Position::BottomCenter);
        let places = [
            ("top-left", true, Horizontal::Left),
            ("top-center", true, Horizontal::Center),
            ("top-right", true, Horizontal::Right),
            ("bottom-left", false, Horizontal::Left),
            ("bottom-center", false, Horizontal::Center),
            ("bottom-right", false, Horizontal::Right),
        ];
        for (name, top, horizontal) in places {
            let position = Position::parse(name);
            assert_eq!(position.map(Position::at_top), Some(top), "{name}");
            assert_eq!(
                position.map(Position::horizontal),
                Some(horizontal),
                "{name}"
            );
        }
        assert_eq!(Position::parse("middle"), None);
    }

    #[test]
    fn tail_start_marks_the_newest_words() {
        assert_eq!(tail_start("", 3), 0);
        assert_eq!(tail_start("one two", 3), 0);
        let text = "move the review to Thursday";
        assert_eq!(&text[tail_start(text, 3)..], "review to Thursday");
        let spaced = "move the review to  Thursday  ";
        assert_eq!(&spaced[tail_start(spaced, 2)..], "to  Thursday  ");
        let accented = "déjà vu encore une fois";
        assert_eq!(&accented[tail_start(accented, 3)..], "encore une fois");
    }

    #[test]
    fn colours_and_timer_format() {
        assert_eq!(rgb("#ffffff"), (1.0, 1.0, 1.0));
        assert_eq!(rgb("#000000"), (0.0, 0.0, 0.0));
        assert_eq!(rgb("nope"), (0.5, 0.5, 0.5));
        assert_eq!(rgba("#000000"), (0.0, 0.0, 0.0, 1.0));
        assert_eq!(rgba("rgba(255, 0, 255, 0.5)"), (1.0, 0.0, 1.0, 0.5));
        assert_eq!(rgba("rgba(1, 2)"), (0.5, 0.5, 0.5, 1.0));
        assert_eq!(elapsed_label(0), "0:00");
        assert_eq!(elapsed_label(75), "1:15");
    }
}
