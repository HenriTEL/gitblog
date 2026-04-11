use std::{error::Error, path::Path};

use gitblog::{atom_feed, git::State, html::markdown_file_to_html};

use clap::Parser;
use gix::bstr::BStr;
use log::error;

const MIN_UTC: chrono::DateTime<chrono::Utc> = chrono::DateTime::<chrono::Utc>::MIN_UTC;

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

    /// Generate all blog files, not just the ones that changed
    #[arg(long)]
    full: bool,
}

fn main() {
    // tracing_subscriber::fmt()
    //     .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    //     .init();

    let args = CliArgs::parse();
    let up_to = if args.full {
        MIN_UTC.fixed_offset()
    } else {
        match fetch_atom_feed(&args.blog_url) {
            Ok(feed) => feed.updated,
            Err(_) => {
                log::error!("Failed to fetch atom feed from {}", args.blog_url);
                log::info!("Falling back to full blog generation");
                MIN_UTC.fixed_offset()
            }
        }
    };

    // hello_gix(up_to);
    
    let url = gix::Url::from_bytes(BStr::new(args.repo.as_bytes())).expect("build git url");
    match url.scheme {
        gix::url::Scheme::Http | gix::url::Scheme::Https => {
            let remote = gitblog::git::GitRemote { url, branch: args.branch };
            let updated_file_blobs = if up_to == MIN_UTC.fixed_offset() {
                vec![]
            } else {
                let tree_ends = remote.fetch_since(&up_to);
                let diff = remote.tree_diff(&tree_ends.up_to_tree, &tree_ends.head_tree);
                diff.iter().filter_map(|(path, (state, maybe_oid))| {
                    match (state, maybe_oid) {
                        (State::Created, Some(oid)) => Some(gitblog::git::FileBlob { file_path: path.clone(), oid: oid.to_owned() }),
                        (State::Deleted, None) => None,
                        (State::Modified, Some(oid)) => Some(gitblog::git::FileBlob { file_path: path.clone(), oid: oid.to_owned() }),
                        (State::Created, None) => None,
                        (State::Deleted, Some(_)) => None,
                        (State::Modified, None) => None,
                    }
                }).collect::<Vec<_>>()
            };
            let dest = remote.pull_files(&updated_file_blobs, None).expect("fetch blobs");
            render_markdown_files(&dest);
            println!("Blog built at {} ", dest.display());
        },
        _ => error!("The URL {} resolved to protocol {} which is not supported.", url, url.scheme), // TODO exit failure
    }
}

fn render_markdown_files(dest: &Path) {
    fn walk(dir: &Path, dest_root: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, dest_root);
            } else if path.extension().is_some_and(|e| e == "md") {
                markdown_file_to_html(&path);
                println!(
                    "wrote {} ",
                    path.strip_prefix(dest_root)
                        .unwrap()
                        .with_extension("html")
                        .to_string_lossy()
                );
            }
        }
    }
    walk(dest, dest);
}

fn fetch_atom_feed(blog_url: &str) -> Result<atom_feed::Feed, Box<dyn Error>> {
    let url = format!("{}/atom.xml", blog_url.trim_end_matches('/'));
    let body = reqwest::blocking::get(&url)?.error_for_status()?.text()?;
    let feed = atom_feed::parse(&body)?;
    Ok(feed)
}
