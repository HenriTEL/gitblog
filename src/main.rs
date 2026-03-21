use std::{error::Error, fs};

use gitblog::atom_feed;

use clap::Parser;
use comrak::{markdown_to_html_with_plugins, Plugins, plugins, Options};
use gix::bstr::BStr;
use log::error;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct CliArgs {
    /// Path or URL of the git repository containing the blog sources
    repo: String,
    /// Branch on the git repository to use
    #[arg(long, default_value = "main")]
    branch: String,
    /// URL where your blog is hosted
    #[arg(long)]
    blog_url: Option<String>,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = CliArgs::parse();

    let up_to = if let Some(ref blog_url) = args.blog_url {
        fetch_atom_feed(blog_url)
            .expect("failed to fetch atom feed")
            .updated
    } else {
        chrono::DateTime::<chrono::Utc>::MIN_UTC.fixed_offset()
    };
    // hello_gix(up_to);
    
    let url = gix::Url::from_bytes(BStr::new(args.repo.as_bytes())).expect("built git url");
    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https => {
            let remote = gitblog::git::GitRemote { url, branch: args.branch };
            remote.fetch(&up_to);
        },
        _ => error!("The URL {} resolved to protocol {} which is not supported.", url, url.scheme), // TODO exit failure
    }

    // let remote = gitblog::git::GitRemote::new(&args.repo_uri, &args.branch)
    //     .expect("invalid repository URI");

    // let changes = remote.changes_since(&up_to).expect("failed to compute changes");

    // let mut paths: Vec<_> = changes.iter().collect();
    // paths.sort_by_key(|(p, _)| p.as_path());
    // for (path, state) in paths {
    //     println!("{:?}  {}", state, path.display());
    // }
}

fn fetch_atom_feed(blog_url: &str) -> Result<atom_feed::Feed, Box<dyn Error>> {
    let url = format!("{}/atom.xml", blog_url.trim_end_matches('/'));
    let body = reqwest::blocking::get(&url)?.error_for_status()?.text()?;
    let feed = atom_feed::parse(&body)?;
    Ok(feed)
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