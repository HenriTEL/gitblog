//! HTML templates embedded at compile time for [`tera::Tera`].

use std::sync::OnceLock;

use tera::Tera;

const RAW_TEMPLATES: &[(&str, &str)] = &[
    (
        "article.html.j2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/templates/article.html.j2"
        )),
    ),
    (
        "index.html.j2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/templates/index.html.j2"
        )),
    ),
    (
        "head_common.html.j2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/templates/head_common.html.j2"
        )),
    ),
    (
        "navbar.html.j2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/templates/navbar.html.j2"
        )),
    ),
    (
        "footer.html.j2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/templates/footer.html.j2"
        )),
    ),
];

static TERA: OnceLock<Tera> = OnceLock::new();

/// Lazily built [`Tera`] instance with all embedded templates registered under the same names used in
/// `{% include "…" %}`.
pub fn tera() -> &'static Tera {
    TERA.get_or_init(|| {
        let mut tera = Tera::default();
        tera
            .add_raw_templates(RAW_TEMPLATES.iter().copied())
            .expect("embedded templates must parse");
        tera
    })
}
