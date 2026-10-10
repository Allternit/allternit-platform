//! Community and project links (Help menu, About dialog, Home screen).
//!
//! This is a vendored, rebranded copy of FilmCraft (see the README's top note); the upstream
//! ArtCraft community links (Discord, website) were removed with the ArtCraft brand marks. The
//! upstream source repository stays linked for attribution.

/// The app's short name, used in the website and repository URLs.
pub const APP: &str = "filmcraft";

/// The upstream source repository (kept for attribution).
pub const GITHUB: &str = "https://github.com/storytold/filmcraft";
/// New issue on the upstream repository.
pub const ISSUES: &str = "https://github.com/storytold/filmcraft/issues";

/// (command id, label, url) for every link, in menu order.
pub const ALL: [(&str, &str, &str); 2] = [
    ("help.github", "Source code on GitHub", GITHUB),
    ("help.reportIssue", "Report an Issue…", ISSUES),
];

/// The URL a `help.*` link command opens.
pub fn url_for(command: &str) -> Option<&'static str> {
    ALL.iter().find(|(id, _, _)| *id == command).map(|(_, _, u)| *u)
}

/// Open `url` in the system browser (a new tab on the web).
pub fn open(ctx: &egui::Context, url: &str) {
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_project_scheme() {
        assert_eq!(GITHUB, format!("https://github.com/storytold/{APP}"));
        assert!(ALL.iter().all(|(id, _, u)| id.starts_with("help.") && u.starts_with("https://")));
        assert_eq!(url_for("help.github"), Some(GITHUB));
        assert_eq!(url_for("help.nope"), None);
    }
}
