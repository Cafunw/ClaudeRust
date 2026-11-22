//! The main window's colour before its page paints, so a dark theme doesn't
//! start with a white window.

use tauri::window::Color;

/// The page background (app.css `--bg-0`) of the theme the page will use:
/// the saved one, or the system's for "system".
pub fn page_background(theme: &str, system_dark: bool) -> Color {
    let dark = match theme {
        "dark" => true,
        "light" => false,
        _ => system_dark,
    };
    if dark { Color(0x19, 0x19, 0x19, 255) } else { Color(0xff, 0xff, 0xff, 255) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_theme_wins_and_system_follows_the_os() {
        let dark = Color(0x19, 0x19, 0x19, 255);
        let light = Color(0xff, 0xff, 0xff, 255);
        assert_eq!(page_background("dark", false), dark);
        assert_eq!(page_background("light", true), light);
        assert_eq!(page_background("system", true), dark);
        assert_eq!(page_background("system", false), light);
        assert_eq!(page_background("", true), dark, "unknown or empty: as system");
    }
}
