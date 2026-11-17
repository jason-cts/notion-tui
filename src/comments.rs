use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    model::{RichText, TableCell},
    rich_text::RichRun,
};

#[derive(Clone, Debug)]
pub struct Comment {
    pub id: String,
    pub discussion: String,
    pub modified: String,
    pub created: String,
    pub author: String,
    pub runs: Vec<RichRun>,
    pub input: Option<RichText>,
    pub attachments: usize,
    pub pending_attachments: bool,
}

impl Comment {
    pub fn from_value(value: &Value) -> Result<Self> {
        let required = |key: &str| -> Result<String> {
            Ok(value[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .with_context(|| format!("Comment response missing {key}"))?
                .to_owned())
        };
        let runs = value["rich_text"]
            .as_array()
            .context("Comment response missing rich_text")?;
        let cell = TableCell::from_value(&value["rich_text"]);
        Ok(Self {
            id: required("id")?,
            discussion: required("discussion_id")?,
            modified: required("last_edited_time")?,
            created: required("created_time")?,
            author: value["display_name"]["resolved_name"]
                .as_str()
                .or_else(|| value["created_by"]["name"].as_str())
                .or_else(|| value["created_by"]["id"].as_str())
                .unwrap_or("Unknown author")
                .to_owned(),
            runs: runs.iter().map(RichRun::from_value).collect(),
            input: cell.input.map(RichText),
            attachments: value["attachments"].as_array().map_or(0, Vec::len),
            pending_attachments: value["has_pending_attachments"].as_bool().unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug)]
pub enum CommentTarget {
    Block(String),
    Page(String),
    Discussion(String),
    Edit(Box<Comment>),
}

#[derive(Clone, Debug)]
pub enum CommentWrite {
    Save {
        target: CommentTarget,
        text: RichText,
    },
    Delete(Box<Comment>),
}

pub fn validate_text(text: &RichText) -> Result<(), &'static str> {
    if text.plain().trim().is_empty() {
        return Err("A comment cannot be blank");
    }
    if text.0.len() > 100 || text.plain().chars().count() > 100_000 {
        return Err("Comment exceeds the API limit (100 rich-text runs / 100,000 characters)");
    }
    if text.0.iter().any(|run| matches!(&run.content,
        rusticnotion::models::requests::RichTextContent::Text { text } if text.content.chars().count() > 2000)) {
        return Err("Comment text runs are limited to 2,000 characters");
    }
    Ok(())
}

/// Keep discussions together without losing the chronological order within them.
pub fn grouped(mut comments: Vec<Comment>) -> Vec<Comment> {
    comments.sort_by(|a, b| a.created.cmp(&b.created));
    let mut threads: Vec<(String, Vec<Comment>)> = Vec::new();
    for comment in comments {
        if let Some((_, items)) = threads.iter_mut().find(|(id, _)| *id == comment.discussion) {
            items.push(comment);
        } else {
            threads.push((comment.discussion.clone(), vec![comment]));
        }
    }
    threads.into_iter().flat_map(|(_, items)| items).collect()
}

pub fn check_snapshot(original: &Comment, current: &Comment) -> Result<()> {
    if original.id != current.id
        || original.modified.is_empty()
        || original.modified != current.modified
    {
        bail!(
            "Conflict: comment changed in Notion. Draft retained; cancel and refresh before trying again"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unsupported_runs_remain_visible_but_cannot_be_edited() {
        let comment = Comment::from_value(&json!({"id":"comment","discussion_id":"thread",
            "created_time":"now","last_edited_time":"now","rich_text":[
                {"type":"mention","plain_text":"@Person","mention":{"type":"user","user":{"id":"user"}}}
            ],"attachments":[{}],"has_pending_attachments":true})).unwrap();
        assert!(comment.input.is_none());
        assert_eq!(comment.runs[0].text, "@Person");
        assert_eq!(comment.attachments, 1);
        assert!(comment.pending_attachments);
    }

    #[test]
    fn comment_limits_count_chunked_unicode_runs_and_styled_fragments() {
        assert!(validate_text(&RichText::from("界".repeat(100_000))).is_ok());
        assert!(validate_text(&RichText::from("界".repeat(100_001))).is_err());
        let run = RichText::from("a").0.remove(0);
        assert!(validate_text(&RichText(vec![run; 101])).is_err());
        assert!(
            validate_text(&RichText(vec![
                rusticnotion::models::requests::RichTextInput::from("界".repeat(2001))
            ]))
            .is_err()
        );
    }

    #[test]
    fn discussions_stay_grouped_in_chronological_order() {
        let make = |id, thread, time| {
            Comment::from_value(&json!({
            "id":id,"discussion_id":thread,"created_time":time,"last_edited_time":time,"rich_text":[]
        })).unwrap()
        };
        let items = grouped(vec![
            make("last", "first-thread", "03"),
            make("other", "second-thread", "02"),
            make("first", "first-thread", "01"),
        ]);
        assert_eq!(
            items.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["first", "last", "other"]
        );
    }
}
