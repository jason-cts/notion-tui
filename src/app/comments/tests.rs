use super::*;
use crate::service::{NotionService, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

fn comment(id: &str) -> Comment {
    Comment::from_value(&serde_json::json!({"id":id,"discussion_id":"thread",
        "created_time":"2026-01-01T00:00:00Z","last_edited_time":"2026-01-01T00:00:00Z",
        "created_by":{"id":"author"},"rich_text":[{"type":"text","text":{"content":"Hello"}}]}))
    .unwrap()
}

#[derive(Default)]
struct CommentsService {
    items: Mutex<Vec<Comment>>,
    writes: Mutex<Vec<CommentWrite>>,
    probes: Mutex<Vec<String>>,
    fail_reads: AtomicBool,
    fail_writes: AtomicBool,
}

#[async_trait::async_trait]
impl NotionService for CommentsService {
    async fn search(&self, _: &str) -> anyhow::Result<Vec<PageItem>> {
        Ok(vec![])
    }
    async fn page(&self, id: &str) -> anyhow::Result<PageItem> {
        Ok(PageItem {
            id: id.into(),
            title: "Page".into(),
        })
    }
    async fn children(&self, _: &str) -> anyhow::Result<Vec<BlockItem>> {
        Ok(vec![crate::model::tests::paragraph()])
    }
    async fn write(&self, _: Write) -> anyhow::Result<Option<PageItem>> {
        anyhow::bail!("Not a block write")
    }
    async fn has_comments(&self, id: &str) -> anyhow::Result<bool> {
        self.probes.lock().unwrap().push(id.into());
        if self.fail_reads.load(Ordering::SeqCst) {
            anyhow::bail!("No read access");
        }
        Ok(!self.items.lock().unwrap().is_empty())
    }
    async fn comments(&self, _: &str) -> anyhow::Result<Vec<Comment>> {
        if self.fail_reads.load(Ordering::SeqCst) {
            anyhow::bail!("No read access");
        }
        Ok(self.items.lock().unwrap().clone())
    }
    async fn comment_write(&self, write: CommentWrite) -> anyhow::Result<String> {
        self.writes.lock().unwrap().push(write.clone());
        if self.fail_writes.load(Ordering::SeqCst) {
            anyhow::bail!("Write failed; draft retained");
        }
        let mut items = self.items.lock().unwrap();
        match write {
            CommentWrite::Save { target, text } => {
                let id = if let CommentTarget::Edit(original) = target {
                    original.id
                } else {
                    format!("new-{}", items.len())
                };
                items.retain(|c| c.id != id);
                let mut item = comment(&id);
                item.runs = vec![crate::rich_text::RichRun::plain(text.plain())];
                item.input = Some(text);
                items.push(item);
                Ok(id)
            }
            CommentWrite::Delete(original) => {
                items.retain(|c| c.id != original.id);
                Ok(original.id)
            }
        }
    }
}

fn app(service: Arc<CommentsService>) -> (App, mpsc::UnboundedReceiver<Message>) {
    let (mut app, rx) = crate::app::tests::app();
    app.service = service;
    (app, rx)
}

async fn receive(app: &mut App, rx: &mut mpsc::UnboundedReceiver<Message>) {
    app.receive(
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap(),
    );
}

async fn open(app: &mut App, rx: &mut mpsc::UnboundedReceiver<Message>) {
    app.key(KeyCode::Char('c').into());
    assert!(app.comments.as_ref().unwrap().loading);
    receive(app, rx).await;
}

fn ctrl_s() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

#[tokio::test]
async fn c_is_scoped_to_selected_real_content_and_ctrl_c_still_quits() {
    let (mut app, mut rx) = app(Arc::new(CommentsService::default()));
    app.focus = Focus::Pages;
    app.key(KeyCode::Char('c').into());
    assert!(app.comments.is_none());
    app.focus = Focus::Content;
    app.loading_content = true;
    app.key(KeyCode::Char('c').into());
    assert!(app.comments.is_none());
    app.loading_content = false;
    app.path.last_mut().unwrap().kind = LocationKind::Database;
    app.key(KeyCode::Char('c').into());
    assert!(app.comments.is_none());
    app.path.last_mut().unwrap().kind = LocationKind::Page;
    let id = std::mem::take(&mut app.blocks[0].id);
    app.key(KeyCode::Char('c').into());
    assert!(app.comments.is_none());
    app.blocks[0].id = id.clone();
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT));
    assert!(app.comments.is_none());
    open(&mut app, &mut rx).await;
    assert_eq!(app.comments.as_ref().unwrap().block_id, id);
    app.key(KeyCode::Char('q').into());
    app.key(KeyCode::Backspace.into());
    app.key(KeyCode::Tab.into());
    assert!(!app.quit);
    assert_eq!(app.focus, Focus::Content);
    assert_eq!(app.path.len(), 1);
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit);
}

#[tokio::test]
async fn comment_create_reply_edit_and_delete_stay_inside_modal() {
    let service = Arc::new(CommentsService::default());
    let (mut app, mut rx) = app(service.clone());
    open(&mut app, &mut rx).await;
    app.key(KeyCode::Char('a').into());
    app.paste("你好\nComment");
    app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
    app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    app.key(KeyCode::Tab.into());
    assert_eq!(app.editor.as_ref().unwrap().kind, Kind::Paragraph);
    app.key(ctrl_s());
    assert!(app.saving);
    receive(&mut app, &mut rx).await;
    assert!(app.editor.is_none());
    assert!(app.block_has_comments(&app.blocks[0].id));
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comments.as_ref().unwrap().items.len(), 1);
    assert_eq!(app.comments.as_ref().unwrap().selected, 0);
    assert!(
        matches!(&service.writes.lock().unwrap()[0], CommentWrite::Save { target: CommentTarget::Block(_), text } if text.plain().starts_with("你好\n"))
    );
    app.key(KeyCode::Enter.into());
    assert!(
        matches!(&app.editor.as_ref().unwrap().target, Target::Comment(CommentTarget::Discussion(id)) if id == "thread")
    );
    app.paste("reply");
    app.key(ctrl_s());
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comments.as_ref().unwrap().selected, 1);
    app.key(KeyCode::Char('e').into());
    app.paste(" edited");
    app.key(ctrl_s());
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert!(
        app.comments.as_ref().unwrap().items[1]
            .input
            .as_ref()
            .unwrap()
            .plain()
            .ends_with(" edited")
    );
    app.key(KeyCode::Char('d').into());
    app.key(KeyCode::Enter.into());
    assert!(!app.saving);
    app.key(KeyCode::Char('n').into());
    assert!(app.confirm.is_none());
    app.key(KeyCode::Char('d').into());
    app.key(KeyCode::Char('y').into());
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comments.as_ref().unwrap().items.len(), 1);
    app.key(KeyCode::Char('d').into());
    app.key(KeyCode::Char('y').into());
    receive(&mut app, &mut rx).await;
    assert!(!app.block_has_comments(&app.blocks[0].id));
    receive(&mut app, &mut rx).await;
    app.key(KeyCode::Esc.into());
    assert!(app.comments.is_none());
    assert_eq!(app.block_selected, 0);
    assert_eq!(app.blocks.len(), 1);
    assert_eq!(app.path.len(), 1);
}

#[tokio::test]
async fn save_failure_retains_rich_draft_and_cancel_returns_to_comments() {
    let service = Arc::new(CommentsService::default());
    service.fail_writes.store(true, Ordering::SeqCst);
    let (mut app, mut rx) = app(service);
    open(&mut app, &mut rx).await;
    app.key(KeyCode::Char('a').into());
    app.key(ctrl_s());
    assert!(!app.saving);
    assert!(
        app.editor
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("blank")
    );
    app.paste("draft");
    app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    let draft = app.editor.as_ref().unwrap().buffer.rich_text();
    app.key(ctrl_s());
    receive(&mut app, &mut rx).await;
    assert!(!app.saving);
    assert_eq!(app.editor.as_ref().unwrap().buffer.rich_text(), draft);
    assert!(app.editor.as_ref().unwrap().error.is_some());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('n').into());
    assert!(app.editor.is_some());
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.confirm == Some(Confirm::Quit));
    app.key(KeyCode::Char('n').into());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('y').into());
    assert!(app.editor.is_none());
    assert!(app.comments.is_some());
}

#[tokio::test]
async fn closed_modal_responses_are_ignored_and_save_success_survives_refresh_failure() {
    let service = Arc::new(CommentsService::default());
    let (mut app, mut rx) = app(service.clone());
    app.key(KeyCode::Char('c').into());
    let old_generation = app.comments.as_ref().unwrap().generation;
    app.key(KeyCode::Esc.into());
    receive(&mut app, &mut rx).await;
    assert!(app.comments.is_none());
    open(&mut app, &mut rx).await;
    app.receive(Message::CommentsLoaded {
        id: app.blocks[0].id.clone(),
        generation: old_generation,
        result: Ok(vec![comment("stale")]),
    });
    assert!(app.comments.as_ref().unwrap().items.is_empty());
    app.key(KeyCode::Char('a').into());
    app.paste("saved");
    app.key(ctrl_s());
    service.fail_reads.store(true, Ordering::SeqCst);
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert!(app.editor.is_none());
    assert!(
        app.comments
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("Comment saved. Could not refresh")
    );
    assert!(app.block_has_comments(&app.blocks[0].id));
    service.fail_reads.store(false, Ordering::SeqCst);
    app.key(KeyCode::Char('r').into());
    receive(&mut app, &mut rx).await;
    assert!(app.comments.as_ref().unwrap().error.is_none());
    assert!(!app.error);
}

#[tokio::test]
async fn indicator_checks_are_visible_only_deduplicated_and_throttled() {
    let service = Arc::new(CommentsService::default());
    let (mut app, mut rx) = app(service.clone());
    let mut second = app.blocks[0].clone();
    second.id = "second".into();
    app.blocks.push(second);
    let first = app.blocks[0].id.clone();
    app.check_visible_comments(&[first.clone(), "not-a-block".into()]);
    app.check_visible_comments(std::slice::from_ref(&first));
    assert_eq!(app.comment_indicators.active.len(), 1);
    app.check_visible_comments(&["second".into()]);
    assert_eq!(app.comment_indicators.active.len(), 1);
    app.comment_indicators.next_start = None;
    app.check_visible_comments(&[first.clone(), "second".into()]);
    assert_eq!(app.comment_indicators.active.len(), 2);
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert_eq!(
        *service.probes.lock().unwrap(),
        [first.clone(), "second".into()]
    );
    app.comment_indicators.next_start = None;
    app.check_visible_comments(&[first]);
    assert!(app.comment_indicators.active.is_empty());
}

#[tokio::test]
async fn failed_indicators_wait_for_refresh_and_stale_checks_cannot_overwrite_modal_results() {
    let service = Arc::new(CommentsService::default());
    service.fail_reads.store(true, Ordering::SeqCst);
    let (mut app, mut rx) = app(service.clone());
    let id = app.blocks[0].id.clone();
    app.check_visible_comments(std::slice::from_ref(&id));
    receive(&mut app, &mut rx).await;
    assert!(app.status.contains("Comment indicators unavailable"));
    app.comment_indicators.next_start = None;
    app.check_visible_comments(std::slice::from_ref(&id));
    assert!(app.comment_indicators.active.is_empty());
    app.invalidate_comment_indicators();
    service.fail_reads.store(false, Ordering::SeqCst);
    app.check_visible_comments(std::slice::from_ref(&id));
    let request = app.comment_indicators.serial;
    app.comment_indicators
        .cache
        .insert(id.clone(), Presence::Known(true));
    receive(&mut app, &mut rx).await;
    assert!(app.block_has_comments(&id));
    assert!(app.comment_indicators.active.is_empty());
    app.comment_indicators
        .cache
        .insert(id.clone(), Presence::Pending(request));
    app.receive_comment_presence(
        id.clone(),
        request,
        app.content_generation + 1,
        Ok(false),
        None,
    );
    assert!(!app.comment_indicators.cache.contains_key(&id));
}

#[tokio::test]
async fn retry_after_blocks_new_indicator_requests_without_rechecking_failed_blocks() {
    let (mut app, _) = app(Arc::new(CommentsService::default()));
    let id = app.blocks[0].id.clone();
    app.comment_indicators
        .cache
        .insert(id.clone(), Presence::Pending(1));
    let before = Instant::now();
    app.receive_comment_presence(
        id,
        1,
        app.content_generation,
        Err("rate limited".into()),
        Some(Duration::from_secs(7)),
    );
    assert!(app.comment_indicators.next_start.unwrap() >= before + Duration::from_secs(7));
    app.check_visible_comments(&["other".into()]);
    assert!(app.comment_indicators.active.is_empty());
}

#[tokio::test]
async fn protected_comments_remain_readable_and_child_pages_use_page_targets() {
    let service = Arc::new(CommentsService::default());
    let mut item = comment("protected");
    item.input = None;
    service.items.lock().unwrap().push(item);
    let (mut app, mut rx) = app(service);
    app.blocks[0].child_page = true;
    open(&mut app, &mut rx).await;
    app.key(KeyCode::Char('e').into());
    assert!(app.editor.is_none());
    assert!(app.status.contains("read-only"));
    app.key(KeyCode::Char('a').into());
    assert!(matches!(
        &app.editor.as_ref().unwrap().target,
        Target::Comment(CommentTarget::Page(_))
    ));
}
