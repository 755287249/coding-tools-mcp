/// Only the UI entry routes may resolve to index.html. Never serve HTML for a missing chunk.
pub(crate) fn asset_path(path: &str) -> Option<&str> {
    if path.contains("..") || path.contains('%') || path.contains('\\') { return None; }
    if path.is_empty() || path == "quick-setup" || path.starts_with("workspace/") || path.starts_with("settings/") {
        return Some("index.html");
    }
    if path.starts_with("_app/") || ["app-icon.svg", "favicon.png", "favicon.svg", "chat-preview.html"].contains(&path) { Some(path) } else { None }
}

pub(crate) fn cache_control(path: &str) -> &'static str {
    if path.starts_with("_app/immutable/") { "public, max-age=31536000, immutable" } else { "no-store" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logo_and_ui_routes_are_available_but_private_paths_are_not() {
        for path in ["app-icon.svg", "favicon.png", "_app/immutable/bundle.hash.js"] { assert_eq!(asset_path(path), Some(path)); }
        for path in ["", "workspace/example", "settings/appearance", "quick-setup"] { assert_eq!(asset_path(path), Some("index.html")); }
        for path in ["browser/invoke", "secrets.json", "_app/../secret", "_app/%2e%2e/secret", "_app/..%2fsecret", "_app/\\secret"] { assert_eq!(asset_path(path), None); }
    }
    #[test]
    fn only_content_addressed_assets_are_cached() {
        assert_eq!(cache_control("_app/immutable/bundle.hash.js"), "public, max-age=31536000, immutable");
        for path in ["index.html", "app-icon.svg", "chat-preview.html", "_app/version.json"] { assert_eq!(cache_control(path), "no-store"); }
    }
}
