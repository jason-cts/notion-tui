use super::*;
use crate::comments::{Comment, CommentTarget, CommentWrite, grouped};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Debug)]
enum Presence {
    Known(bool),
    Pending(u64),
    Unavailable,
}

#[derive(Default)]
pub struct CommentIndicators {
    cache: HashMap<String, Presence>,
    active: HashSet<u64>,
    serial: u64,
    next_start: Option<Instant>,
    access: CommentAccess,
    retry_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CommentAccess {
    #[default]
    Unchecked,
    Checking(u64),
    Available,
    Denied,
    TemporarilyUnavailable,
}

impl CommentIndicators {
    pub(super) fn new(demo: bool) -> Self {
        Self {
            access: if demo {
                CommentAccess::Available
            } else {
                CommentAccess::Unchecked
            },
            ..Self::default()
        }
    }
}

async fn comment_read<T>(
    read: impl std::future::Future<Output = anyhow::Result<T>>,
) -> Result<T, CommentReadError> {
    tokio::time::timeout(Duration::from_secs(30), read)
        .await
        .map_err(|_| CommentReadError::Other("Comment request timed out".into()))?
        .map_err(CommentReadError::from)
}

pub struct CommentsModal {
    pub block_id: String,
    pub page: bool,
    pub label: BlockLabel,
    pub generation: u64,
    pub items: Vec<Comment>,
    pub selected: usize,
    pub scroll: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    select_after: Option<String>,
}

impl App {
    fn begin_comment_access(&mut self) -> u64 {
        let indicators = &mut self.comment_indicators;
        // Explicit reads supersede older probes and pending indicator results.
        indicators.serial += 1;
        indicators.access = CommentAccess::Checking(indicators.serial);
        indicators.active.clear();
        indicators
            .cache
            .retain(|_, presence| !matches!(presence, Presence::Pending(_)));
        indicators.serial
    }

    fn pause_comment_access(&mut self, error: &CommentReadError) {
        let indicators = &mut self.comment_indicators;
        indicators.access = if matches!(error, CommentReadError::Denied) {
            indicators.cache.clear();
            CommentAccess::Denied
        } else {
            CommentAccess::TemporarilyUnavailable
        };
        indicators.active.clear();
        indicators
            .cache
            .retain(|_, presence| !matches!(presence, Presence::Pending(_)));
        if let CommentReadError::RateLimited(delay) = error {
            let until = Instant::now() + *delay;
            indicators.retry_at = Some(indicators.retry_at.map_or(until, |old| old.max(until)));
        }
    }

    fn finish_comment_access<T>(
        &mut self,
        request: u64,
        result: &Result<T, CommentReadError>,
    ) -> bool {
        if self.comment_indicators.access != CommentAccess::Checking(request) {
            return false;
        }
        match result {
            Ok(_) => {
                self.comment_indicators.access = CommentAccess::Available;
                self.comment_indicators
                    .cache
                    .retain(|_, presence| matches!(presence, Presence::Known(_)));
            }
            Err(error) => self.pause_comment_access(error),
        }
        true
    }

    pub(super) fn retry_temporary_comment_access(&mut self) {
        if self.comment_indicators.access == CommentAccess::TemporarilyUnavailable {
            self.comment_indicators.access = if self.demo {
                CommentAccess::Available
            } else {
                CommentAccess::Unchecked
            };
        }
    }

    pub(super) fn probe_comment_access(&mut self) {
        if self.demo
            || self.comment_indicators.access != CommentAccess::Unchecked
            || self
                .comment_indicators
                .retry_at
                .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let Some(id) = self
            .pages
            .iter()
            .find(|page| !page.id.is_empty())
            .map(|page| page.id.clone())
            .or_else(|| {
                self.location()
                    .map(|location| location.page_id.clone())
                    .filter(|id| !id.is_empty())
            })
        else {
            return;
        };
        let request = self.begin_comment_access();
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = comment_read(service.has_comments(&id)).await;
            let _ = sender.send(Message::CommentAccess { request, result });
        });
    }

    fn comment_access_notice(error: &CommentReadError) -> String {
        match error {
            CommentReadError::Denied => error.to_string(),
            CommentReadError::RateLimited(_) => format!("{error}; press c or r to retry."),
            CommentReadError::Other(_) => {
                "Comment indicators unavailable; press c or r to retry.".into()
            }
        }
    }

    pub(super) fn receive_comment_access(
        &mut self,
        request: u64,
        result: Result<bool, CommentReadError>,
    ) {
        if self.finish_comment_access(request, &result)
            && let Err(error) = result
        {
            self.fail(Self::comment_access_notice(&error));
        }
    }

    pub fn block_has_comments(&self, id: &str) -> bool {
        matches!(
            self.comment_indicators.cache.get(id),
            Some(Presence::Known(true))
        )
    }

    pub(super) fn reset_comment_checks(&mut self) {
        self.comment_indicators
            .cache
            .retain(|_, presence| !matches!(presence, Presence::Pending(_)));
        self.comments = None;
    }

    pub(super) fn invalidate_comment_indicators(&mut self) {
        for block in &self.blocks {
            self.comment_indicators.cache.remove(&block.id);
        }
    }

    pub fn check_visible_comments(&mut self, visible: &[String]) {
        self.probe_comment_access();
        if self.comment_indicators.access != CommentAccess::Available
            || self.loading_content
            || self.database().is_some()
            || self
                .location()
                .is_some_and(|location| location.kind == LocationKind::Database)
            || self.comments.is_some()
            || self.editor.is_some()
            || self.help
            || self.viewer.is_some()
            || self.search.is_some()
            || self.link_picker.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        let indicators = &mut self.comment_indicators;
        let now = Instant::now();
        if indicators.active.len() >= 2
            || indicators.next_start.is_some_and(|next| now < next)
            || indicators.retry_at.is_some_and(|until| now < until)
        {
            return;
        }
        let Some(id) = visible.iter().find(|id| {
            !id.is_empty()
                && !indicators.cache.contains_key(*id)
                && self.blocks.iter().any(|block| block.id == **id)
        }) else {
            return;
        };
        let id = id.clone();
        indicators.serial += 1;
        let request = indicators.serial;
        indicators
            .cache
            .insert(id.clone(), Presence::Pending(request));
        indicators.active.insert(request);
        // A start interval prevents bursts, including when reads complete immediately.
        indicators.next_start = Some(now + Duration::from_millis(334));
        let generation = self.content_generation;
        let sender = self.sender.clone();
        let service = self.service.clone();
        tokio::spawn(async move {
            let result = comment_read(service.has_comments(&id)).await;
            let _ = sender.send(Message::CommentPresence {
                id,
                request,
                generation,
                result,
            });
        });
    }

    pub(super) fn receive_comment_presence(
        &mut self,
        id: String,
        request: u64,
        generation: u64,
        result: Result<bool, CommentReadError>,
    ) {
        let indicators = &mut self.comment_indicators;
        indicators.active.remove(&request);
        if !matches!(indicators.cache.get(&id), Some(Presence::Pending(pending)) if *pending == request)
        {
            return;
        }
        if generation != self.content_generation {
            indicators.cache.remove(&id);
            return;
        }
        match result {
            Ok(present) => {
                indicators.cache.insert(id, Presence::Known(present));
            }
            Err(error) => {
                indicators.cache.insert(id, Presence::Unavailable);
                self.pause_comment_access(&error);
                self.fail(Self::comment_access_notice(&error));
            }
        }
    }

    pub(super) fn open_comments(&mut self) {
        if self.focus != Focus::Content
            || self.loading_content
            || self.database().is_some()
            || self
                .location()
                .is_none_or(|location| location.kind == LocationKind::Database)
        {
            return;
        }
        let Some(block) = self
            .blocks
            .get(self.block_selected)
            .filter(|b| !b.id.is_empty())
        else {
            return;
        };
        self.comments = Some(CommentsModal {
            block_id: block.id.clone(),
            page: block.child_page,
            label: self.block_labels()[self.block_selected].clone(),
            generation: 0,
            items: vec![],
            selected: 0,
            scroll: 0,
            loading: false,
            error: None,
            notice: None,
            select_after: None,
        });
        self.load_comments();
    }

    fn load_comments(&mut self) {
        if self.comments.is_none() {
            return;
        }
        if let Some(until) = self
            .comment_indicators
            .retry_at
            .filter(|until| Instant::now() < *until)
        {
            let error =
                CommentReadError::RateLimited(until.saturating_duration_since(Instant::now()))
                    .to_string();
            let modal = self.comments.as_mut().unwrap();
            modal.loading = false;
            modal.error = Some(error.clone());
            self.fail(error);
            return;
        }
        let access_request = self.begin_comment_access();
        let modal = self.comments.as_mut().unwrap();
        self.comment_generation += 1;
        modal.generation = self.comment_generation;
        modal.loading = true;
        modal.error = None;
        let id = modal.block_id.clone();
        let generation = modal.generation;
        let sender = self.sender.clone();
        let service = self.service.clone();
        tokio::spawn(async move {
            let result = comment_read(service.comments(&id)).await;
            let _ = sender.send(Message::CommentsLoaded {
                id,
                generation,
                access_request,
                result,
            });
        });
    }

    pub(super) fn receive_comments(
        &mut self,
        id: String,
        generation: u64,
        access_request: u64,
        result: Result<Vec<Comment>, CommentReadError>,
    ) {
        // Access is session-wide, even if the modal closed during this read.
        if !self.finish_comment_access(access_request, &result) {
            return;
        }
        let Some(modal) = &mut self.comments else {
            return;
        };
        if modal.block_id != id || modal.generation != generation {
            return;
        }
        modal.loading = false;
        match result {
            Ok(comments) => {
                let selected_id = modal
                    .select_after
                    .take()
                    .or_else(|| modal.items.get(modal.selected).map(|c| c.id.clone()));
                modal.items = grouped(comments);
                modal.selected = selected_id
                    .and_then(|id| modal.items.iter().position(|c| c.id == id))
                    .unwrap_or(modal.selected.min(modal.items.len().saturating_sub(1)));
                modal.scroll = 0;
                modal.error = None;
                self.comment_indicators
                    .cache
                    .insert(id, Presence::Known(!modal.items.is_empty()));
                self.status = modal
                    .notice
                    .clone()
                    .unwrap_or_else(|| format!("{} comments", modal.items.len()));
                self.error = false;
            }
            Err(error) => {
                let error = if let Some(notice) = &modal.notice {
                    format!("{notice}. Could not refresh comments: {error}")
                } else {
                    error.to_string()
                };
                modal.error = Some(error.clone());
                self.fail(error);
            }
        }
    }

    pub(super) fn comments_key(&mut self, key: KeyEvent) -> bool {
        let Some(modal) = &mut self.comments else {
            return false;
        };
        if key.code == KeyCode::Esc {
            self.comments = None;
            return true;
        }
        if modal.loading {
            return true;
        }
        match key.code {
            KeyCode::Down => {
                modal.selected = (modal.selected + 1).min(modal.items.len().saturating_sub(1));
                modal.scroll = 0;
            }
            KeyCode::Up => {
                modal.selected = modal.selected.saturating_sub(1);
                modal.scroll = 0;
            }
            KeyCode::Home => {
                modal.selected = 0;
                modal.scroll = 0;
            }
            KeyCode::End => {
                modal.selected = modal.items.len().saturating_sub(1);
                modal.scroll = 0;
            }
            KeyCode::PageDown => modal.scroll = modal.scroll.saturating_add(10),
            KeyCode::PageUp => modal.scroll = modal.scroll.saturating_sub(10),
            KeyCode::Char('r') => {
                modal.notice = None;
                self.load_comments();
            }
            KeyCode::Char('a') if modal.error.is_none() => {
                let target = if modal.page {
                    CommentTarget::Page(modal.block_id.clone())
                } else {
                    CommentTarget::Block(modal.block_id.clone())
                };
                self.editor = Some(Editor::new(Target::Comment(target)));
            }
            KeyCode::Enter if modal.error.is_none() => {
                if let Some(comment) = modal.items.get(modal.selected) {
                    self.editor = Some(Editor::new(Target::Comment(CommentTarget::Discussion(
                        comment.discussion.clone(),
                    ))));
                }
            }
            KeyCode::Char('e') if modal.error.is_none() => {
                if let Some(comment) = modal.items.get(modal.selected) {
                    if comment.input.is_none() {
                        modal.notice = Some("Comments with mentions, equations, or unsupported rich text are read-only".into());
                        self.fail("Comments with mentions, equations, or unsupported rich text are read-only");
                    } else {
                        self.editor = Some(Editor::new(Target::Comment(CommentTarget::Edit(
                            Box::new(comment.clone()),
                        ))));
                    }
                }
            }
            KeyCode::Char('d') if modal.error.is_none() && !modal.items.is_empty() => {
                self.confirm = Some(Confirm::DeleteComment);
            }
            _ => {}
        }
        true
    }

    pub(super) fn delete_comment(&mut self) {
        let Some(comment) = self
            .comments
            .as_ref()
            .and_then(|m| m.items.get(m.selected))
            .cloned()
        else {
            return;
        };
        self.save_comment(CommentWrite::Delete(Box::new(comment)));
    }

    pub(super) fn save_comment(&mut self, write: CommentWrite) {
        let Some(modal) = &self.comments else {
            return;
        };
        let id = modal.block_id.clone();
        let generation = modal.generation;
        let deleting = matches!(write, CommentWrite::Delete(_));
        self.saving = true;
        self.error = false;
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = service
                .comment_write(write)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::CommentSaved {
                id,
                generation,
                deleting,
                result,
            });
        });
    }

    pub(super) fn receive_comment_saved(
        &mut self,
        id: String,
        generation: u64,
        deleting: bool,
        result: Result<String, String>,
    ) {
        let Some(modal) = &mut self.comments else {
            return;
        };
        if modal.block_id != id || modal.generation != generation {
            return;
        }
        self.saving = false;
        match result {
            Ok(comment_id) => {
                self.editor = None;
                modal.error = None;
                if deleting {
                    modal.items.retain(|c| c.id != comment_id);
                } else {
                    modal.select_after = Some(comment_id);
                }
                self.comment_indicators
                    .cache
                    .insert(id, Presence::Known(!deleting || !modal.items.is_empty()));
                let notice = if deleting {
                    "Comment deleted"
                } else {
                    "Comment saved"
                };
                modal.notice = Some(notice.into());
                self.status = notice.into();
                self.error = false;
                self.load_comments();
            }
            Err(error) => {
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(error.clone());
                } else {
                    modal.error = Some(error.clone());
                }
                self.fail(error);
            }
        }
    }
}

#[cfg(test)]
mod tests;
