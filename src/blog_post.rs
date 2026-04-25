use std::path::{Path, PathBuf};

use crate::markdown::parse_title_and_summary;
use chrono::{DateTime, FixedOffset};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlogPost {
    pub last_updated: DateTime<FixedOffset>,
    pub title: String,
    pub summary: String,
    pub path: PathBuf,
}

impl BlogPost {
    pub fn new(
        path: PathBuf,
        last_updated: DateTime<FixedOffset>,
        title: String,
        summary: String,
    ) -> Self {
        Self {
            last_updated,
            title,
            summary,
            path,
        }
    }

    pub fn with_defaults(path: PathBuf, last_updated: DateTime<FixedOffset>) -> Self {
        Self {
            title: fallback_title(&path),
            summary: String::new(),
            path,
            last_updated,
        }
    }

    pub fn update_from_atom(
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

    pub fn from_atom(
        path: PathBuf,
        title: String,
        summary: String,
        last_updated: DateTime<FixedOffset>,
    ) -> Self {
        Self {
            path,
            title,
            summary,
            last_updated,
        }
    }

    pub fn update_from_markdown(&mut self, markdown: &str) {
        let fallback = fallback_title(&self.path);
        let (title, summary) = parse_title_and_summary(markdown, &fallback);
        self.title = title;
        self.summary = summary;
    }
}

pub fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("post")
        .to_string()
}
