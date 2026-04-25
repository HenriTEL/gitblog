use std::path::Path;

const STYLE_SHEET: &str = concat!(
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/css/theme.css")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/css/layout.css")),
);

const MEDIAS: &[(&str, &str)] = &[
    (
        "favicon.svg",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/media/favicon.svg")),
    ),
    (
        "icons.svg",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/media/icons.svg")),
    ),
];

pub fn write_static_content(dest: &Path) {
    for (name, content) in MEDIAS {
        let path = dest.join("media").join(name);
        std::fs::write(path, content).unwrap();
    }
    let path = dest.join("style.css");
    std::fs::write(path, STYLE_SHEET).unwrap();
}
