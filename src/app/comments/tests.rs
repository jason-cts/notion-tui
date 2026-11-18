use super::*;
use crate::service::{NotionService, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
};

fn comment(id: &str) -> Comment {
    Comment::from_value(&serde_json::json!({"id":id,"discussion_id":"thread",
        "created_time":"2026-01-01T00:00:00Z","last_edited_time":"2026-01-01T00:00:00Z",
        "created_by":{"id":"author"},"rich_text":[{"type":"text","text":{"content":"Hello"}}]}))
    .unwrap()
}

#[derive(Default)]
struct CommentsService {
    pages: Mutex<Vec<PageItem>>,
    items: Mutex<Vec<Comment>>,
    writes: Mutex<Vec<CommentWrite>>,
    probes: Mutex<Vec<String>>,
    fail_reads: AtomicBool,
    fail_writes: AtomicBool,
    read_status: AtomicU16,
    comment_reads: AtomicUsize,
}

impl CommentsService {
    fn check_read(&self) -> anyhow::Result<()> {
        let status = self.read_status.load(Ordering::SeqCst);
        if status == 429 {
            return Err(rusticnotion::Error::RateLimited {
                retry_after_secs: 7,
            }
            .into());
        }
        if status != 0 {
            let code = match status {
                403 => "restricted_resource",
                404 => "object_not_found",
                _ => "internal_server_error",
            };
            return Err(rusticnotion::Error::ApiError {
                error: serde_json::from_value(serde_json::json!({
                    "object":"error", "status":status, "code":code, "message":"Unavailable"
                }))
                .unwrap(),
            }
            .into());
        }
        if self.fail_reads.load(Ordering::SeqCst) {
            anyhow::bail!("No read access");
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl NotionService for CommentsService {
    async fn search(&self, _: &str) -> anyhow::Result<Vec<PageItem>> {
        Ok(self.pages.lock().unwrap().clone())
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
        self.check_read()?;
        Ok(!self.items.lock().unwrap().is_empty())
    }
    async fn comments(&self, _: &str) -> anyhow::Result<Vec<Comment>> {
        self.comment_reads.fetch_add(1, Ordering::SeqCst);
        self.check_read()?;
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

fn authenticated_app(service: Arc<CommentsService>) -> (App, mpsc::UnboundedReceiver<Message>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (App::new(service, tx, false), rx)
}

fn shared_page() -> PageItem {
    PageItem {
        id: "shared-page".into(),
        title: "Shared page".into(),
    }
}

#[tokio::test]
async fn startup_probes_first_shared_page_once_and_gates_indicators() {
    for populated in [false, true] {
        let service = Arc::new(CommentsService::default());
        *service.pages.lock().unwrap() = vec![
            shared_page(),
            PageItem {
                id: "second-page".into(),
                title: "Second".into(),
            },
        ];
        if populated {
            service.items.lock().unwrap().push(comment("first"));
        }
        let (mut app, mut rx) = authenticated_app(service.clone());
        app.start_search();
        receive(&mut app, &mut rx).await;
        assert!(matches!(
            app.comment_indicators.access,
            CommentAccess::Checking(_)
        ));
        app.blocks.push(crate::model::tests::paragraph());
        for _ in 0..3 {
            app.check_visible_comments(&["block".into()]);
            assert!(app.comment_indicators.active.is_empty());
        }
        receive(&mut app, &mut rx).await;
        assert_eq!(app.comment_indicators.access, CommentAccess::Available);
        assert_eq!(*service.probes.lock().unwrap(), ["shared-page"]);
        app.start_search();
        receive(&mut app, &mut rx).await;
        assert_eq!(*service.probes.lock().unwrap(), ["shared-page"]);
        app.check_visible_comments(&["block".into()]);
        receive(&mut app, &mut rx).await;
        assert_eq!(*service.probes.lock().unwrap(), ["shared-page", "block"]);
        assert_eq!(app.block_has_comments("block"), populated);
    }
}

#[tokio::test]
async fn startup_defers_without_pages_and_demo_skips_the_probe() {
    let service = Arc::new(CommentsService::default());
    let (mut app, mut rx) = authenticated_app(service.clone());
    app.start_search();
    receive(&mut app, &mut rx).await;
    app.check_visible_comments(&[]);
    assert_eq!(app.comment_indicators.access, CommentAccess::Unchecked);
    assert!(service.probes.lock().unwrap().is_empty());
    service.pages.lock().unwrap().push(shared_page());
    app.start_search();
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comment_indicators.access, CommentAccess::Available);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut demo = App::new(service.clone(), tx, true);
    demo.start_search();
    receive(&mut demo, &mut rx).await;
    assert_eq!(demo.comment_indicators.access, CommentAccess::Available);
    assert_eq!(*service.probes.lock().unwrap(), ["shared-page"]);
}

#[tokio::test]
async fn denied_startup_keeps_browsing_and_refresh_quiet_until_c_succeeds() {
    let service = Arc::new(CommentsService::default());
    service.pages.lock().unwrap().push(shared_page());
    service.read_status.store(403, Ordering::SeqCst);
    let (mut app, mut rx) = authenticated_app(service.clone());
    app.start_search();
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comment_indicators.access, CommentAccess::Denied);
    assert_eq!(app.status, CommentReadError::Denied.to_string());
    assert!(!app.status.contains("Insert comments"));
    assert!(!app.quit);
    app.key(KeyCode::Char('r').into());
    receive(&mut app, &mut rx).await;
    app.open_page(shared_page());
    receive(&mut app, &mut rx).await;
    app.key(KeyCode::Char('r').into());
    receive(&mut app, &mut rx).await;
    for _ in 0..3 {
        app.check_visible_comments(&["block".into()]);
    }
    assert_eq!(app.comment_indicators.access, CommentAccess::Denied);
    assert_eq!(*service.probes.lock().unwrap(), ["shared-page"]);
    assert!(!app.status.contains("Comments unavailable"));
    open(&mut app, &mut rx).await;
    assert_eq!(
        app.comments.as_ref().unwrap().error.as_deref(),
        Some(CommentReadError::Denied.to_string().as_str())
    );
    app.key(KeyCode::Esc.into());
    service.read_status.store(0, Ordering::SeqCst);
    open(&mut app, &mut rx).await;
    assert_eq!(app.comment_indicators.access, CommentAccess::Available);
    assert!(app.comments.as_ref().unwrap().error.is_none());
    app.key(KeyCode::Esc.into());
    app.check_visible_comments(&["block".into()]);
    // The modal already cached this block; another visible block is checked normally.
    let mut second = app.blocks[0].clone();
    second.id = "another-block".into();
    app.blocks.push(second);
    app.check_visible_comments(&["another-block".into()]);
    receive(&mut app, &mut rx).await;
    assert_eq!(
        *service.probes.lock().unwrap(),
        ["shared-page", "another-block"]
    );
}

#[tokio::test]
async fn temporary_startup_failures_pause_scanning_and_refresh_can_recover() {
    for status in [0, 404, 500] {
        let service = Arc::new(CommentsService::default());
        service.pages.lock().unwrap().push(shared_page());
        service.read_status.store(status, Ordering::SeqCst);
        service.fail_reads.store(status == 0, Ordering::SeqCst);
        let (mut app, mut rx) = authenticated_app(service.clone());
        app.start_search();
        receive(&mut app, &mut rx).await;
        receive(&mut app, &mut rx).await;
        assert_eq!(
            app.comment_indicators.access,
            CommentAccess::TemporarilyUnavailable
        );
        assert!(!app.status.contains("enable Read comments"));
        app.check_visible_comments(&["block".into()]);
        assert_eq!(service.probes.lock().unwrap().len(), 1);
        service.read_status.store(0, Ordering::SeqCst);
        service.fail_reads.store(false, Ordering::SeqCst);
        app.key(KeyCode::Char('r').into());
        receive(&mut app, &mut rx).await;
        receive(&mut app, &mut rx).await;
        assert_eq!(app.comment_indicators.access, CommentAccess::Available);
        assert_eq!(
            *service.probes.lock().unwrap(),
            ["shared-page", "shared-page"]
        );
    }
}

#[tokio::test]
async fn rate_limited_startup_blocks_explicit_retries_until_the_deadline() {
    let service = Arc::new(CommentsService::default());
    service.pages.lock().unwrap().push(shared_page());
    service.read_status.store(429, Ordering::SeqCst);
    let (mut app, mut rx) = authenticated_app(service.clone());
    app.start_search();
    receive(&mut app, &mut rx).await;
    receive(&mut app, &mut rx).await;
    assert_eq!(
        app.comment_indicators.access,
        CommentAccess::TemporarilyUnavailable
    );
    app.key(KeyCode::Char('r').into());
    receive(&mut app, &mut rx).await;
    app.open_page(shared_page());
    receive(&mut app, &mut rx).await;
    app.key(KeyCode::Char('c').into());
    assert!(!app.comments.as_ref().unwrap().loading);
    assert!(
        app.comments
            .as_ref()
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("rate limited")
    );
    assert_eq!(service.comment_reads.load(Ordering::SeqCst), 0);
    assert_eq!(service.probes.lock().unwrap().len(), 1);
    app.key(KeyCode::Esc.into());
    service.read_status.store(0, Ordering::SeqCst);
    app.comment_indicators.retry_at = Some(Instant::now() - Duration::from_secs(1));
    app.check_visible_comments(&["block".into()]);
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comment_indicators.access, CommentAccess::Available);
    assert_eq!(service.probes.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn explicit_read_supersedes_pending_startup_and_indicator_results() {
    let service = Arc::new(CommentsService::default());
    service.pages.lock().unwrap().push(shared_page());
    let (mut app, mut rx) = authenticated_app(service.clone());
    app.start_search();
    receive(&mut app, &mut rx).await;
    let request = match app.comment_indicators.access {
        CommentAccess::Checking(request) => request,
        _ => panic!("expected startup probe"),
    };
    let _late_startup = rx.recv().await.unwrap();
    app.open_page(shared_page());
    receive(&mut app, &mut rx).await;
    open(&mut app, &mut rx).await;
    app.receive(Message::CommentAccess {
        request,
        result: Err(CommentReadError::Denied),
    });
    assert_eq!(app.comment_indicators.access, CommentAccess::Available);
    assert!(!app.error);
    app.key(KeyCode::Esc.into());
    app.comment_indicators.cache.remove("block");
    app.check_visible_comments(&["block".into()]);
    let late_presence = rx.recv().await.unwrap();
    open(&mut app, &mut rx).await;
    let Message::CommentPresence {
        id,
        request,
        generation,
        ..
    } = late_presence
    else {
        panic!("expected indicator request")
    };
    app.receive(Message::CommentPresence {
        id,
        request,
        generation,
        result: Err(CommentReadError::RateLimited(Duration::from_secs(60))),
    });
    assert_eq!(app.comment_indicators.access, CommentAccess::Available);
    assert!(app.comment_indicators.retry_at.is_none());
    assert!(!app.error);
}

#[tokio::test]
async fn denied_indicator_cancels_other_pending_checks_and_warns_once() {
    let service = Arc::new(CommentsService::default());
    service.read_status.store(403, Ordering::SeqCst);
    let (mut app, mut rx) = app(service.clone());
    let mut second = app.blocks[0].clone();
    second.id = "second".into();
    app.blocks.push(second);
    app.check_visible_comments(&["block".into()]);
    app.comment_indicators.next_start = None;
    app.check_visible_comments(&["second".into()]);
    receive(&mut app, &mut rx).await;
    assert_eq!(app.comment_indicators.access, CommentAccess::Denied);
    app.status = "Browsing".into();
    receive(&mut app, &mut rx).await;
    assert_eq!(app.status, "Browsing");
    assert!(app.comment_indicators.active.is_empty());
    app.check_visible_comments(&["block".into(), "second".into()]);
    assert_eq!(service.probes.lock().unwrap().len(), 2);
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
    let old_access_request = app.comment_indicators.serial;
    app.key(KeyCode::Esc.into());
    receive(&mut app, &mut rx).await;
    assert!(app.comments.is_none());
    open(&mut app, &mut rx).await;
    app.receive(Message::CommentsLoaded {
        id: app.blocks[0].id.clone(),
        generation: old_generation,
        access_request: old_access_request,
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
    app.retry_temporary_comment_access();
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
    app.receive_comment_presence(id.clone(), request, app.content_generation + 1, Ok(false));
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
        Err(CommentReadError::RateLimited(Duration::from_secs(7))),
    );
    assert!(app.comment_indicators.retry_at.unwrap() >= before + Duration::from_secs(7));
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
