//! Colours. The terminal keeps its own background; only foregrounds are set. Brand: signal lime
//! `#C8F25A` on dark, `#4D7A00` on light (docs/BRAND.md). Meaning is never colour-only.
use klip_core::KlipPaths;
use ratatui::style::{Color, Modifier, Style};

/// What the TUI needs to know about its surroundings.
#[derive(Debug, Clone)]
pub struct TuiEnv {
    pub paths: KlipPaths,
    pub no_color: bool,
    pub truecolor: bool,
    pub colors256: bool,
    pub light: bool,
    pub mouse: bool,
}

impl TuiEnv {
    pub fn from_process(paths: KlipPaths) -> Self {
        Self::from_vars(paths, |k| std::env::var(k).ok())
    }

    pub fn from_vars(paths: KlipPaths, get: impl Fn(&str) -> Option<String>) -> Self {
        let no_color = get("NO_COLOR").map(|v| !v.is_empty()).unwrap_or(false);
        let colorterm = get("COLORTERM").unwrap_or_default().to_lowercase();
        let term = get("TERM").unwrap_or_default();
        let truecolor = colorterm.contains("truecolor") || colorterm.contains("24bit");
        let colors256 = truecolor || term.contains("256color");
        let light = get("KLIP_THEME")
            .map(|v| v.eq_ignore_ascii_case("light"))
            .unwrap_or(false);
        Self {
            paths,
            no_color,
            truecolor,
            colors256,
            light,
            mouse: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub signal: Color,
    pub warn: Color,
    pub bad: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    /// `NO_COLOR`: no colours at all; emphasis comes from bold/dim/reverse only.
    pub plain: bool,
}

impl Theme {
    pub fn from_env(env: &TuiEnv) -> Self {
        if env.no_color {
            return Self {
                signal: Color::Reset,
                warn: Color::Reset,
                bad: Color::Reset,
                text: Color::Reset,
                dim: Color::Reset,
                faint: Color::Reset,
                plain: true,
            };
        }
        let (signal, warn, bad, text, dim, faint) = if env.truecolor {
            if env.light {
                (
                    Color::Rgb(77, 122, 0),
                    Color::Rgb(168, 95, 0),
                    Color::Rgb(179, 38, 30),
                    Color::Rgb(18, 22, 26),
                    Color::Rgb(90, 100, 107),
                    Color::Rgb(140, 151, 159),
                )
            } else {
                (
                    Color::Rgb(200, 242, 90),
                    Color::Rgb(255, 184, 77),
                    Color::Rgb(255, 107, 107),
                    Color::Rgb(232, 237, 240),
                    Color::Rgb(140, 151, 159),
                    Color::Rgb(90, 100, 107),
                )
            }
        } else if env.colors256 {
            if env.light {
                (
                    Color::Indexed(64),
                    Color::Indexed(130),
                    Color::Indexed(124),
                    Color::Indexed(234),
                    Color::Indexed(240),
                    Color::Indexed(245),
                )
            } else {
                (
                    Color::Indexed(192),
                    Color::Indexed(215),
                    Color::Indexed(203),
                    Color::Indexed(255),
                    Color::Indexed(245),
                    Color::Indexed(240),
                )
            }
        } else {
            (
                Color::Green,
                Color::Yellow,
                Color::Red,
                Color::Reset,
                Color::DarkGray,
                Color::DarkGray,
            )
        };
        Self {
            signal,
            warn,
            bad,
            text,
            dim,
            faint,
            plain: false,
        }
    }

    fn fg(&self, c: Color, plain_mods: Modifier) -> Style {
        if self.plain {
            Style::default().add_modifier(plain_mods)
        } else {
            Style::default().fg(c)
        }
    }

    pub fn st_signal(&self) -> Style {
        self.fg(self.signal, Modifier::BOLD)
    }
    pub fn st_warn(&self) -> Style {
        self.fg(self.warn, Modifier::BOLD)
    }
    pub fn st_bad(&self) -> Style {
        self.fg(self.bad, Modifier::BOLD | Modifier::REVERSED)
    }
    pub fn st_text(&self) -> Style {
        self.fg(self.text, Modifier::empty())
    }
    pub fn st_dim(&self) -> Style {
        self.fg(self.dim, Modifier::DIM)
    }
    pub fn st_faint(&self) -> Style {
        self.fg(self.faint, Modifier::DIM)
    }
    /// The selected row of a list.
    pub fn st_selected(&self) -> Style {
        if self.plain {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
                .fg(self.signal)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        }
    }
    pub fn st_title(&self) -> Style {
        self.st_signal().add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klip_core::KlipPaths;
    use ratatui::style::Color;

    fn env(vars: &[(&str, &str)]) -> TuiEnv {
        let dir = std::env::temp_dir().join("klip-tui-theme-test");
        TuiEnv::from_vars(KlipPaths::from_dir(dir), |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn truecolor_uses_the_brand_colours() {
        let t = Theme::from_env(&env(&[
            ("COLORTERM", "truecolor"),
            ("TERM", "xterm-256color"),
        ]));
        assert_eq!(t.signal, Color::Rgb(200, 242, 90));
        assert!(!t.plain);
    }

    #[test]
    fn light_theme_uses_the_dark_green_signal() {
        let t = Theme::from_env(&env(&[("COLORTERM", "24bit"), ("KLIP_THEME", "light")]));
        assert_eq!(t.signal, Color::Rgb(77, 122, 0));
    }

    #[test]
    fn a_256_colour_terminal_gets_indexed_colours() {
        let t = Theme::from_env(&env(&[("TERM", "xterm-256color")]));
        assert_eq!(t.signal, Color::Indexed(192));
    }

    #[test]
    fn a_basic_terminal_gets_the_16_colours() {
        let t = Theme::from_env(&env(&[("TERM", "xterm")]));
        assert_eq!(t.signal, Color::Green);
    }

    #[test]
    fn no_color_means_plain_and_wins_over_everything() {
        let t = Theme::from_env(&env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]));
        assert!(t.plain);
        assert_eq!(t.signal, Color::Reset);
        let t = Theme::from_env(&env(&[("NO_COLOR", ""), ("COLORTERM", "truecolor")]));
        assert!(!t.plain, "an empty NO_COLOR does not count");
    }
}
