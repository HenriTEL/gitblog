use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use gix::ObjectId;

use crate::markdown::parse_content_metadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlogPost {
    pub object_id: Option<ObjectId>,
    pub last_updated: DateTime<FixedOffset>,
    /// `None` until set from Atom or git; use [`BlogPost::effective_publication_date`].
    pub publication_date: Option<DateTime<FixedOffset>>,
    pub title: String,
    pub summary: String,
    pub path: PathBuf,
    /// Top-level source folder (e.g. `tech` for `tech/post.md`).
    pub section: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BlogPostUpdate {
    pub object_id: Option<ObjectId>,
    pub last_updated: Option<DateTime<FixedOffset>>,
    pub publication_date: Option<DateTime<FixedOffset>>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Default)]
struct BlogPostStore {
    next_store_id: u64,
    posts: HashMap<u64, BlogPost>,
    by_object_id: HashMap<ObjectId, u64>,
    by_path: HashMap<PathBuf, u64>,
}

thread_local! {
    static BLOG_POST_STORE: RefCell<BlogPostStore> = RefCell::new(BlogPostStore::default());
}

impl BlogPost {
    pub fn new(
        path: PathBuf,
        last_updated: DateTime<FixedOffset>,
        title: String,
        summary: String,
    ) -> Self {
        let section = section_from_path(&path);
        Self {
            object_id: None,
            last_updated,
            publication_date: None,
            title,
            summary,
            path,
            section,
        }
    }

    pub fn with_defaults(path: PathBuf, last_updated: DateTime<FixedOffset>) -> Self {
        let section = section_from_path(&path);
        Self {
            object_id: None,
            title: fallback_title(&path),
            summary: String::new(),
            path,
            last_updated,
            publication_date: None,
            section,
        }
    }

    pub fn effective_publication_date(&self) -> DateTime<FixedOffset> {
        self.publication_date.unwrap_or(self.last_updated)
    }

    pub fn update_from_source(
        &mut self,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
        path: PathBuf,
    ) {
        self.path = path.clone();
        self.section = section_from_path(&path);
        self.last_updated = last_updated;
        self.title = title;
        self.summary = summary;
    }

    pub fn from_source(
        path: PathBuf,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
        publication_date: DateTime<FixedOffset>,
    ) -> Self {
        let section = section_from_path(&path);
        Self {
            object_id: None,
            path,
            title,
            summary,
            last_updated,
            publication_date: Some(publication_date),
            section,
        }
    }

    pub fn update_from_source_content(&mut self, content: &str, frontmatter_delimiter: &str) {
        let fallback = fallback_title(&self.path);
        let (title, summary, publication_date, last_modified) =
            parse_content_metadata(content, &fallback, frontmatter_delimiter);
        self.title = title;
        self.summary = summary;
        if let Some(date) = publication_date {
            self.publication_date = Some(date);
        }
        if let Some(date) = last_modified {
            self.last_updated = date;
        }
    }

    fn apply_update(&mut self, update: BlogPostUpdate) {
        if let Some(object_id) = update.object_id {
            self.object_id = Some(object_id);
        }
        if let Some(last_updated) = update.last_updated {
            self.last_updated = last_updated;
        }
        if let Some(publication_date) = update.publication_date {
            self.publication_date = Some(publication_date);
        }
        if let Some(title) = update.title {
            self.title = title;
        }
        if let Some(summary) = update.summary {
            self.summary = summary;
        }
        if let Some(path) = update.path {
            self.path = path;
            self.section = section_from_path(&self.path);
        }
    }
}

/// Top-level folder name for a markdown source path (`tech/post.md` → `Some("tech")`).
pub fn section_from_path(path: &Path) -> Option<String> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .and_then(|p| p.components().next())
        .and_then(|c| c.as_os_str().to_str())
        .map(str::to_owned)
}

/// Sorted, deduplicated section names from `posts`.
pub fn collect_sections(posts: &[BlogPost]) -> Vec<String> {
    let mut sections: Vec<String> = posts.iter().filter_map(|p| p.section.clone()).collect();
    sections.sort();
    sections.dedup();
    sections
}

pub fn is_draft_md(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".draft.md"))
}

/// `post.draft.md` → `post.md`
pub fn published_path_from_draft(draft: &Path) -> Option<PathBuf> {
    let name = draft.file_name()?.to_str()?;
    let published = name.strip_suffix(".draft.md")?;
    Some(draft.with_file_name(format!("{published}.md")))
}

pub fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("post")
        .to_string()
}

impl BlogPostStore {
    fn post_id_for(&self, object_id: Option<ObjectId>, path: &Path) -> Option<u64> {
        object_id
            .and_then(|oid| self.by_object_id.get(&oid).copied())
            .or_else(|| self.by_path.get(path).copied())
    }

    fn reindex(&mut self, store_id: u64, previous: &BlogPost, current: &BlogPost) {
        if previous.path != current.path {
            self.by_path.remove(&previous.path);
        }
        self.by_path.insert(current.path.clone(), store_id);

        if let Some(previous_oid) = previous.object_id {
            if Some(previous_oid) != current.object_id {
                self.by_object_id.remove(&previous_oid);
            }
        }
        if let Some(current_oid) = current.object_id {
            self.by_object_id.insert(current_oid, store_id);
        }
    }

    fn upsert_post(&mut self, mut post: BlogPost) -> BlogPost {
        post.section = section_from_path(&post.path);
        if let Some(store_id) = self.post_id_for(post.object_id, &post.path) {
            let previous = self
                .posts
                .get(&store_id)
                .cloned()
                .expect("existing store id must exist");
            let mut merged = post;
            if merged.object_id.is_none() {
                merged.object_id = previous.object_id;
            }
            if merged.publication_date.is_none() {
                merged.publication_date = previous.publication_date;
            }
            self.posts.insert(store_id, merged.clone());
            self.reindex(store_id, &previous, &merged);
            return merged;
        }

        let store_id = self.next_store_id;
        self.next_store_id += 1;
        self.posts.insert(store_id, post.clone());
        self.by_path.insert(post.path.clone(), store_id);
        if let Some(oid) = post.object_id {
            self.by_object_id.insert(oid, store_id);
        }
        post
    }

    fn update_by_id(&mut self, store_id: u64, update: BlogPostUpdate) -> Option<BlogPost> {
        let previous = self.posts.get(&store_id).cloned()?;
        let mut current = previous.clone();
        current.apply_update(update);
        self.posts.insert(store_id, current.clone());
        self.reindex(store_id, &previous, &current);
        Some(current)
    }
}

pub fn upsert(post: BlogPost) -> BlogPost {
    BLOG_POST_STORE.with(|store| store.borrow_mut().upsert_post(post))
}

pub fn upsert_with_defaults(
    path: PathBuf,
    object_id: Option<ObjectId>,
    last_updated: DateTime<FixedOffset>,
) -> BlogPost {
    let mut post = BlogPost::with_defaults(path, last_updated);
    post.object_id = object_id;
    upsert(post)
}

pub fn register_object_path(
    object_id: ObjectId,
    path: PathBuf,
    last_updated: DateTime<FixedOffset>,
) -> BlogPost {
    let existing = get_by_object_id(&object_id).or_else(|| get_by_path(&path));
    let mut post = existing.unwrap_or_else(|| BlogPost::with_defaults(path.clone(), last_updated));
    post.object_id = Some(object_id);
    post.path = path.clone();
    post.section = section_from_path(&path);
    if last_updated > post.last_updated {
        post.last_updated = last_updated;
    }
    upsert(post)
}

pub fn try_set_publication_date(path: &Path, date: DateTime<FixedOffset>) {
    let Some(post) = get_by_path(path) else {
        return;
    };
    if post.publication_date.is_none() {
        let _ = update_by_path(
            path,
            BlogPostUpdate {
                publication_date: Some(date),
                ..BlogPostUpdate::default()
            },
        );
    }
}

pub fn set_publication_date(path: &Path, date: DateTime<FixedOffset>) {
    let _ = update_by_path(
        path,
        BlogPostUpdate {
            publication_date: Some(date),
            ..BlogPostUpdate::default()
        },
    );
}

pub fn transfer_post_to_path(
    from: &Path,
    to: PathBuf,
    object_id: ObjectId,
    last_updated: DateTime<FixedOffset>,
) {
    let existing = get_by_path(from).or_else(|| get_by_object_id(&object_id));
    let mut post = existing.unwrap_or_else(|| BlogPost::with_defaults(to.clone(), last_updated));
    post.path = to.clone();
    post.section = section_from_path(&to);
    post.object_id = Some(object_id);
    if last_updated > post.last_updated {
        post.last_updated = last_updated;
    }
    upsert(post);
}

pub fn get_by_object_id(object_id: &ObjectId) -> Option<BlogPost> {
    BLOG_POST_STORE.with(|store| {
        let store = store.borrow();
        let id = store.by_object_id.get(object_id)?;
        store.posts.get(id).cloned()
    })
}

pub fn get_by_path(path: &Path) -> Option<BlogPost> {
    BLOG_POST_STORE.with(|store| {
        let store = store.borrow();
        let id = store.by_path.get(path)?;
        store.posts.get(id).cloned()
    })
}

pub fn update_by_object_id(object_id: &ObjectId, update: BlogPostUpdate) -> Option<BlogPost> {
    BLOG_POST_STORE.with(|store| {
        let mut store = store.borrow_mut();
        let store_id = *store.by_object_id.get(object_id)?;
        store.update_by_id(store_id, update)
    })
}

pub fn update_by_path(path: &Path, update: BlogPostUpdate) -> Option<BlogPost> {
    BLOG_POST_STORE.with(|store| {
        let mut store = store.borrow_mut();
        let store_id = *store.by_path.get(path)?;
        store.update_by_id(store_id, update)
    })
}

pub fn all() -> Vec<BlogPost> {
    BLOG_POST_STORE.with(|store| store.borrow().posts.values().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_path_from_draft_strips_suffix() {
        let draft = PathBuf::from("notes/post.draft.md");
        assert_eq!(
            published_path_from_draft(&draft),
            Some(PathBuf::from("notes/post.md"))
        );
    }

    #[test]
    fn is_draft_md_matches_pattern() {
        assert!(is_draft_md(Path::new("a.draft.md")));
        assert!(!is_draft_md(Path::new("a.md")));
    }

    #[test]
    fn section_from_path_uses_top_level_folder() {
        assert_eq!(
            section_from_path(Path::new("tech/hello.md")),
            Some("tech".to_string())
        );
        assert_eq!(
            section_from_path(Path::new("tech/sub/hello.md")),
            Some("tech".to_string())
        );
        assert_eq!(section_from_path(Path::new("hello.md")), None);
    }
}
