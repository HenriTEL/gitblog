use std::{error::Error, fs};

use gitblog::{atom_feed, git::State};

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
    blog_url: String,
}

fn main() {
    // tracing_subscriber::fmt()
    //     .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    //     .init();

    let args = CliArgs::parse();
    let up_to = match fetch_atom_feed(&args.blog_url) {
        Ok(feed) => feed.updated,
        Err(_) => {
            log::error!("Failed to fetch atom feed from {}", args.blog_url);
            chrono::DateTime::<chrono::Utc>::MIN_UTC.fixed_offset()
        }
    };

    // hello_gix(up_to);
    
    let url = gix::Url::from_bytes(BStr::new(args.repo.as_bytes())).expect("build git url");
    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https => {
            let remote = gitblog::git::GitRemote { url, branch: args.branch };
            let tree_ends = remote.fetch(&up_to);
            let diff = remote.tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree).expect("tree diff");
            for (path, state) in diff.iter() {
                println!("{:?}  {}", state, path.display());
            }
            let updated_file_blobs = diff.iter().filter_map(|(path, (state, maybe_oid))| {
                match (state, maybe_oid) {
                    (State::Created, Some(oid)) => Some(gitblog::git::FileBlob { file_path: path.clone(), oid: oid.to_owned() }),
                    (State::Deleted, None) => None,
                    (State::Modified, Some(oid)) => Some(gitblog::git::FileBlob { file_path: path.clone(), oid: oid.to_owned() }),
                    (State::Created, None) => None,
                    (State::Deleted, Some(_)) => None,
                    (State::Modified, None) => None,
                }
            }).collect::<Vec<_>>();
            let dest = remote.get_files(&updated_file_blobs, None).expect("fetch blobs");
            println!("blobs fetched to {}", dest.display());
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