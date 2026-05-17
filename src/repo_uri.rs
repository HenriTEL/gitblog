use std::io;
use std::path::PathBuf;

use gix::bstr::BStr;
use gix::url::Scheme;

/// Error while normalizing a repository path or URL.
#[derive(Debug)]
pub enum RepoUriError {
    Parse(gix::url::parse::Error),
    Io(io::Error),
    NotFileScheme,
}

impl std::fmt::Display for RepoUriError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoUriError::Parse(e) => write!(f, "invalid repository URI: {e}"),
            RepoUriError::Io(e) => write!(f, "repository path: {e}"),
            RepoUriError::NotFileScheme => write!(f, "expected a file:// repository URL"),
        }
    }
}

impl std::error::Error for RepoUriError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RepoUriError::Parse(e) => Some(e),
            RepoUriError::Io(e) => Some(e),
            _ => None,
        }
    }
}

fn canonicalize_err(e: gix::path::realpath::Error) -> RepoUriError {
    RepoUriError::Io(io::Error::other(e))
}

fn has_remote_scheme_prefix(repo: &str) -> bool {
    repo.starts_with("http://")
        || repo.starts_with("https://")
        || repo.starts_with("ssh://")
        || repo.starts_with("git://")
        || repo.starts_with("git@")
}

/// Parse `repo` as a remote Git URL or as a local path (always yielding [`Scheme::File`]).
pub fn parse_repo_url(repo: &str) -> Result<gix::Url, RepoUriError> {
    if has_remote_scheme_prefix(repo) {
        return gix::Url::from_bytes(BStr::new(repo.as_bytes())).map_err(RepoUriError::Parse);
    }

    if repo.starts_with("file://") {
        let mut url =
            gix::Url::from_bytes(BStr::new(repo.as_bytes())).map_err(RepoUriError::Parse)?;
        let cwd = std::env::current_dir().map_err(RepoUriError::Io)?;
        url.canonicalize(&cwd).map_err(canonicalize_err)?;
        debug_assert_eq!(url.scheme, Scheme::File);
        return Ok(url);
    }

    let path = PathBuf::from(repo);
    let abs = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(RepoUriError::Io)?
            .join(path)
    };
    let abs = abs.canonicalize().map_err(RepoUriError::Io)?;

    let file_uri = format!("file://{}", abs.display());
    let mut url =
        gix::Url::from_bytes(BStr::new(file_uri.as_bytes())).map_err(RepoUriError::Parse)?;
    let cwd = std::env::current_dir().map_err(RepoUriError::Io)?;
    url.canonicalize(&cwd).map_err(canonicalize_err)?;
    Ok(url)
}

/// Filesystem path for a [`Scheme::File`] URL.
pub fn file_url_to_path(url: &gix::Url) -> Result<PathBuf, RepoUriError> {
    if url.scheme != Scheme::File {
        return Err(RepoUriError::NotFileScheme);
    }

    let path_bytes = url.path.as_ref();
    let path_str = std::str::from_utf8(path_bytes).map_err(|_| {
        RepoUriError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "file URL path is not valid UTF-8",
        ))
    })?;

    let path = if cfg!(windows) {
        let trimmed = path_str.trim_start_matches('/');
        if trimmed.len() >= 2 && trimmed.as_bytes().get(1) == Some(&b':') {
            PathBuf::from(trimmed)
        } else {
            PathBuf::from(path_str)
        }
    } else {
        PathBuf::from(path_str)
    };

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::url::Scheme;

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
}
