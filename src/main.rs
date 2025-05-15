use std::{error::Error, fs};

use gix_protocol::handshake::Ref;
use gix_protocol::ls_refs;
use gix_url::Url;
use gix_transport::bstr::BStr;
use gix_transport::client::http;
use prodash::progress;
use comrak::{markdown_to_html_with_plugins, Plugins, plugins, Options};

fn main() -> Result<(), Box<dyn Error>> {
    let url = BStr::new("https://github.com/GitoxideLabs/gitoxide");
    let gix_url = Url::from_bytes(url)?;

    let mut transport = http::connect(gix_url, gix_transport::Protocol::default(), false);
    gix_protocol::handshake(
        &mut transport,
        gix_transport::Service::UploadPack,
        &mut |_| Ok(None),
        vec![],
        &mut progress::Discard,
    )?;
    let refs = ls_refs(
        &mut transport,
        &gix_transport::client::Capabilities::default(),
        |_, _, _| Ok(ls_refs::Action::Continue),
        &mut progress::Discard,
        false,
    )?;

    for ref_ in refs.iter().take(10) {
        match ref_ {
            Ref::Direct { full_ref_name, .. } => println!("{}", full_ref_name),
            _ => (),
        };
    }
    readme_to_html();

    Ok(())
}


fn readme_to_html() {
    let file_path = "README.md";
    let md_input = fs::read_to_string(file_path)
        .expect("Read README.md file");
    let mut options = Options::default();
    options.extension.footnotes = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;

    let code_highlighter = plugins::syntect::SyntectAdapter::new(Some("base16-eighties.dark"));
    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&code_highlighter);


    let html_output = markdown_to_html_with_plugins(&md_input, &Options::default(), &plugins);
    println!("{html_output}");
}