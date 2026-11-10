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

#[test]
fn both_backspace_encodings_edit_drafts_without_navigating() {
    for action in ['e', 'a', 'n'] {
        for backspace in [
            KeyCode::Backspace.into(),
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        ] {
            let (mut app, _) = app();
            app.key(KeyCode::Char(action).into());
            let editor = app.editor.as_mut().unwrap();
            editor.buffer = Buffer::new("你🦀x".into());
            app.key(KeyCode::Left.into());
            app.key(backspace);
            let buffer = &app.editor.as_ref().unwrap().buffer;
            assert_eq!(buffer.text, "你x", "{action}: {backspace:?}");
            assert_eq!(buffer.cursor, "你".len());
            app.key(KeyCode::Home.into());
            app.key(backspace);
            let buffer = &app.editor.as_ref().unwrap().buffer;
            assert_eq!(buffer.text, "你x");
            assert_eq!(buffer.cursor, 0);
            assert_eq!(app.path.len(), 1);
            assert_eq!(app.focus, Focus::Content);
            assert!(app.confirm.is_none());
        }
    }
}

#[test]
fn append_captures_the_selected_sibling_and_its_parent() {
    let (mut app, _) = app();
    app.blocks = ["first", "middle", "last"]
        .into_iter()
        .map(|id| {
            let mut block = paragraph();
            block.id = id.into();
            block
        })
        .collect();
    app.block_selected = 1;
    app.key(KeyCode::Char('a').into());
    app.block_selected = 2;
    app.editor.as_mut().unwrap().buffer.insert("new sibling");
    assert!(matches!(app.editor.as_ref().unwrap().write().unwrap(),
        Write::Append { parent, after: Some(after), text, .. }
        if parent == "page" && after == "middle" && text == "new sibling"));

    app.editor = None;
    app.path.push(Location {
        id: "nested".into(),
        title: "Nested".into(),
        is_page: false,
        page_id: "page".into(),
        list_depth: 1,
    });
    app.blocks[0].id = "nested-child".into();
    app.block_selected = 0;
    app.key(KeyCode::Char('a').into());
    assert!(matches!(app.editor.as_ref().unwrap().write().unwrap(),
        Write::Append { parent, after: Some(after), .. }
        if parent == "nested" && after == "nested-child"));

    app.editor = None;
    app.blocks.clear();
    app.key(KeyCode::Char('a').into());
    assert!(matches!(app.editor.as_ref().unwrap().write().unwrap(),
        Write::Append { parent, after: None, .. } if parent == "nested"));

    app.editor = None;
    app.loading_content = true;
    app.key(KeyCode::Char('a').into());
    assert!(app.editor.is_none());
}

#[test]
fn browser_links_open_explicitly_with_selection_for_multiple_targets() {
    let (mut app, _) = app();
    app.blocks[0] = BlockItem::from_value(
        &serde_json::json!({"type":"paragraph", "paragraph":{"rich_text":[
            {"type":"text","plain_text":"Google","href":"https://www.google.com/"},
            {"type":"text","plain_text":"duplicate","href":"https://www.google.com/"},
            {"type":"text","plain_text":"Other","href":"https://example.com/"},
            {"type":"text","plain_text":"Unsafe","href":"javascript:alert(1)"}
        ]}}),
    );
    assert!(app.pending_link.is_none());
    app.key(KeyCode::Char('o').into());
    assert_eq!(app.link_picker.as_ref().unwrap().links.len(), 2);
    assert!(app.pending_link.is_none());
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Enter.into());
    assert_eq!(
        app.pending_link.take().as_deref(),
        Some("https://example.com/")
    );
    assert!(app.link_picker.is_none());
    app.key(KeyCode::Char('o').into());
    app.key(KeyCode::Esc.into());
    assert!(app.pending_link.is_none());
    app.key(KeyCode::Char('v').into());
    // A refreshed block must not change links in an already-open reader snapshot.
    app.blocks[0] = paragraph();
    app.key(KeyCode::Char('o').into());
    app.key(KeyCode::Enter.into());
    assert_eq!(
        app.pending_link.take().as_deref(),
        Some("https://www.google.com/")
    );
    app.viewer = None;
    app.key(KeyCode::Char('o').into());
    assert!(app.pending_link.is_none());
    assert!(app.error);
    app.receive(Message::Browser(Err("No browser available".into())));
    assert_eq!(app.status, "No browser available");
    app.receive(Message::Browser(Ok(())));
    assert!(!app.error);
}

#[test]
fn single_browser_link_works_from_the_reader_without_closing_it() {
    let (mut app, _) = app();
    app.blocks[0] = BlockItem::from_value(
        &serde_json::json!({"type":"paragraph", "paragraph":{"rich_text":[
            {"type":"text","plain_text":"Google","href":"https://www.google.com/"}
        ]}}),
    );
    app.key(KeyCode::Char('v').into());
    app.key(KeyCode::Char('o').into());
    assert!(app.viewer.is_some());
    assert_eq!(app.pending_link.as_deref(), Some("https://www.google.com/"));
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
    let mut editor = Editor::new(Target::Append {
        parent: "page".into(),
        after: None,
    });
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
