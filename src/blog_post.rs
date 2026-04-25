use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use gix::ObjectId;

use crate::markdown::parse_title_and_summary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlogPost {
    pub object_id: Option<ObjectId>,
    pub last_updated: DateTime<FixedOffset>,
    pub title: String,
    pub summary: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct BlogPostUpdate {
    pub object_id: Option<ObjectId>,
    pub last_updated: Option<DateTime<FixedOffset>>,
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
        Self {
            object_id: None,
            last_updated,
            title,
            summary,
            path,
        }
    }

    pub fn with_defaults(path: PathBuf, last_updated: DateTime<FixedOffset>) -> Self {
        Self {
            object_id: None,
            title: fallback_title(&path),
            summary: String::new(),
            path,
            last_updated,
        }
    }

    pub fn update_from_source(
        &mut self,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
        path: PathBuf,
    ) {
        self.path = path;
        self.last_updated = last_updated;
        self.title = title;
        self.summary = summary;
    }

    pub fn from_source(
        path: PathBuf,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
    ) -> Self {
        Self {
            object_id: None,
            path,
            title,
            summary,
            last_updated,
        }
    }

    pub fn update_from_source_content(&mut self, content: &str) {
        let fallback = fallback_title(&self.path);
        let (title, summary) = parse_title_and_summary(content, &fallback);
        self.title = title;
        self.summary = summary;
    }

    fn apply_update(&mut self, update: BlogPostUpdate) {
        if let Some(object_id) = update.object_id {
            self.object_id = Some(object_id);
        }
        if let Some(last_updated) = update.last_updated {
            self.last_updated = last_updated;
        }
        if let Some(title) = update.title {
            self.title = title;
        }
        if let Some(summary) = update.summary {
            self.summary = summary;
        }
        if let Some(path) = update.path {
            self.path = path;
        }
    }
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

    fn upsert_post(&mut self, post: BlogPost) -> BlogPost {
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
    post.path = path;
    if last_updated > post.last_updated {
        post.last_updated = last_updated;
    }
    upsert(post)
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
