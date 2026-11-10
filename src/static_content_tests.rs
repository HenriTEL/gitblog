use std::fs;

use tempfile::tempdir;

use super::{site_has_icon, write_static_content};

#[test]
fn creates_media_dir_when_the_blog_has_none() {
    let dir = tempdir().unwrap();

    write_static_content(dir.path()).unwrap();

    assert!(dir.path().join("media/favicon.svg").is_file());
    assert!(dir.path().join("media/icons.svg").is_file());
    assert!(dir.path().join("style.css").is_file());
}

#[test]
fn keeps_the_blogs_own_favicon() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("media")).unwrap();
    fs::write(dir.path().join("media/favicon.svg"), "<svg id=\"mine\"/>").unwrap();

    write_static_content(dir.path()).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("media/favicon.svg")).unwrap(),
        "<svg id=\"mine\"/>"
    );
}

#[test]
fn custom_stylesheets_replace_the_bundled_ones() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("css")).unwrap();
    fs::write(dir.path().join("css/theme.css"), "/* my-theme */").unwrap();

    write_static_content(dir.path()).unwrap();

    let style = fs::read_to_string(dir.path().join("style.css")).unwrap();
    assert!(style.contains("/* my-theme */"));
    assert!(
        !style.contains(super::THEME),
        "bundled theme should be replaced"
    );
    assert!(style.contains("grid-template-rows"), "layout stays bundled");
}

#[test]
fn icon_lookup_prefers_the_blogs_own_sprite() {
    let dir = tempdir().unwrap();
    assert!(site_has_icon(dir.path(), "github"));
    assert!(!site_has_icon(dir.path(), "bluesky"));

    fs::create_dir_all(dir.path().join("media")).unwrap();
    fs::write(
        dir.path().join("media/icons.svg"),
        "<symbol id=\"bluesky\"></symbol>",
    )
    .unwrap();

    assert!(site_has_icon(dir.path(), "bluesky"));
    assert!(!site_has_icon(dir.path(), "github"));
}

#[test]
fn bundled_stylesheet_styles_every_alert_type() {
    let dir = tempdir().unwrap();

    write_static_content(dir.path()).unwrap();

    let style = fs::read_to_string(dir.path().join("style.css")).unwrap();
    for class in ["tip", "important", "warning", "caution", "title"] {
        assert!(
            style.contains(&format!(".markdown-alert-{class}")),
            "{class}"
        );
    }
}
