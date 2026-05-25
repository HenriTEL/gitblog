use gix::url::Scheme;

use super::{file_url_to_path, parse_repo_url};

#[test]
fn parse_https_unchanged() {
    let url = parse_repo_url("https://example.com/repo.git").unwrap();
    assert!(matches!(url.scheme, Scheme::Http | Scheme::Https));
}

#[test]
fn parse_bare_absolute_path_as_file() {
    let dir = std::env::temp_dir();
    let url = parse_repo_url(dir.to_str().unwrap()).unwrap();
    assert_eq!(url.scheme, Scheme::File);
}

#[test]
fn parse_file_scheme_as_file() {
    let dir = std::env::temp_dir();
    let input = format!("file://{}", dir.display());
    let url = parse_repo_url(&input).unwrap();
    assert_eq!(url.scheme, Scheme::File);
}

#[test]
fn file_url_to_path_roundtrip() {
    let dir = std::env::temp_dir().canonicalize().unwrap();
    let url = parse_repo_url(dir.to_str().unwrap()).unwrap();
    let path = file_url_to_path(&url).unwrap();
    assert_eq!(path, dir);
}
