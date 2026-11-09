//! User-facing profile metadata (e.g. for nav / social templates).

pub mod github;

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use log::info;
use regex::Regex;

pub use github::GithubUserProfile;

pub trait UserProfile {
    /// # Errors
    /// Fails when the profile cannot be fetched or has no username.
    fn get_username(&self) -> Result<String, UserProfileError>;

    /// # Errors
    /// Fails when the profile cannot be fetched or has no bio.
    fn get_about(&self) -> Result<String, UserProfileError>;

    /// `(provider, url)` pairs for the social accounts listed on the profile; `provider` is lowercase.
    ///
    /// # Errors
    /// Fails when the profile cannot be fetched.
    fn get_social_accounts(&self) -> Result<Vec<(String, String)>, UserProfileError>;

    /// # Errors
    /// Fails when the avatar cannot be downloaded or is not an image.
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
            UserProfileError::HttpStatus(_)
            | UserProfileError::MissingMeta(_)
            | UserProfileError::NonImageAvatar { .. } => None,
        }
    }
}

impl From<reqwest::Error> for UserProfileError {
    fn from(value: reqwest::Error) -> Self {
        UserProfileError::Http(value)
    }
}

/// Author details for templates (assembled by callers from [`UserProfile`] or the git history).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserProfileMeta {
    pub username: String,
    pub bio: String,
    /// Footer links, keyed by icon name in `media/icons.svg`.
    pub social_accounts: HashMap<String, String>,
    /// Site-relative avatar URL; `None` when the site has no avatar.
    pub avatar_url: Option<String>,
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
///
/// # Errors
/// Fails when the avatar cannot be fetched or written.
pub fn download_avatar(
    provider: &impl UserProfile,
    dest: &Path,
) -> Result<(), UserProfileDownloadError> {
    let avatar = provider.fetch_avatar()?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
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
#[path = "user_profile_tests.rs"]
mod tests;
