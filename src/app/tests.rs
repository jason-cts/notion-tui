use super::*;
use crate::{model::tests::paragraph, service::DemoService};
use std::sync::Arc;

fn app() -> (App, mpsc::UnboundedReceiver<Message>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut app = App::new(Arc::new(DemoService), tx, true);
    app.path.push(Location {
        id: "page".into(),
        title: "Page".into(),
        kind: LocationKind::Page,
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
async fn formatting_only_save_retains_selection_styles_and_draft_on_failure() {
    let (mut app, mut rx) = app();
    app.key(KeyCode::Char('e').into());
    app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
    app.key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT));
    app.key(KeyCode::Char(' ').into());
    app.paste("ignored");
    app.key(ctrl_s());
    assert!(!app.saving);
    assert!(rx.try_recv().is_err());
    app.key(KeyCode::Enter.into());
    let original = app.editor.as_ref().unwrap().buffer.clone();
    assert_eq!(original.text, "Hello");
    assert_eq!(original.style_states()[0], Some(true));
    assert!(app.editor.as_ref().unwrap().changed());
    app.key(ctrl_s());
    assert!(app.saving);
    app.receive(rx.recv().await.unwrap());
    let retained = &app.editor.as_ref().unwrap().buffer;
    assert_eq!(retained.text, original.text);
    assert_eq!(retained.cursor, original.cursor);
    assert_eq!(retained.selection(), original.selection());
    assert_eq!(retained.rich_text(), original.rich_text());
    assert!(app.editor.as_ref().unwrap().error.is_some());
    assert!(app.confirm.is_none());
}

#[test]
fn table_cells_keep_independent_styles_and_selection() {
    let mut editor = Editor::new(Target::Edit(crate::model::tests::table_row()));
    editor.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
    editor.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    let first = editor.active_buffer().clone();
    editor.key(KeyCode::Tab.into());
    editor.key(KeyCode::Home.into());
    editor.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    let second = editor.active_buffer().style_states();
    editor.key(KeyCode::BackTab.into());
    assert_eq!(editor.active_buffer().selection(), first.selection());
    assert_eq!(editor.active_buffer().style_states(), first.style_states());
    editor.key(KeyCode::Tab.into());
    assert_eq!(editor.active_buffer().style_states(), second);
    assert!(editor.changed());
}

async fn drain_content(app: &mut App, rx: &mut mpsc::UnboundedReceiver<Message>) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while app.loading_content {
            app.receive(rx.recv().await.unwrap());
        }
    })
    .await
    .unwrap();
}

struct DeleteService {
    blocks: std::sync::Mutex<Vec<BlockItem>>,
    deleted: std::sync::Mutex<Vec<BlockItem>>,
    fail: bool,
}

#[async_trait::async_trait]
impl crate::service::NotionService for DeleteService {
    async fn search(&self, _: &str) -> anyhow::Result<Vec<PageItem>> {
        Ok(vec![])
    }
    async fn page(&self, id: &str) -> anyhow::Result<PageItem> {
        Ok(PageItem {
            id: id.into(),
            title: "Parent".into(),
        })
    }
    async fn children(&self, _: &str) -> anyhow::Result<Vec<BlockItem>> {
        Ok(self.blocks.lock().unwrap().clone())
    }
    async fn write(&self, write: Write) -> anyhow::Result<Option<PageItem>> {
        let Write::Delete { original } = write else {
            panic!("expected deletion");
        };
        self.deleted.lock().unwrap().push(original.clone());
        if self.fail {
            anyhow::bail!("Delete response unavailable; inspect Notion before retrying");
        }
        self.blocks
            .lock()
            .unwrap()
            .retain(|block| block.id != original.id);
        Ok(None)
    }
}

fn deletion_app(
    count: usize,
    fail: bool,
) -> (App, mpsc::UnboundedReceiver<Message>, Arc<DeleteService>) {
    let blocks: Vec<_> = (0..count)
        .map(|index| {
            let mut block = paragraph();
            block.id = format!("block-{index}");
            block
        })
        .collect();
    let service = Arc::new(DeleteService {
        blocks: std::sync::Mutex::new(blocks.clone()),
        deleted: std::sync::Mutex::new(vec![]),
        fail,
    });
    let (mut app, _) = app();
    let (tx, rx) = mpsc::unbounded_channel();
    app.sender = tx;
    app.service = service.clone();
    app.demo = false;
    app.blocks = blocks;
    app.pages = vec![PageItem {
        id: "old-search-result".into(),
        title: "Old result".into(),
    }];
    (app, rx, service)
}

async fn finish_deletion(app: &mut App, rx: &mut mpsc::UnboundedReceiver<Message>) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while app.saving || app.loading_content || app.loading_search {
            app.receive(rx.recv().await.unwrap());
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn deletion_confirmation_captures_the_target_and_isolates_input() {
    let (mut app, mut rx, service) = deletion_app(3, false);
    app.block_selected = 1;
    app.key(KeyCode::Char('d').into());
    assert!(app.confirm == Some(Confirm::Delete));
    assert_eq!(app.pending_delete.as_ref().unwrap().block.id, "block-1");
    assert_eq!(app.pending_delete.as_ref().unwrap().parent, "page");
    for key in [
        KeyCode::Enter.into(),
        KeyCode::Down.into(),
        KeyCode::Tab.into(),
        KeyCode::Char('q').into(),
        KeyCode::Char('e').into(),
        KeyCode::Char('/').into(),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        app.key(key);
        assert!(app.confirm == Some(Confirm::Delete));
        assert_eq!(app.block_selected, 1);
        assert!(!app.saving && !app.quit);
    }
    app.paste("y");
    assert!(service.deleted.lock().unwrap().is_empty());
    // A refreshed display must not alter the captured confirmation target.
    app.blocks[1].text = "changed display".into();
    app.key(KeyCode::Char('Y').into());
    assert!(app.confirm.is_none());
    assert!(app.saving);
    for key in [KeyCode::Char('q'), KeyCode::Down, KeyCode::Char('d')] {
        app.key(key.into());
    }
    assert!(!app.quit);
    assert_eq!(app.block_selected, 1);
    finish_deletion(&mut app, &mut rx).await;
    let deleted = service.deleted.lock().unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].id, "block-1");
    assert_ne!(deleted[0].text, "changed display");
    assert_eq!(app.blocks[app.block_selected].id, "block-2");
    assert!(app.pending_delete.is_none());
}

#[tokio::test]
async fn deletion_cancellation_and_ineligible_contexts_never_write() {
    let (mut app, mut rx, service) = deletion_app(1, false);
    for cancel in [KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc] {
        app.key(KeyCode::Char('d').into());
        app.key(cancel.into());
        assert!(app.confirm.is_none() && app.pending_delete.is_none());
        assert_eq!(app.blocks.len(), 1);
    }
    app.focus = Focus::Pages;
    app.key(KeyCode::Char('d').into());
    app.focus = Focus::Content;
    app.loading_content = true;
    app.key(KeyCode::Char('d').into());
    app.loading_content = false;
    app.block_selected = 100;
    app.key(KeyCode::Char('d').into());
    app.block_selected = 0;
    app.blocks[0].id.clear();
    app.key(KeyCode::Char('d').into());
    app.blocks.clear();
    app.key(KeyCode::Char('d').into());
    app.blocks.push(paragraph());
    app.path.last_mut().unwrap().kind = LocationKind::Database;
    app.key(KeyCode::Char('d').into());
    app.path.last_mut().unwrap().kind = LocationKind::Page;
    app.editor = Some(Editor::new(Target::Edit(paragraph())));
    app.key(KeyCode::Char('d').into());
    assert!(app.editor.as_ref().unwrap().buffer.text.contains('d'));
    app.editor = None;
    app.search = Some(Buffer::new(String::new()));
    app.key(KeyCode::Char('d').into());
    assert_eq!(app.search.as_ref().unwrap().text, "d");
    app.search = None;
    app.viewer = Some((paragraph().label(1, 0), 0));
    app.key(KeyCode::Char('d').into());
    assert!(app.viewer.is_none());
    app.help = true;
    app.key(KeyCode::Char('d').into());
    assert!(!app.help);
    assert!(app.confirm.is_none() && app.pending_delete.is_none() && !app.saving);
    assert!(service.deleted.lock().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn successful_deletion_restores_selection_for_first_middle_last_and_only_blocks() {
    for (count, index, expected) in [(3, 0, 0), (3, 1, 1), (3, 2, 1), (1, 0, 0)] {
        let (mut app, mut rx, service) = deletion_app(count, false);
        app.block_selected = index;
        app.key(KeyCode::Char('d').into());
        app.key(KeyCode::Char('y').into());
        finish_deletion(&mut app, &mut rx).await;
        assert_eq!(app.blocks.len(), count - 1);
        assert_eq!(app.block_selected, expected);
        assert_eq!(service.deleted.lock().unwrap().len(), 1);
        assert!(app.pages.is_empty(), "page search was refreshed");
        assert!(!app.error);
    }
}

#[tokio::test]
async fn failed_deletion_retains_selection_and_requires_a_new_confirmation() {
    let (mut app, mut rx, service) = deletion_app(3, true);
    app.block_selected = 1;
    app.key(KeyCode::Char('d').into());
    app.key(KeyCode::Char('y').into());
    finish_deletion(&mut app, &mut rx).await;
    assert!(app.error && app.status.contains("inspect Notion"));
    assert_eq!(app.blocks.len(), 3);
    assert_eq!(app.block_selected, 1);
    assert!(app.pending_delete.is_none() && app.confirm.is_none());
    assert!(rx.try_recv().is_err());
    app.key(KeyCode::Char('y').into());
    assert!(!app.saving);
    assert_eq!(service.deleted.lock().unwrap().len(), 1);
    app.key(KeyCode::Char('d').into());
    assert!(!app.saving);
    app.key(KeyCode::Char('y').into());
    finish_deletion(&mut app, &mut rx).await;
    assert_eq!(service.deleted.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn deletion_accepts_protected_and_unsupported_blocks_and_invalidates_database_metadata() {
    for block_type in [
        "child_page",
        "child_database",
        "table_row",
        "unsupported",
        "paragraph",
    ] {
        let (mut app, mut rx, _) = deletion_app(1, false);
        app.blocks[0].block_type = block_type.into();
        app.blocks[0].protected = true;
        let id = app.blocks[0].id.clone();
        app.databases.insert(id.clone(), DatabaseState::default());
        app.metadata_pending.insert(id.clone());
        app.metadata_generations.insert(id.clone(), 7);
        app.key(KeyCode::Char('d').into());
        assert!(app.confirm == Some(Confirm::Delete));
        app.key(KeyCode::Char('y').into());
        finish_deletion(&mut app, &mut rx).await;
        app.receive(Message::DatabaseMeta {
            id: id.clone(),
            generation: 7,
            result: Err("stale".into()),
        });
        assert!(!app.databases.contains_key(&id));
        assert!(!app.metadata_pending.contains(&id));
        assert_eq!(app.metadata_generations.get(&id), Some(&8));
    }
}

#[tokio::test]
async fn child_database_opens_switches_views_and_restores_row_after_page_navigation() {
    let (mut app, mut rx) = app();
    app.blocks = vec![BlockItem::from_value(
        &serde_json::json!({"id":"demo-database","type":"child_database","child_database":{"title":"Tasks"},"has_children":false}),
    )];
    app.key(KeyCode::Enter.into());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.location().unwrap().kind, LocationKind::Database);
    assert_eq!(app.database().unwrap().view().unwrap().view_type, "list");
    app.key(KeyCode::Char(']').into());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.database().unwrap().view().unwrap().view_type, "board");
    app.key(KeyCode::Right.into());
    assert_eq!(app.database().unwrap().selected, 1);
    app.key(KeyCode::Enter.into());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.location().unwrap().id, "demo-row-1");
    app.key(KeyCode::Backspace.into());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.database().unwrap().view().unwrap().view_type, "board");
    assert_eq!(app.database().unwrap().selected, 1);
    for key in ['e', 'd', 'a', 'n', ' '] {
        app.key(KeyCode::Char(key).into());
        assert!(app.editor.is_none());
        assert!(!app.saving);
    }
    app.key(KeyCode::Char('v').into());
    assert!(app.viewer.as_ref().unwrap().0.text.contains("Beta"));
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('o').into());
    assert_eq!(app.pending_link.as_deref(), Some("https://example.com/"));
}

#[tokio::test]
async fn only_backspace_returns_to_parent_in_database_views() {
    for (view_index, layout) in ["list", "board", "table"].into_iter().enumerate() {
        for back in [
            KeyCode::Backspace.into(),
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        ] {
            let (mut app, mut rx) = app();
            app.blocks = vec![BlockItem::from_value(&serde_json::json!({
                "id":"demo-database", "type":"child_database",
                "child_database":{"title":"Tasks"}, "has_children":false
            }))];
            app.key(KeyCode::Enter.into());
            drain_content(&mut app, &mut rx).await;
            for _ in 0..view_index {
                app.key(KeyCode::Char(']').into());
                drain_content(&mut app, &mut rx).await;
            }
            assert_eq!(app.database().unwrap().view().unwrap().view_type, layout);
            let selected = app.database().unwrap().selected;
            for key in ['h', 'j', 'k', 'l'] {
                app.key(KeyCode::Char(key).into());
            }
            let db = app.database().unwrap();
            assert_eq!(
                (db.selected, db.column, db.card, db.table_cell, db.table_row),
                (selected, 0, 0, 0, 0)
            );
            if layout == "board" {
                app.key(KeyCode::Right.into());
                assert_eq!(app.database().unwrap().column, 1);
                app.key(KeyCode::Left.into());
                assert_eq!(app.database().unwrap().column, 0);
            }
            for action in ['a', 'A'] {
                app.key(KeyCode::Char(action).into());
                assert!(app.editor.is_none());
                assert!(app.error);
            }
            if layout == "table" {
                app.key(KeyCode::Right.into());
                assert_eq!(app.database().unwrap().table_cell, 1);
                app.key(KeyCode::Left.into());
                assert_eq!(app.database().unwrap().table_cell, 0);
            }
            app.key(KeyCode::Left.into());
            assert_eq!(app.path.len(), 2);
            app.key(back);
            assert_eq!(app.path.len(), 1, "{layout}: {back:?}");
            assert_eq!(app.location().unwrap().id, "page");
            assert_eq!(app.location().unwrap().kind, LocationKind::Page);
            assert_eq!(app.focus, Focus::Content);
            drain_content(&mut app, &mut rx).await;
            assert!(app.database().is_none());
            assert!(!app.blocks.is_empty());
        }
    }
}

struct CellService {
    data: std::sync::Mutex<DatabaseData>,
    fail: std::sync::atomic::AtomicBool,
    writes: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::service::NotionService for CellService {
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
        Ok(vec![])
    }
    async fn database_rows(
        &self,
        _: &rusticnotion::models::view::View,
        _: crate::service::Progress,
    ) -> anyhow::Result<DatabaseData> {
        Ok(self.data.lock().unwrap().clone())
    }
    async fn write(&self, write: Write) -> anyhow::Result<Option<PageItem>> {
        use std::sync::atomic::Ordering;
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("Save failed; draft retained");
        }
        let Write::UpdateDatabaseCell { original, value } = write else {
            panic!("expected database cell");
        };
        let mut data = self.data.lock().unwrap();
        let row = data
            .rows
            .iter_mut()
            .find(|r| r.id == original.row.id)
            .unwrap();
        row.page["properties"][&original.column.name][&original.kind] = value;
        if original.kind == "title" {
            row.title =
                crate::database::plain(&row.page["properties"]["Name"], &Default::default());
            data.rows.sort_by_key(|r| r.title.clone());
        }
        data.rows
            .retain(|r| r.page["properties"]["Status"]["status"]["id"] != "done");
        Ok(None)
    }
}

fn cell_app() -> (App, mpsc::UnboundedReceiver<Message>, Arc<CellService>) {
    let meta = crate::service::database::demo_metadata("db");
    let data = crate::service::database::demo_data(meta.views[2].clone());
    let service = Arc::new(CellService {
        data: std::sync::Mutex::new(data.clone()),
        fail: false.into(),
        writes: 0.into(),
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let mut app = App::new(service.clone(), tx, false);
    app.path.push(Location {
        id: "db".into(),
        title: "DB".into(),
        kind: LocationKind::Database,
        page_id: "parent".into(),
        list_depth: 0,
    });
    app.focus = Focus::Content;
    app.databases.insert(
        "db".into(),
        DatabaseState {
            meta: Some(meta),
            view_index: 2,
            data: Some(data),
            ..Default::default()
        },
    );
    (app, rx, service)
}

#[tokio::test]
async fn table_editing_keeps_failed_drafts_and_restores_cells_after_sorted_reload() {
    use std::sync::atomic::Ordering;
    let (mut app, mut rx, service) = cell_app();
    app.key(KeyCode::Char('e').into());
    assert!(app.editor.is_none());
    app.key(KeyCode::Right.into());
    app.key(KeyCode::Enter.into());
    assert_eq!(app.path.len(), 1);
    app.key(KeyCode::Char('e').into());
    app.key(ctrl_s());
    assert!(app.editor.is_none());
    assert_eq!(service.writes.load(Ordering::SeqCst), 0);
    app.key(KeyCode::Char('e').into());
    let buffer = &mut app.editor.as_mut().unwrap().cell.as_mut().unwrap().fields[0];
    *buffer = Buffer::new("ZZZ".into());
    service.fail.store(true, Ordering::SeqCst);
    app.key(ctrl_s());
    assert!(app.confirm.is_none());
    app.receive(rx.recv().await.unwrap());
    assert!(!app.saving);
    assert_eq!(app.editor.as_ref().unwrap().active_buffer().text, "ZZZ");
    assert_eq!(app.editor.as_ref().unwrap().active_buffer().cursor, 3);
    service.fail.store(false, Ordering::SeqCst);
    app.key(ctrl_s());
    app.receive(rx.recv().await.unwrap());
    drain_content(&mut app, &mut rx).await;
    assert!(app.editor.is_none());
    assert_eq!(
        app.database().unwrap().selected_row().unwrap().id,
        "demo-row-0"
    );
    assert_eq!(app.database().unwrap().table_row, 1);
    assert_eq!(app.database().unwrap().table_cell, 1);
    // Enter opens the page only after navigating back to its icon.
    app.key(KeyCode::Left.into());
    app.key(KeyCode::Enter.into());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.location().unwrap().id, "demo-row-0");
    app.key(KeyCode::Backspace.into());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.database().unwrap().table_row, 1);
    assert_eq!(app.database().unwrap().table_cell, 0);
}

#[tokio::test]
async fn table_editing_requeries_filters_and_can_cancel_a_picker() {
    let (mut app, mut rx, _) = cell_app();
    app.key(KeyCode::Right.into());
    app.key(KeyCode::Right.into());
    app.key(KeyCode::Char('e').into());
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Enter.into());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('n').into());
    assert!(app.editor.is_some());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('y').into());
    assert!(app.editor.is_none());
    app.key(KeyCode::Char('e').into());
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Enter.into());
    app.key(ctrl_s());
    app.receive(rx.recv().await.unwrap());
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.database().unwrap().data.as_ref().unwrap().rows.len(), 1);
    assert_eq!(
        app.database().unwrap().selected_row().unwrap().id,
        "demo-row-1"
    );
    assert_eq!(app.database().unwrap().table_cell, 2);
}

#[test]
fn table_refresh_restores_group_and_property_identity_and_handles_removed_columns() {
    let (mut app, _, _) = cell_app();
    let db = app.databases.get_mut("db").unwrap();
    let data = db.data.as_mut().unwrap();
    let mut view = serde_json::to_value(&data.view).unwrap();
    view["configuration"]["group_by"] =
        serde_json::json!({"type":"multi_select","property_id":"tags"});
    data.view = serde_json::from_value(view).unwrap();
    data.rows[0].page["properties"]["Tags"] = serde_json::json!({"id":"tags","type":"multi_select","multi_select":[{"id":"a","name":"Alpha"},{"id":"b","name":"Beta"}]});
    db.table_row = 1;
    db.selected = 0;
    db.table_cell = 3;
    let mut refreshed = db.data.as_ref().unwrap().clone();
    refreshed.rows.reverse();
    refreshed
        .view
        .configuration
        .as_mut()
        .unwrap()
        .properties
        .as_mut()
        .unwrap()
        .swap(0, 2);
    app.receive(Message::DatabaseRows {
        id: "db".into(),
        generation: 0,
        result: Ok(Box::new(refreshed)),
    });
    let db = app.database().unwrap();
    assert_eq!(db.selected_row().unwrap().id, "demo-row-0");
    assert_eq!(
        db.data.as_ref().unwrap().table_rows()[db.table_row].group,
        "b"
    );
    assert_eq!(db.table_cell, 1);
    let mut refreshed = db.data.as_ref().unwrap().clone();
    refreshed
        .view
        .configuration
        .as_mut()
        .unwrap()
        .properties
        .as_mut()
        .unwrap()
        .remove(0);
    app.receive(Message::DatabaseRows {
        id: "db".into(),
        generation: 0,
        result: Ok(Box::new(refreshed)),
    });
    assert_eq!(app.database().unwrap().table_cell, 0);
}

#[test]
fn database_messages_are_generation_checked_and_view_type_is_shown_in_header() {
    let (mut app, _) = app();
    let meta = crate::service::database::demo_metadata("db");
    let data = crate::service::database::demo_data(meta.views[0].clone());
    app.databases.insert(
        "db".into(),
        DatabaseState {
            generation: 2,
            meta: Some(meta),
            data: Some(data),
            ..Default::default()
        },
    );
    app.receive(Message::DatabaseRows {
        id: "db".into(),
        generation: 1,
        result: Err("obsolete failure".into()),
    });
    assert!(app.databases["db"].error.is_none());
    let block = BlockItem::from_value(
        &serde_json::json!({"id":"db","type":"child_database","child_database":{"title":"Tasks"}}),
    );
    assert_eq!(app.content_type(&block), "child_database (list)");
    app.receive(Message::DatabaseProgress {
        id: "db".into(),
        generation: 1,
        count: 100,
    });
    assert_eq!(app.databases["db"].progress, 0);
}

#[tokio::test]
async fn switching_to_an_unsupported_view_invalidates_an_inflight_load() {
    let (mut app, _) = app();
    app.path.push(Location {
        id: "db".into(),
        title: "DB".into(),
        kind: LocationKind::Database,
        page_id: "page".into(),
        list_depth: 0,
    });
    let mut meta = crate::service::database::demo_metadata("db");
    meta.views[1].view_type = "gallery".into();
    let old_data = crate::service::database::demo_data(meta.views[0].clone());
    app.databases.insert(
        "db".into(),
        DatabaseState {
            meta: Some(meta),
            generation: 0,
            ..Default::default()
        },
    );
    app.key(KeyCode::Char(']').into());
    assert!(!app.loading_content);
    assert!(
        app.database()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("gallery")
    );
    app.receive(Message::DatabaseRows {
        id: "db".into(),
        generation: 0,
        result: Ok(Box::new(old_data)),
    });
    assert!(app.database().unwrap().data.is_none());
    assert_eq!(app.database().unwrap().view().unwrap().view_type, "gallery");
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
        Write::Append { parent, position: InsertPosition::After(after), text, .. }
        if parent == "page" && after == "middle" && text.plain() == "new sibling"));

    app.editor = None;
    app.path.push(Location {
        id: "nested".into(),
        title: "Nested".into(),
        kind: LocationKind::Blocks,
        page_id: "page".into(),
        list_depth: 1,
    });
    app.blocks[0].id = "nested-child".into();
    app.block_selected = 0;
    app.key(KeyCode::Char('a').into());
    assert!(matches!(app.editor.as_ref().unwrap().write().unwrap(),
        Write::Append { parent, position: InsertPosition::After(after), .. }
        if parent == "nested" && after == "nested-child"));

    app.editor = None;
    app.blocks.clear();
    app.key(KeyCode::Char('a').into());
    assert!(matches!(app.editor.as_ref().unwrap().write().unwrap(),
        Write::Append { parent, position: InsertPosition::End, .. } if parent == "nested"));

    app.editor = None;
    app.loading_content = true;
    app.key(KeyCode::Char('a').into());
    assert!(app.editor.is_none());
}

#[test]
fn arrows_move_list_selection_and_letters_remain_text() {
    let (mut app, _) = app();
    app.blocks = vec![paragraph(); 3];
    app.pages = vec![
        PageItem {
            id: "page".into(),
            title: "Page".into()
        };
        3
    ];
    for focus in [Focus::Pages, Focus::Content] {
        app.focus = focus;
        app.page_selected = 1;
        app.block_selected = 1;
        for key in ['h', 'j', 'k', 'l'] {
            app.key(KeyCode::Char(key).into());
        }
        app.key(KeyCode::Left.into());
        app.key(KeyCode::Right.into());
        assert_eq!(app.focus, focus);
        assert_eq!((app.page_selected, app.block_selected), (1, 1));
        app.key(KeyCode::Down.into());
        assert_eq!(
            if focus == Focus::Pages {
                app.page_selected
            } else {
                app.block_selected
            },
            2
        );
        app.key(KeyCode::Up.into());
        assert_eq!((app.page_selected, app.block_selected), (1, 1));
    }
    app.key(KeyCode::Char('a').into());
    for key in ['h', 'j', 'k', 'l'] {
        app.key(KeyCode::Char(key).into());
    }
    assert_eq!(app.editor.as_ref().unwrap().buffer.text, "hjkl");
    app.editor = None;
    app.key(KeyCode::Char('/').into());
    for key in ['h', 'j', 'k', 'l'] {
        app.key(KeyCode::Char(key).into());
    }
    assert_eq!(app.search.as_ref().unwrap().text, "hjkl");
}

#[test]
fn readers_and_link_choosers_use_arrows() {
    let (mut app, _) = app();
    for help in [true, false] {
        app.help = help;
        if !help {
            app.viewer = Some((app.block_labels()[0].clone(), 0));
        }
        app.key(KeyCode::Down.into());
        app.key(KeyCode::Down.into());
        app.key(KeyCode::Up.into());
        assert_eq!(
            if help {
                app.help_scroll
            } else {
                app.viewer.as_ref().unwrap().1
            },
            1
        );
        app.key(KeyCode::Char('j').into());
        // Unbound reader keys close the overlay instead of scrolling.
        assert!(!app.help && app.viewer.is_none());
    }
    app.link_picker = Some(LinkPicker {
        links: vec![
            ("One".into(), "https://one.example".into()),
            ("Two".into(), "https://two.example".into()),
        ],
        selected: 0,
    });
    app.key(KeyCode::Char('j').into());
    assert_eq!(app.link_picker.as_ref().unwrap().selected, 0);
    app.key(KeyCode::Down.into());
    app.key(KeyCode::Char('k').into());
    assert_eq!(app.link_picker.as_ref().unwrap().selected, 1);
    app.key(KeyCode::Up.into());
    assert_eq!(app.link_picker.as_ref().unwrap().selected, 0);
}

#[tokio::test]
async fn insert_before_keeps_its_position_and_draft_after_failure() {
    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        for (selected, expected) in [
            (0, InsertPosition::Start),
            (1, InsertPosition::After("first".into())),
            (2, InsertPosition::After("middle".into())),
        ] {
            let (mut app, mut rx) = app();
            app.path.push(Location {
                id: "nested".into(),
                title: "Nested".into(),
                kind: LocationKind::Blocks,
                page_id: "page".into(),
                list_depth: 1,
            });
            app.blocks = ["first", "middle", "last"]
                .into_iter()
                .map(|id| {
                    let mut block = paragraph();
                    block.id = id.into();
                    block
                })
                .collect();
            app.block_selected = selected;
            app.key(KeyEvent::new(KeyCode::Char('A'), modifiers));
            app.block_selected = 2;
            app.key(KeyCode::Tab.into());
            app.paste("new sibling");
            app.key(ctrl_s());
            app.receive(rx.recv().await.unwrap());
            let editor = app.editor.as_ref().unwrap();
            assert_eq!(editor.buffer.text, "new sibling");
            assert_eq!(editor.kind, Kind::Heading1);
            assert!(editor.error.is_some());
            assert!(
                matches!(editor.write().unwrap(), Write::Append { parent, position, .. }
                if parent == "nested" && position == expected)
            );
            app.key(KeyCode::Esc.into());
            app.key(KeyCode::Char('y').into());
            assert!(app.editor.is_none());
        }
    }
}

#[test]
fn insertion_shortcuts_respect_loading_and_selection_guards() {
    for action in ['a', 'A'] {
        for (loading, selected, empty_id) in
            [(true, 0, false), (false, 10, false), (false, 0, true)]
        {
            let (mut app, _) = app();
            app.loading_content = loading;
            app.block_selected = selected;
            if empty_id {
                app.blocks[0].id.clear();
            }
            app.key(KeyCode::Char(action).into());
            assert!(app.editor.is_none());
        }
    }
    let (mut app, _) = app();
    app.blocks.push(paragraph());
    app.blocks[0].id.clear();
    app.block_selected = 1;
    app.key(KeyCode::Char('A').into());
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
                kind: LocationKind::Page,
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

#[test]
fn table_row_noop_closes_and_protected_rows_cannot_open() {
    let (mut app, mut rx) = app();
    app.blocks = vec![crate::model::tests::table_row()];
    app.key(KeyCode::Char('e').into());
    assert!(app.editor.as_ref().unwrap().row.is_some());
    app.key(ctrl_s());
    assert!(app.editor.is_none());
    assert!(!app.saving);
    assert_eq!(app.status, "No changes");
    assert!(rx.try_recv().is_err());
    app.blocks[0].protected = true;
    app.key(KeyCode::Char('e').into());
    assert!(app.editor.is_none());
    assert!(app.error);
}

#[tokio::test]
async fn table_row_failed_save_retains_all_cells_and_active_cursor() {
    let (mut app, mut rx) = app();
    app.blocks = vec![crate::model::tests::table_row()];
    app.key(KeyCode::Char('e').into());
    app.paste(" first draft");
    app.key(KeyCode::Tab.into());
    app.paste(" styled draft");
    app.key(ctrl_s());
    assert!(app.confirm.is_none());
    assert!(app.saving);
    let draft = app.editor.as_ref().unwrap().row.clone().unwrap();
    app.paste("must not arrive during save");
    app.key(KeyCode::Tab.into());
    app.receive(rx.recv().await.unwrap());
    assert!(!app.saving);
    let editor = app.editor.as_ref().unwrap();
    assert!(editor.error.is_some());
    let retained = editor.row.as_ref().unwrap();
    assert_eq!(retained.active, draft.active);
    for (before, after) in draft.cells.iter().zip(&retained.cells) {
        assert_eq!(before.text, after.text);
        assert_eq!(before.cursor, after.cursor);
    }
    assert!(rx.try_recv().is_err());
    app.key(KeyCode::Tab.into());
    app.paste("你🦀x");
    app.key(KeyCode::Left.into());
    app.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert_eq!(app.editor.as_ref().unwrap().active_buffer().text, "你x");
    app.key(ctrl_s());
    app.receive(rx.recv().await.unwrap());
    assert!(app.confirm.is_none());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('n').into());
    assert!(app.editor.is_some());
    app.key(KeyCode::Esc.into());
    app.key(KeyCode::Char('y').into());
    assert!(app.editor.is_none());
}

#[tokio::test]
async fn saved_table_row_refresh_restores_selection_by_id() {
    let (mut app, _) = app();
    let row = crate::model::tests::table_row();
    app.blocks = vec![row.clone()];
    app.key(KeyCode::Char('e').into());
    app.paste(" revised");
    // Exercise the successful response and a refreshed list with an inserted sibling.
    app.receive(Message::Saved(Ok(None)));
    assert!(app.editor.is_none());
    assert!(app.loading_content);
    app.receive(Message::Content {
        generation: app.content_generation,
        result: Ok((
            app.location().unwrap().clone(),
            vec![
                {
                    let mut sibling = paragraph();
                    sibling.id = "sibling".into();
                    sibling
                },
                {
                    let mut sibling = paragraph();
                    sibling.id = "other".into();
                    sibling
                },
                row,
            ],
        )),
    });
    assert_eq!(app.block_selected, 2);
    assert_eq!(app.blocks[app.block_selected].block_type, "table_row");
}

#[tokio::test]
async fn rich_text_saves_directly_and_protected_runs_stay_read_only() {
    let (mut app, _) = app();
    app.key(KeyCode::Char('e').into());
    app.paste(" revised");
    app.key(ctrl_s());
    assert!(app.confirm.is_none());
    assert!(app.saving);
    assert!(app.editor.is_some());
    app.editor = None;
    app.saving = false;
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
        position: InsertPosition::End,
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

#[tokio::test]
async fn untitled_database_metadata_keeps_the_child_block_title() {
    let (mut app, mut rx) = app();
    app.path.push(Location {
        id: "db".into(),
        title: "table view database".into(),
        kind: LocationKind::Database,
        page_id: "page".into(),
        list_depth: 0,
    });
    app.metadata_generations.insert("db".into(), 1);
    let mut meta = crate::service::database::demo_metadata("db");
    meta.title = "Untitled".into();
    app.receive_database_meta("db".into(), 1, Ok(meta));
    drain_content(&mut app, &mut rx).await;
    assert_eq!(app.location().unwrap().title, "table view database");
}
