//! The tray icons: the Klip mark (two devices joined by a line) as a monochrome symbolic icon.
//!
//! Like the other panel icons they have no colour of their own; the desktop paints them in the
//! panel's foreground colour. The state is the shape: a solid line = connected, dots = waiting, bars =
//! paused, faded = no device or stopped. A solid dot in the corner = an image waits to be sent.
use super::model::State;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const STATES: [State; 5] = [
    State::Connected,
    State::Waiting,
    State::NoDevices,
    State::Paused,
    State::Stopped,
];

/// A 64x64 symbolic SVG (fills only: the theme recolours fills, not strokes).
pub fn svg(state: State, badge: bool) -> String {
    let mut parts = String::new();
    match state {
        State::Connected => {
            parts.push_str(r#"<rect x="14" y="29.5" width="36" height="5" rx="2.5"/>"#)
        }
        State::Waiting => {
            for x in [26, 32, 38] {
                parts.push_str(&format!(r#"<circle cx="{x}" cy="32" r="2.6"/>"#));
            }
        }
        State::Paused => parts.push_str(
            r#"<rect x="26.5" y="23" width="4.5" height="18" rx="2"/><rect x="33" y="23" width="4.5" height="18" rx="2"/>"#,
        ),
        State::NoDevices | State::Stopped => {}
    }
    for x in [14, 50] {
        parts.push_str(&format!(r#"<circle cx="{x}" cy="32" r="9"/>"#));
    }
    let faded = match state {
        State::NoDevices => Some("0.55"),
        State::Stopped => Some("0.3"),
        _ => None,
    };
    let mut body = match faded {
        Some(opacity) => format!(r#"<g opacity="{opacity}">{parts}</g>"#),
        None => parts,
    };
    if badge {
        body.push_str(r#"<circle cx="52" cy="12" r="10"/>"#);
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">{body}</svg>"#
    )
}

pub fn name_for(state: State, badge: bool) -> String {
    format!(
        "klip-{}{}-symbolic",
        state.key(),
        if badge { "-badge" } else { "" }
    )
}

/// `<data home>/icons/hicolor/scalable/status`: where the panel (another process) finds them.
pub fn directory(data_home: &Path) -> PathBuf {
    data_home.join("icons/hicolor/scalable/status")
}

/// Write every icon; a file that already has the right content is left alone.
pub fn install(data_home: &Path) -> io::Result<()> {
    let dir = directory(data_home);
    fs::create_dir_all(&dir)?;
    for state in STATES {
        for badge in [false, true] {
            let path = dir.join(format!("{}.svg", name_for(state, badge)));
            let content = svg(state, badge);
            if fs::read_to_string(&path).is_ok_and(|c| c == content) {
                continue;
            }
            let tmp = dir.join(format!(
                ".{}.{}.tmp",
                name_for(state, badge),
                std::process::id()
            ));
            fs::write(&tmp, &content)?;
            fs::rename(&tmp, &path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_fills_only_and_has_no_colour_of_its_own() {
        for state in STATES {
            for badge in [false, true] {
                let text = svg(state, badge);
                assert!(text.starts_with("<svg ") && text.ends_with("</svg>"));
                assert!(
                    !text.contains("stroke"),
                    "symbolic icons are recoloured through fills"
                );
                assert!(!text.contains('#'), "the panel paints it");
                assert_eq!(text.matches('<').count(), text.matches('>').count());
            }
        }
    }

    #[test]
    fn the_states_look_different_and_the_badge_shows() {
        let shapes: std::collections::HashSet<String> =
            STATES.iter().map(|s| svg(*s, false)).collect();
        assert_eq!(shapes.len(), STATES.len());
        assert_ne!(svg(State::Connected, false), svg(State::Connected, true));
    }

    #[test]
    fn names_are_symbolic() {
        assert_eq!(name_for(State::Connected, false), "klip-connected-symbolic");
        assert_eq!(name_for(State::Paused, true), "klip-paused-badge-symbolic");
        assert_eq!(
            name_for(State::NoDevices, false),
            "klip-no_devices-symbolic"
        );
    }

    #[test]
    fn install_writes_every_icon_once_and_leaves_no_temp_files() {
        let home = std::env::temp_dir().join(format!("klip-icons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        install(&home).unwrap();
        let dir = directory(&home);
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), STATES.len() * 2);
        assert!(names.iter().all(|n| n.ends_with("-symbolic.svg")));
        let first = fs::metadata(dir.join(&names[0]))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        install(&home).unwrap();
        assert_eq!(
            fs::metadata(dir.join(&names[0]))
                .unwrap()
                .modified()
                .unwrap(),
            first
        );
        fs::write(dir.join(&names[0]), "tampered").unwrap();
        install(&home).unwrap();
        assert!(fs::read_to_string(dir.join(&names[0]))
            .unwrap()
            .starts_with("<svg"));
        let _ = fs::remove_dir_all(&home);
    }
}
