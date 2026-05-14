use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use html5ever::{
    Attribute, ExpandedName, QualName, local_name, parse_document,
    tendril::{StrTendril, TendrilSink},
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;

use super::{AvatarData, UserProfile, UserProfileError, strip_github_profile_og_suffix};

const PROFILE_URL_PREFIX: &str = "https://github.com/";
const FETCH_TIMEOUT_SECS: u64 = 10;

#[derive(Clone)]
struct ElementInner {
    name: QualName,
    attrs: RefCell<Vec<Attribute>>,
}

fn empty_fragment_placeholder() -> Rc<ElementInner> {
    Rc::new(ElementInner {
        name: QualName::new(None, "".into(), "fragment".into()),
        attrs: RefCell::new(Vec::new()),
    })
}

/// Minimal [`TreeSink`](html5ever::tree_builder::TreeSink): retains every created element + merged attrs.
struct MetaExtractSink {
    document: Rc<ElementInner>,
    /// Keeps `Rc` alive after the tree builder finishes (append is a no-op).
    nodes: RefCell<Vec<Rc<ElementInner>>>,
    template_fragments: RefCell<Vec<(Rc<ElementInner>, Rc<ElementInner>)>>,
}

impl MetaExtractSink {
    fn into_github_metas(self) -> (Option<String>, Option<String>, Option<String>) {
        let mut username = None;
        let mut image = None;
        let mut description = None;
        let mut visit = |node: &Rc<ElementInner>| {
            if node.name.local != local_name!("meta") {
                return;
            }
            let attrs = node.attrs.borrow();
            let mut prop_val = None::<String>;
            let mut content_val = None::<String>;
            for attr in attrs.iter() {
                if attr.name.local == local_name!("property") {
                    prop_val = Some(attr.value.to_string());
                } else if attr.name.local == local_name!("content") {
                    content_val = Some(attr.value.to_string());
                }
            }
            match (prop_val, content_val) {
                (Some(p), Some(c)) if !c.is_empty() => match p.as_str() {
                    "profile:username" => username = Some(c),
                    "og:image" => image = Some(c),
                    "og:description" => description = Some(c),
                    _ => {}
                },
                _ => {}
            }
        };
        for node in self.nodes.into_inner() {
            visit(&node);
        }
        (username, image, description)
    }
}

impl TreeSink for MetaExtractSink {
    type Handle = Rc<ElementInner>;
    type Output = Self;
    type ElemName<'a>
        = ExpandedName<'a>
    where
        Self: 'a;

    fn finish(self) -> Self {
        self
    }

    fn get_document(&self) -> Self::Handle {
        self.document.clone()
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        self.template_fragments
            .borrow()
            .iter()
            .find(|(t, _)| Rc::ptr_eq(t, target))
            .map(|(_, f)| f.clone())
            .expect("<template> contents handle missing")
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        Rc::ptr_eq(x, y)
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> ExpandedName<'a> {
        target.name.expanded()
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle {
        let node = Rc::new(ElementInner {
            name,
            attrs: RefCell::new(attrs),
        });
        self.nodes.borrow_mut().push(node.clone());
        if flags.template {
            let frag = empty_fragment_placeholder();
            self.template_fragments
                .borrow_mut()
                .push((node.clone(), frag));
        }
        node
    }

    fn create_comment(&self, _text: StrTendril) -> Self::Handle {
        empty_fragment_placeholder()
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Self::Handle {
        empty_fragment_placeholder()
    }

    fn append_before_sibling(&self, _sibling: &Self::Handle, _new_node: NodeOrText<Self::Handle>) {}

    fn append_based_on_parent_node(
        &self,
        _element: &Self::Handle,
        _prev_element: &Self::Handle,
        _new_node: NodeOrText<Self::Handle>,
    ) {
    }

    fn append(&self, _parent: &Self::Handle, _child: NodeOrText<Self::Handle>) {}

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let mut existing = target.attrs.borrow_mut();
        for attr in attrs {
            if !existing.iter().any(|a| a.name == attr.name) {
                existing.push(attr);
            }
        }
    }

    fn remove_from_parent(&self, _target: &Self::Handle) {}

    fn reparent_children(&self, _node: &Self::Handle, _new_parent: &Self::Handle) {}
}

impl MetaExtractSink {
    fn new() -> Self {
        MetaExtractSink {
            document: Rc::new(ElementInner {
                name: QualName::new(None, "".into(), "document".into()),
                attrs: RefCell::new(Vec::new()),
            }),
            nodes: RefCell::new(Vec::new()),
            template_fragments: RefCell::new(Vec::new()),
        }
    }
}

/// Profile metadata loaded from GitHub public profile HTML (`https://github.com/{user}` head metas).
pub struct GithubUserProfile {
    user_name: String,
    client: Client,
    profile_html: Mutex<Option<String>>,
}

impl GithubUserProfile {
    /// `user_name` is the profile slug (`HenriTEL` in `/HenriTEL`).
    pub fn new(user_name: impl Into<String>) -> Self {
        Self::with_client(default_client(), user_name)
    }

    pub fn with_client(client: Client, user_name: impl Into<String>) -> Self {
        GithubUserProfile {
            user_name: user_name.into(),
            client,
            profile_html: Mutex::new(None),
        }
    }

    pub fn profile_url(&self) -> String {
        format!(
            "{PROFILE_URL_PREFIX}{}",
            self.user_name.trim_start_matches('/')
        )
    }

    fn profile_html(&self) -> Result<String, UserProfileError> {
        let mut guard = self.profile_html.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref cached) = *guard {
            return Ok(cached.clone());
        }
        let resp = self.client.get(self.profile_url()).send()?;
        let status = resp.status();
        if status != StatusCode::OK {
            return Err(UserProfileError::HttpStatus(status.as_u16()));
        }
        let text = resp.text()?;
        *guard = Some(text.clone());
        Ok(text)
    }
}

fn parse_github_meta(html: &str) -> (Option<String>, Option<String>, Option<String>) {
    parse_document(MetaExtractSink::new(), Default::default())
        .one(html)
        .finish()
        .into_github_metas()
}

fn default_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .expect("GithubUserProfile default reqwest Client")
}

impl UserProfile for GithubUserProfile {
    fn get_about(&self) -> Result<String, UserProfileError> {
        let html = self.profile_html()?;
        let (_, _, desc) = parse_github_meta(html.as_str());
        let bio = desc
            .map(|d| strip_github_profile_og_suffix(&d))
            .unwrap_or_default();
        Ok(bio)
    }

    fn fetch_avatar(&self) -> Result<AvatarData, UserProfileError> {
        let html = self.profile_html()?;
        let (_, image_url, _) = parse_github_meta(html.as_str());
        let image_url = image_url.ok_or(UserProfileError::MissingMeta("og:image"))?;
        let resp = self.client.get(&image_url).send()?;
        let status = resp.status();
        if status != StatusCode::OK {
            return Err(UserProfileError::HttpStatus(status.as_u16()));
        }
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !content_type.to_ascii_lowercase().starts_with("image/") {
            return Err(UserProfileError::NonImageAvatar { content_type });
        }
        let bytes = resp.bytes()?.to_vec();
        Ok(AvatarData {
            bytes,
            content_type,
        })
    }

    fn get_username(&self) -> Result<String, UserProfileError> {
        let html = self.profile_html()?;
        let (username, _, _) = parse_github_meta(html.as_str());
        username.ok_or(UserProfileError::MissingMeta("profile:username"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
