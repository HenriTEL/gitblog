use crate::user_profile::strip_github_profile_og_suffix;

use super::{parse_github_meta, parse_github_social_accounts};

const SOCIAL_FIXTURE: &str = r#"<ul class="vcard-details">
<li itemprop="url" class="vcard-detail"><svg class="octicon"></svg>
  <a rel="nofollow me" href="https://example.com">https://example.com</a></li>
<li itemprop="social" class="vcard-detail"><svg role="img" class="octicon"><title id="a1">X</title></svg>
  <a rel="nofollow me" href="https://x.com/alice">@alice</a></li>
<li itemprop="social" class="vcard-detail"><svg role="img" class="octicon"><title id="a2">Mastodon</title><path d="M0"></path></svg>
  <a rel="nofollow me" href="https://fedi.example/@alice">@alice@fedi.example</a></li>
<li itemprop="homeLocation"><a href="https://example.com/not-social">Paris</a></li>
</ul>"#;

#[test]
fn parses_social_accounts_and_skips_other_profile_links() {
    assert_eq!(
        parse_github_social_accounts(SOCIAL_FIXTURE),
        vec![
            ("twitter".to_string(), "https://x.com/alice".to_string()),
            (
                "mastodon".to_string(),
                "https://fedi.example/@alice".to_string()
            ),
        ]
    );
}

#[test]
fn profile_without_social_accounts_yields_none() {
    assert!(parse_github_social_accounts(FIXTURE).is_empty());
}

const FIXTURE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta property="og:description" content="Weekend handyman. HenriTEL has 15 repositories available. Follow their code on GitHub.">
<meta property="og:image" content="https://avatars.githubusercontent.com/u/5563535?v=4?s=400">
<meta property="profile:username" content="HenriTEL">
<title>Henri (@HenriTEL)</title>
</head></html>"#;

const FIXTURE_CP_ORDER: &str = r#"<!DOCTYPE html>
<html><head>
<meta content="https://avatars.example.com/me.png?v=4" property="og:image">
<meta content="Charlie" property="profile:username">
<meta content="Hobbyist. HenriTEL has 9 repositories available. Follow their code on GitHub." property="og:description">
</head></html>"#;

#[test]
fn parse_fixture_metas_standard_order() {
    let (u, img, desc) = parse_github_meta(FIXTURE);
    assert_eq!(u.as_deref(), Some("HenriTEL"));
    assert_eq!(
        img.as_deref(),
        Some("https://avatars.githubusercontent.com/u/5563535?v=4?s=400")
    );
    assert_eq!(
        desc.as_deref(),
        Some(
            "Weekend handyman. HenriTEL has 15 repositories available. Follow their code on GitHub."
        )
    );
}

#[test]
fn parse_fixture_metas_content_before_property() {
    let (u, img, desc) = parse_github_meta(FIXTURE_CP_ORDER);
    assert_eq!(u.as_deref(), Some("Charlie"));
    assert_eq!(
        img.as_deref(),
        Some("https://avatars.example.com/me.png?v=4")
    );
    assert_eq!(
        strip_github_profile_og_suffix(desc.unwrap().as_str()),
        "Hobbyist"
    );
}
