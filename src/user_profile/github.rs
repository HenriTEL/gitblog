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
    text: RefCell<String>,
}

fn empty_fragment_placeholder() -> Rc<ElementInner> {
    Rc::new(ElementInner {
        name: QualName::new(None, "".into(), "fragment".into()),
        attrs: RefCell::new(Vec::new()),
        text: RefCell::new(String::new()),
    })
}

impl ElementInner {
    fn attr(&self, name: &str) -> Option<String> {
        self.attrs
            .borrow()
            .iter()
            .find(|attr| attr.name.local.as_ref() == name)
            .map(|attr| attr.value.to_string())
    }
}

/// Maps the provider label GitHub shows on a profile to its icon name in `media/icons.svg`.
fn provider_icon_name(label: &str) -> String {
    match label.trim().to_lowercase().as_str() {
        "x" => "twitter".to_string(),
        other => other.to_string(),
    }
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

    /// Reads `<li itemprop="social"><svg><title>Provider</title></svg><a href="…">` entries, relying
    /// on nodes being retained in document order.
    fn into_social_accounts(self) -> Vec<(String, String)> {
        let mut accounts = Vec::new();
        let mut in_social_item = false;
        let mut provider = None::<String>;
        for node in self.nodes.into_inner() {
            let local = node.name.local.as_ref();
            if local == "li" {
                in_social_item = node.attr("itemprop").as_deref() == Some("social");
                provider = None;
            } else if in_social_item && local == "title" {
                provider = Some(provider_icon_name(&node.text.borrow()));
            } else if in_social_item && local == "a" {
                if let (Some(name), Some(href)) = (provider.take(), node.attr("href")) {
                    accounts.push((name, href));
                }
                in_social_item = false;
            }
        }
        accounts
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
            text: RefCell::new(String::new()),
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

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        if let NodeOrText::AppendText(text) = child {
            parent.text.borrow_mut().push_str(&text);
        }
    }

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
                text: RefCell::new(String::new()),
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

fn parse_github_social_accounts(html: &str) -> Vec<(String, String)> {
    parse_document(MetaExtractSink::new(), Default::default())
        .one(html)
        .finish()
        .into_social_accounts()
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

    fn get_social_accounts(&self) -> Result<Vec<(String, String)>, UserProfileError> {
        Ok(parse_github_social_accounts(self.profile_html()?.as_str()))
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
#[path = "github_tests.rs"]
mod tests;
