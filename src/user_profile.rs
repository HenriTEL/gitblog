//! User-facing profile metadata (e.g. for nav / social templates).

pub mod github;

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use log::info;
use regex::Regex;

pub use github::GithubUserProfile;

pub trait UserProfile {
    fn get_username(&self) -> Result<String, UserProfileError>;

    fn get_about(&self) -> Result<String, UserProfileError>;

    fn fetch_avatar(&self) -> Result<AvatarData, UserProfileError>;
}

/// Binary avatar payload plus HTTP `Content-Type` (typically `image/jpeg`, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarData {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[derive(Debug)]
pub enum UserProfileError {
    Http(reqwest::Error),
    HttpStatus(u16),
    MissingMeta(&'static str),
    NonImageAvatar { content_type: String },
}

impl fmt::Display for UserProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UserProfileError::Http(e) => write!(f, "HTTP client error: {e}"),
            UserProfileError::HttpStatus(code) => write!(f, "unexpected HTTP status {code}"),
            UserProfileError::MissingMeta(name) => write!(f, "missing required meta `{name}`"),
            UserProfileError::NonImageAvatar { content_type } => write!(
                f,
                "avatar response is not an image (content-type: {content_type})"
            ),
        }
    }
}

impl Error for UserProfileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            UserProfileError::Http(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for UserProfileError {
    fn from(value: reqwest::Error) -> Self {
        UserProfileError::Http(value)
    }
}

/// Username and biography for templates (assembled by callers from [`UserProfile::get_username`]
/// and [`UserProfile::get_about`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfileMeta {
    pub username: String,
    pub bio: String,
}

#[derive(Debug)]
pub enum UserProfileDownloadError {
    Profile(UserProfileError),
    Io(std::io::Error),
}

impl fmt::Display for UserProfileDownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UserProfileDownloadError::Profile(e) => write!(f, "{e}"),
            UserProfileDownloadError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl Error for UserProfileDownloadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            UserProfileDownloadError::Profile(e) => Some(e),
            UserProfileDownloadError::Io(e) => Some(e),
        }
    }
}

impl From<UserProfileError> for UserProfileDownloadError {
    fn from(value: UserProfileError) -> Self {
        UserProfileDownloadError::Profile(value)
    }
}

impl From<std::io::Error> for UserProfileDownloadError {
    fn from(value: std::io::Error) -> Self {
        UserProfileDownloadError::Io(value)
    }
}

/// Fetches the avatar via [`UserProfile::fetch_avatar`] and writes it at `dest`.
pub fn download_avatar(
    provider: &impl UserProfile,
    dest: &Path,
) -> Result<(), UserProfileDownloadError> {
    let avatar = provider.fetch_avatar()?;
    fs::write(dest, avatar.bytes)?;
    info!("Updated user profile avatar");
    Ok(())
}

/// Suffix GitHub adds to `og:description` on profile pages (`… has N repositories available. Follow their code on GitHub.`).
pub static OG_DESCRIPTION_BOILERPLATE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\. [^.]+ has \d+ repositories available\. Follow their code on GitHub\.$")
        .expect("OG_DESCRIPTION_BOILERPLATE_SUFFIX regex")
});

/// Removes [`OG_DESCRIPTION_BOILERPLATE_SUFFIX`] from `og:description` when present.
pub fn strip_github_profile_og_suffix(raw: &str) -> String {
    OG_DESCRIPTION_BOILERPLATE_SUFFIX
        .replace(raw, "")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        AvatarData, OG_DESCRIPTION_BOILERPLATE_SUFFIX, UserProfile, UserProfileError,
        download_avatar, strip_github_profile_og_suffix,
    };

    #[test]
    fn strip_suffix_removes_repository_boilerplate() {
        let raw = "Weekend handyman. HenriTEL has 15 repositories available. Follow their code on GitHub.";
        // Pattern includes the delimiter `. ` before `<handle> has N repositories…`, so the
        // sentence-ending period is removed with the boilerplate.
        assert_eq!(strip_github_profile_og_suffix(raw), "Weekend handyman");
    }

    #[test]
    fn strip_suffix_leaves_bio_without_boilerplate_unchanged() {
        assert_eq!(
            strip_github_profile_og_suffix("Only a short bio."),
            "Only a short bio."
        );
    }

    #[test]
    fn strip_suffix_handles_various_repo_counts() {
        let raw = "Builder. HenriTEL has 28 repositories available. Follow their code on GitHub.";
        assert_eq!(strip_github_profile_og_suffix(raw), "Builder");
        assert!(
            OG_DESCRIPTION_BOILERPLATE_SUFFIX
                .is_match("x. HenriTEL has 0 repositories available. Follow their code on GitHub."),
            "digits should match with \\d+"
        );
    }

    struct StubProfile;

    impl UserProfile for StubProfile {
        fn get_about(&self) -> Result<String, UserProfileError> {
            Ok("About me.".into())
        }

        fn fetch_avatar(&self) -> Result<AvatarData, UserProfileError> {
            Ok(AvatarData {
                bytes: vec![7, 7, 7],
                content_type: "image/png".into(),
            })
        }

        fn get_username(&self) -> Result<String, UserProfileError> {
            Ok("alice".into())
        }
    }

    #[test]
    fn download_avatar_writes_avatar_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let avatar_path = dir.path().join("media").join("avatar");
        std::fs::create_dir_all(avatar_path.parent().expect("parent")).expect("create_dir_all");
        download_avatar(&StubProfile, &avatar_path).expect("download_avatar");
        assert!(avatar_path.is_file());
        assert_eq!(std::fs::read(&avatar_path).unwrap(), vec![7, 7, 7]);
    }
}
