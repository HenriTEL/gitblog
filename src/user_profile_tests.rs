use super::{
    AvatarData, OG_DESCRIPTION_BOILERPLATE_SUFFIX, UserProfile, UserProfileError, download_avatar,
    strip_github_profile_og_suffix,
};

#[test]
fn strip_suffix_removes_repository_boilerplate() {
    let raw =
        "Weekend handyman. HenriTEL has 15 repositories available. Follow their code on GitHub.";
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
