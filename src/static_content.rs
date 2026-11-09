use std::fs;
use std::path::Path;

const PREFLIGHT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/css/preflight.css"));
const THEME: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/css/theme.css"));
const LAYOUT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/css/layout.css"));
const ICONS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/media/icons.svg"));

const MEDIAS: &[(&str, &str)] = &[
    (
        "favicon.svg",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/media/favicon.svg")),
    ),
    ("icons.svg", ICONS),
];

/// Adds the bundled media the blog does not provide itself and writes `style.css`, using the blog's
/// own `css/theme.css` and `css/layout.css` in place of the bundled ones when present.
///
/// # Errors
/// Fails when a file cannot be written under `dest`.
pub fn write_static_content(dest: &Path) -> std::io::Result<()> {
    let media_dir = dest.join("media");
    fs::create_dir_all(&media_dir)?;
    for (name, content) in MEDIAS {
        let path = media_dir.join(name);
        if !path.exists() {
            fs::write(path, content)?;
        }
    }

    let style_sheet = [
        PREFLIGHT.to_string(),
        stylesheet_or_default(dest, "theme.css", THEME),
        stylesheet_or_default(dest, "layout.css", LAYOUT),
    ]
    .concat();
    fs::write(dest.join("style.css"), style_sheet)
}

fn stylesheet_or_default(dest: &Path, name: &str, default: &str) -> String {
    fs::read_to_string(dest.join("css").join(name)).unwrap_or_else(|_| default.to_string())
}

/// Whether the site's icon sprite (the blog's own `media/icons.svg`, else the bundled one) has `icon`.
#[must_use]
pub fn site_has_icon(dest: &Path, icon: &str) -> bool {
    let custom = fs::read_to_string(dest.join("media").join("icons.svg")).ok();
    custom
        .as_deref()
        .unwrap_or(ICONS)
        .contains(&format!("id=\"{icon}\""))
}

#[cfg(test)]
#[path = "static_content_tests.rs"]
mod tests;
