//! HTML templates embedded at compile time for [`tera::Tera`].

use std::fs;
use std::path::Path;
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

fn embedded_tera() -> Tera {
    let mut tera = Tera::default();
    tera.add_raw_templates(RAW_TEMPLATES.iter().copied())
        .expect("embedded templates must parse");
    tera
}

/// Lazily built [`Tera`] instance with all embedded templates registered under the same names used in
/// `{% include "…" %}`, plus any overrides from [`install_blog_templates`].
pub fn tera() -> &'static Tera {
    TERA.get_or_init(embedded_tera)
}

fn tera_with_overrides(overrides: &[(String, String)]) -> Result<Tera, tera::Error> {
    let mut tera = embedded_tera();
    tera.add_raw_templates(
        overrides
            .iter()
            .map(|(name, content)| (name.as_str(), content.as_str())),
    )?;
    Ok(tera)
}

/// Uses the files in the blog's `templates/` folder over the embedded templates of the same name,
/// then removes that folder from `dest` so it is not published. Call before rendering anything.
pub fn install_blog_templates(dest: &Path) -> Result<(), tera::Error> {
    let dir = dest.join("templates");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut overrides = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(content) = fs::read_to_string(entry.path()) {
            overrides.push((name, content));
        }
    }
    let _ = TERA.set(tera_with_overrides(&overrides)?);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[cfg(test)]
#[path = "templates_tests.rs"]
mod tests;
