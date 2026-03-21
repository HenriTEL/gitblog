use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
// use quick_xml::Writer;
// use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
// use std::io::Cursor;

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

// pub fn generate(feed: &Feed) -> Result<String, quick_xml::Error> {
//     let mut writer = Writer::new(Cursor::new(Vec::new()));

//     writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

//     let mut feed_start = BytesStart::new("feed");
//     feed_start.push_attribute(("xmlns", "http://www.w3.org/2005/Atom"));
//     writer.write_event(Event::Start(feed_start))?;

//     write_text_elem(&mut writer, "id", &feed.id)?;
//     write_text_elem(&mut writer, "title", &feed.title)?;
//     write_text_elem(&mut writer, "updated", &feed.updated.to_rfc3339())?;

//     writer.write_event(Event::Start(BytesStart::new("author")))?;
//     write_text_elem(&mut writer, "name", &feed.author.name)?;
//     writer.write_event(Event::End(BytesEnd::new("author")))?;

//     let mut link_el = BytesStart::new("link");
//     link_el.push_attribute(("href", feed.link.href.as_str()));
//     writer.write_event(Event::Empty(link_el))?;

//     if let Some(logo) = &feed.logo {
//         write_text_elem(&mut writer, "logo", logo)?;
//     }
//     if let Some(subtitle) = &feed.subtitle {
//         write_text_elem(&mut writer, "subtitle", subtitle)?;
//     }

//     for entry in &feed.entries {
//         writer.write_event(Event::Start(BytesStart::new("entry")))?;

//         write_text_elem(&mut writer, "id", &entry.id)?;
//         write_text_elem(&mut writer, "title", &entry.title)?;
//         write_text_elem(&mut writer, "updated", &entry.updated.to_rfc3339())?;

//         let mut link_el = BytesStart::new("link");
//         link_el.push_attribute(("href", entry.link.href.as_str()));
//         link_el.push_attribute(("rel", "alternate"));
//         writer.write_event(Event::Empty(link_el))?;

//         if entry.summary.is_empty() {
//             writer.write_event(Event::Empty(BytesStart::new("summary")))?;
//         } else {
//             write_text_elem(&mut writer, "summary", &entry.summary)?;
//         }

//         writer.write_event(Event::End(BytesEnd::new("entry")))?;
//     }

//     writer.write_event(Event::End(BytesEnd::new("feed")))?;

//     let bytes = writer.into_inner().into_inner();
//     Ok(String::from_utf8(bytes).expect("valid UTF-8"))
// }

// fn write_text_elem<W: std::io::Write>(
//     writer: &mut Writer<W>,
//     name: &str,
//     text: &str,
// ) -> Result<(), quick_xml::Error> {
//     writer.write_event(Event::Start(BytesStart::new(name)))?;
//     writer.write_event(Event::Text(BytesText::new(text)))?;
//     writer.write_event(Event::End(BytesEnd::new(name)))?;
//     Ok(())
// }
