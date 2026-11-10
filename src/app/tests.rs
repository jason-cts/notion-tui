use super::*;
use crate::{model::tests::paragraph, service::DemoService};
use std::sync::Arc;

fn app() -> (App, mpsc::UnboundedReceiver<Message>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut app = App::new(Arc::new(DemoService), tx, true);
    app.path.push(Location {
        id: "page".into(),
        title: "Page".into(),
        is_page: true,
        page_id: "page".into(),
        list_depth: 0,
    });
    app.focus = Focus::Content;
    app.blocks.push(paragraph());
    (app, rx)
}

fn ctrl_s() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

#[tokio::test]
async fn stale_search_and_content_responses_cannot_replace_current_state() {
    let (mut app, _) = app();
    app.search_generation = 2;
    app.content_generation = 3;
    app.receive(Message::Search {
        generation: 1,
        result: Ok(vec![PageItem {
            id: "old".into(),
            title: "Old".into(),
        }]),
    });
    assert!(app.pages.is_empty());
    app.receive(Message::Content {
        generation: 2,
        result: Ok((
            Location {
                id: "old".into(),
                title: "Old".into(),
                is_page: true,
                page_id: "old".into(),
                list_depth: 0,
            },
            vec![],
        )),
    });
    assert_eq!(app.blocks.len(), 1);
    assert_eq!(app.location().unwrap().id, "page");
    app.receive(Message::Content {
        generation: 3,
        result: Err("Failure".into()),
    });
    assert!(app.error);
}

#[tokio::test]
async fn failed_save_keeps_draft_and_does_not_retry() {
    let (mut app, mut rx) = app();
    app.key(KeyCode::Char('e').into());
    app.paste(" new 🦀\nline");
    let draft = app.editor.as_ref().unwrap().buffer.text.clone();
    app.key(ctrl_s());
    assert!(app.saving);
    // q cannot silently terminate an in-flight write.
    app.key(KeyCode::Char('q').into());
    assert!(!app.quit);
    app.receive(
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap(),
    );
    assert!(!app.saving);
    assert_eq!(app.editor.as_ref().unwrap().buffer.text, draft);
    assert!(app.editor.as_ref().unwrap().error.is_some());
    assert!(rx.try_recv().is_err());
    app.key(KeyCode::Esc.into());
    assert!(app.confirm == Some(Confirm::Discard));
    app.key(KeyCode::Char('n').into());
    assert!(app.editor.is_some());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('y').into());
    assert!(app.editor.is_none());
}

#[tokio::test]
async fn formatting_confirmation_and_protected_runs() {
    let (mut app, _) = app();
    app.blocks[0].styled = true;
    app.key(KeyCode::Char('e').into());
    app.paste(" revised");
    app.key(ctrl_s());
    assert!(app.confirm == Some(Confirm::Formatting));
    assert!(!app.saving);
    app.key(KeyCode::Char('n').into());
    assert!(app.editor.is_some());
    app.editor = None;
    app.blocks[0].protected = true;
    app.key(KeyCode::Char('e').into());
    assert!(app.editor.is_none());
    assert!(app.error);
}

#[tokio::test]
async fn nested_navigation_search_and_creation_parent() {
    let (mut app, mut rx) = app();
    app.blocks[0].children = true;
    app.key(KeyCode::Enter.into());
    assert_eq!(app.path.len(), 2);
    assert_eq!(app.location().unwrap().id, "block");
    app.receive(rx.recv().await.unwrap());
    app.key(KeyCode::Char('n').into());
    assert!(
        matches!(&app.editor.as_ref().unwrap().target, Target::Create(parent) if parent == "page")
    );
    app.editor = None;
    app.key(KeyCode::Backspace.into());
    assert_eq!(app.path.len(), 1);
    app.key(KeyCode::Tab.into());
    assert_eq!(app.focus, Focus::Pages);
    app.key(KeyCode::Char('/').into());
    app.paste("Welcome");
    app.key(KeyCode::Enter.into());
    assert_eq!(app.query, "Welcome");
}

#[test]
fn editor_kind_selection_and_title_validation() {
    let mut editor = Editor::new(Target::Append("page".into()));
    editor.cycle_kind();
    assert_eq!(editor.kind, Kind::Heading1);
    let mut editor = Editor::new(Target::Create("page".into()));
    assert!(editor.write().is_err());
    editor.buffer.insert(&"a".repeat(2001));
    assert!(editor.write().is_err());
}

#[tokio::test]
async fn list_depth_survives_refresh_and_back_navigation_but_resets_at_pages() {
    let (mut app, mut rx) = app();
    for depth in 1..=2 {
        let mut item = paragraph();
        item.id = format!("list-{depth}");
        item.block_type = "numbered_list_item".into();
        item.kind = Some(Kind::Number);
        item.children = true;
        app.blocks = vec![item];
        app.key(KeyCode::Enter.into());
        app.receive(rx.recv().await.unwrap());
        assert_eq!(app.location().unwrap().list_depth, depth);
    }
    app.key(KeyCode::Char('r').into());
    app.receive(rx.recv().await.unwrap());
    assert_eq!(app.location().unwrap().list_depth, 2);

    let mut page = paragraph();
    page.child_page = true;
    app.blocks = vec![page];
    app.key(KeyCode::Enter.into());
    app.receive(rx.recv().await.unwrap());
    assert_eq!(app.location().unwrap().list_depth, 0);
    for depth in [2, 1, 0] {
        app.key(KeyCode::Backspace.into());
        app.receive(rx.recv().await.unwrap());
        assert_eq!(app.location().unwrap().list_depth, depth);
    }
}
