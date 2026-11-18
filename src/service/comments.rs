use super::*;
use crate::comments::{Comment, CommentTarget, CommentWrite, check_snapshot, validate_text};
use rusticnotion::{
    ids::{CommentId, DiscussionId},
    models::{
        comment::{CommentCreateRequest, CommentUpdateRequest},
        paging::Paging,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommentReadError {
    Denied,
    RateLimited(Duration),
    Other(String),
}

impl From<anyhow::Error> for CommentReadError {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast_ref::<rusticnotion::Error>() {
            Some(rusticnotion::Error::ApiError { error }) if error.status.code() == 403 => {
                Self::Denied
            }
            Some(rusticnotion::Error::RateLimited { retry_after_secs }) => {
                Self::RateLimited(Duration::from_secs((*retry_after_secs).max(1)))
            }
            _ => Self::Other(format!("{error:#}")),
        }
    }
}

impl std::fmt::Display for CommentReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(
                f,
                "Comments unavailable: enable Read comments. Press c on a block to retry."
            ),
            Self::RateLimited(delay) => write!(
                f,
                "Comments rate limited; retry in {} seconds",
                delay.as_secs().max(1)
            ),
            Self::Other(message) => f.write_str(message),
        }
    }
}

fn comment_error(error: rusticnotion::Error, writing: bool) -> anyhow::Error {
    let denied =
        matches!(&error, rusticnotion::Error::ApiError { error } if error.status.code() == 403);
    let error: anyhow::Error = error.into();
    if denied {
        error.context(if writing {
            "Comment change denied. Check Insert comments and whether this integration may change the comment"
        } else {
            "Comment access denied. Enable Read comments for your integration"
        })
    } else {
        error
    }
}

impl ApiService {
    pub(super) async fn comment_presence(&self, id: &str) -> Result<bool> {
        let response = self
            .api
            .list_comments(
                id.parse::<BlockId>()?,
                Paging {
                    page_size: Some(1),
                    ..Paging::default()
                },
            )
            .await
            .map_err(|error| comment_error(error, false))
            .context("Could not load comments")?;
        Ok(!response.results.is_empty())
    }

    pub(super) async fn list_comments(&self, id: &str) -> Result<Vec<Comment>> {
        let id = id.parse::<BlockId>()?;
        let mut comments = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let response = self
                .api
                .list_comments(
                    &id,
                    Paging {
                        start_cursor: cursor,
                        page_size: Some(100),
                    },
                )
                .await
                .map_err(|error| comment_error(error, false))
                .context("Could not load comments")?;
            for comment in response.results {
                comments.push(Comment::from_value(&serde_json::to_value(comment)?)?);
            }
            cursor = next_cursor(response.has_more, response.next_cursor, &mut seen)?;
            if cursor.is_none() {
                return Ok(comments);
            }
        }
    }

    async fn check_comment(&self, original: &Comment) -> Result<()> {
        let current = self
            .api
            .get_comment(original.id.parse::<CommentId>()?)
            .await
            .map_err(|error| comment_error(error, false))
            .context("Could not check comment for remote changes")?;
        check_snapshot(
            original,
            &Comment::from_value(&serde_json::to_value(current)?)?,
        )
    }

    pub(super) async fn mutate_comment(&self, write: CommentWrite) -> Result<String> {
        let response = match write {
            CommentWrite::Save { target, text } => {
                validate_text(&text).map_err(anyhow::Error::msg)?;
                match target {
                    CommentTarget::Edit(original) => {
                        self.check_comment(&original).await?;
                        self.api
                            .update_comment(
                                original.id.parse::<CommentId>()?,
                                CommentUpdateRequest { rich_text: text.0 },
                            )
                            .await
                    }
                    target => {
                        let request = match target {
                            CommentTarget::Block(id) => {
                                CommentCreateRequest::on_block(id.parse::<BlockId>()?, text.0)
                            }
                            CommentTarget::Page(id) => {
                                CommentCreateRequest::on_page(id.parse::<PageId>()?, text.0)
                            }
                            CommentTarget::Discussion(id) => {
                                CommentCreateRequest::reply(id.parse::<DiscussionId>()?, text.0)
                            }
                            CommentTarget::Edit(_) => unreachable!(),
                        };
                        self.api.create_comment(request).await
                    }
                }
            }
            CommentWrite::Delete(original) => {
                self.check_comment(&original).await?;
                self.api
                    .delete_comment(original.id.parse::<CommentId>()?)
                    .await
            }
        }
        .map_err(|error| comment_error(error, true))
        .context(
            "Comment write failed; inspect Notion before retrying if the response was unavailable",
        )?;
        Ok(response.id.to_string())
    }
}

pub(super) fn demo_comments(id: &str) -> Vec<Comment> {
    if id != "demo-1" && id != "demo-rich-text" {
        return vec![];
    }
    ["Press c on a block with 💬 to read its comments.", "Use a to compose, Enter to reply, e to edit, or d to delete. Demo stays read-only."]
        .into_iter().enumerate().map(|(i, text)| Comment::from_value(&json!({
            "id":format!("demo-comment-{i}"), "discussion_id":"demo-discussion",
            "created_time":format!("2026-01-01T12:0{i}:00Z"), "last_edited_time":"2026-01-01T12:00:00Z",
            "display_name":{"resolved_name":"Notion TUI"},
            "rich_text":[{"type":"text","text":{"content":text},"annotations":{"bold":i == 0}}]
        })).expect("valid demo comment")).collect()
}

#[cfg(test)]
mod tests;
