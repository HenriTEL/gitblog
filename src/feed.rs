use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use std::io::Cursor;

use crate::blog_post::BlogPost;

#[derive(Debug, Deserialize)]
pub struct Author {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Link {
    #[serde(rename = "@href")]
    pub href: String,
    #[serde(rename = "@rel", default)]
    pub rel: String,
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub updated: DateTime<FixedOffset>,
    pub link: Link,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Deserialize)]
pub struct Feed {
    pub id: String,
    pub title: String,
    pub updated: DateTime<FixedOffset>,
    pub author: Author,
    pub link: Link,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(rename = "entry", default)]
    pub entries: Vec<Entry>,
}

pub fn parse(xml: &str) -> Result<Feed, quick_xml::DeError> {
    quick_xml::de::from_str(xml)
}

pub fn build_feed_from_blog_posts(
    blog_url: &str,
    posts: &[BlogPost],
    previous: Option<&Feed>,
) -> Feed {
    let base_url = blog_url.trim_end_matches('/');
    let mut sorted_posts = posts.to_vec();
    sorted_posts.sort_by(|a, b| {
        b.last_updated
            .cmp(&a.last_updated)
            .then_with(|| a.path.cmp(&b.path))
    });

    let entries = sorted_posts
        .iter()
        .map(|post| {
            let rel_html = post.path.with_extension("html").to_string_lossy().to_string();
            let href = format!("{base_url}/{rel_html}");
            Entry {
                id: href.clone(),
                title: post.title.clone(),
                updated: post.last_updated,
                link: Link {
                    href,
                    rel: "alternate".to_string(),
                },
                summary: post.summary.clone(),
            }
        })
        .collect::<Vec<_>>();

    let fallback_updated = previous
        .map(|f| f.updated)
        .unwrap_or_else(|| chrono::Utc::now().fixed_offset());
    let updated = sorted_posts
        .iter()
        .map(|p| p.last_updated)
        .max()
        .unwrap_or(fallback_updated);

    Feed {
        id: previous
            .map(|f| f.id.clone())
            .unwrap_or_else(|| format!("{base_url}/atom.xml")),
        title: previous
            .map(|f| f.title.clone())
            .unwrap_or_else(|| "Blog".to_string()),
        updated,
        author: Author {
            name: previous
                .map(|f| f.author.name.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
        },
        link: Link {
            href: base_url.to_string(),
            rel: "alternate".to_string(),
        },
        logo: previous.and_then(|f| f.logo.clone()),
        subtitle: previous.and_then(|f| f.subtitle.clone()),
        entries,
    }
}

pub fn generate(feed: &Feed) -> Result<String, quick_xml::Error> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));

    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut feed_start = BytesStart::new("feed");
    feed_start.push_attribute(("xmlns", "http://www.w3.org/2005/Atom"));
    writer.write_event(Event::Start(feed_start))?;

    write_text_elem(&mut writer, "id", &feed.id)?;
    write_text_elem(&mut writer, "title", &feed.title)?;
    write_text_elem(&mut writer, "updated", &feed.updated.to_rfc3339())?;

    writer.write_event(Event::Start(BytesStart::new("author")))?;
    write_text_elem(&mut writer, "name", &feed.author.name)?;
    writer.write_event(Event::End(BytesEnd::new("author")))?;

    let mut link_el = BytesStart::new("link");
    link_el.push_attribute(("href", feed.link.href.as_str()));
    writer.write_event(Event::Empty(link_el))?;

    if let Some(logo) = &feed.logo {
        write_text_elem(&mut writer, "logo", logo)?;
    }
    if let Some(subtitle) = &feed.subtitle {
        write_text_elem(&mut writer, "subtitle", subtitle)?;
    }

    for entry in &feed.entries {
        writer.write_event(Event::Start(BytesStart::new("entry")))?;

        write_text_elem(&mut writer, "id", &entry.id)?;
        write_text_elem(&mut writer, "title", &entry.title)?;
        write_text_elem(&mut writer, "updated", &entry.updated.to_rfc3339())?;

        let mut link_el = BytesStart::new("link");
        link_el.push_attribute(("href", entry.link.href.as_str()));
        link_el.push_attribute(("rel", "alternate"));
        writer.write_event(Event::Empty(link_el))?;

        if entry.summary.is_empty() {
            writer.write_event(Event::Empty(BytesStart::new("summary")))?;
        } else {
            write_text_elem(&mut writer, "summary", &entry.summary)?;
        }

        writer.write_event(Event::End(BytesEnd::new("entry")))?;
    }

    writer.write_event(Event::End(BytesEnd::new("feed")))?;

    let bytes = writer.into_inner().into_inner();
    Ok(String::from_utf8(bytes).expect("valid UTF-8"))
}

fn write_text_elem<W: std::io::Write>(
    writer: &mut Writer<W>,
    name: &str,
    text: &str,
) -> Result<(), quick_xml::Error> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}
