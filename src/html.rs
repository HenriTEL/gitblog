use std::path::Path;

use comrak::{Options, markdown_to_html};

pub fn markdown_file_to_html(markdown_path: &Path) {
    let md_content = std::fs::read_to_string(markdown_path).expect("read markdown");
    let html_content = markdown_to_html(&md_content, &Options::default());

    let mut html_path = markdown_path.to_path_buf();
    html_path.set_extension("html");
    std::fs::write(&html_path, html_content).expect("write html");
    println!("wrote {} ", &html_path.display());
}
