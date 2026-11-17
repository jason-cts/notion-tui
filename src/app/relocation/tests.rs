use super::*;
use crate::{
    app::tests::{app, drain_content},
    model::tests::paragraph,
    service::NotionService,
};
use std::sync::{Arc, Mutex};

struct RelocateService {
    blocks: Mutex<HashMap<String, Vec<BlockItem>>>,
    requests: Mutex<Vec<Relocation>>,
    failure: u8,
}

#[async_trait::async_trait]
impl NotionService for RelocateService {
    async fn search(&self, _: &str) -> anyhow::Result<Vec<PageItem>> {
        Ok(vec![])
    }
    async fn page(&self, id: &str) -> anyhow::Result<PageItem> {
        Ok(PageItem {
            id: id.into(),
            title: "Page".into(),
        })
    }
    async fn children(&self, id: &str) -> anyhow::Result<Vec<BlockItem>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default())
    }
    async fn write(&self, _: Write) -> anyhow::Result<Option<PageItem>> {
        panic!("unexpected write")
    }
    async fn relocate(&self, request: &mut Relocation) -> anyhow::Result<String> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request.clone());
        if requests.len() == 1 && self.failure != 0 {
            request.state = if self.failure == 1 {
                RelocationState::Copied {
                    id: "copy".into(),
                    payload: serde_json::json!({}),
                }
            } else {
                RelocationState::Uncertain
            };
            anyhow::bail!("relocation failed");
        }
        let mut blocks = self.blocks.lock().unwrap();
        blocks
            .get_mut(&request.source_parent)
            .unwrap()
            .retain(|b| b.id != request.original.id);
        let destination = blocks.entry(request.destination.clone()).or_default();
        let index = match &request.position {
            InsertPosition::End => destination.len(),
            InsertPosition::After(id) => destination.iter().position(|b| &b.id == id).unwrap() + 1,
            InsertPosition::Start => 0,
        };
        let mut copy = request.original.clone();
        copy.id = "copy".into();
        destination.insert(index, copy);
        Ok("copy".into())
    }
}

fn named(id: &str) -> BlockItem {
    let mut block = paragraph();
    block.id = id.into();
    block.text = id.into();
    block
}

fn relocation_app(
    indent: bool,
    failure: u8,
) -> (App, mpsc::UnboundedReceiver<Message>, Arc<RelocateService>) {
    let (mut app, _) = app();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut parent = named("parent");
    parent.kind = Some(Kind::Number);
    parent.block_type = "numbered_list_item".into();
    let source = named("source");
    let mut blocks = HashMap::new();
    if indent {
        blocks.insert("page".into(), vec![parent, source, named("tail")]);
        blocks.insert("parent".into(), vec![named("existing-child")]);
    } else {
        blocks.insert("page".into(), vec![parent, named("tail")]);
        blocks.insert("parent".into(), vec![named("existing-child"), source]);
        app.path.push(Location {
            id: "parent".into(),
            title: "Parent".into(),
            kind: LocationKind::Blocks,
            page_id: "page".into(),
            list_depth: 1,
        });
    }
    app.blocks = blocks[if indent { "page" } else { "parent" }].clone();
    app.block_selected = 1;
    let service = Arc::new(RelocateService {
        blocks: Mutex::new(blocks),
        requests: Mutex::new(vec![]),
        failure,
    });
    app.sender = tx;
    app.service = service.clone();
    app.demo = false;
    (app, rx, service)
}

#[tokio::test]
async fn confirmed_indent_and_outdent_follow_destination_and_select_replacement() {
    for indent in [true, false] {
        let (mut app, mut rx, service) = relocation_app(indent, 0);
        let key = KeyCode::Char(if indent { '>' } else { '<' });
        app.key(key.into());
        assert!(app.confirm == Some(Confirm::Relocate));
        assert!(service.requests.lock().unwrap().is_empty());
        for key in [
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Tab,
            KeyCode::Char('q'),
            KeyCode::Char('e'),
        ] {
            app.key(key.into());
            assert!(app.confirm == Some(Confirm::Relocate));
            assert_eq!(app.block_selected, 1);
            assert!(!app.quit && !app.saving);
        }
        app.key(KeyCode::Char('y').into());
        assert!(app.saving);
        app.key(KeyCode::Char('r').into());
        app.key(KeyCode::Char('<').into());
        app.receive(rx.recv().await.unwrap());
        drain_content(&mut app, &mut rx).await;
        assert!(!app.saving && !app.error);
        assert_eq!(app.path.len(), if indent { 2 } else { 1 });
        assert_eq!(
            app.location().unwrap().id,
            if indent { "parent" } else { "page" }
        );
        assert_eq!(app.location().unwrap().page_id, "page");
        assert_eq!(app.location().unwrap().list_depth, usize::from(indent));
        assert_eq!(app.blocks[app.block_selected].id, "copy");
        assert_eq!(app.block_selected, 1);
        assert_eq!(service.requests.lock().unwrap().len(), 1);
        assert!(app.relocation_recovery.is_empty());
        if !indent {
            assert_eq!(
                app.blocks.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
                ["parent", "copy", "tail"]
            );
        }
    }
}

#[tokio::test]
async fn cancellation_and_ineligible_contexts_never_relocate() {
    let (mut app, _, service) = relocation_app(true, 0);
    for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
        app.key(KeyCode::Char('>').into());
        app.key(cancel.into());
        assert!(app.confirm.is_none() && app.pending_relocation.is_none());
    }
    app.block_selected = 0;
    app.key(KeyCode::Char('>').into());
    assert!(app.status.contains("first sibling"));
    app.block_selected = 1;
    app.key(KeyCode::Char('<').into());
    assert!(app.status.contains("Top-level"));
    for invalid in ["children", "protected", "heading", "table", "empty_id"] {
        app.blocks[1] = named("source");
        match invalid {
            "children" => app.blocks[1].children = true,
            "protected" => app.blocks[1].protected = true,
            "heading" => app.blocks[1].kind = Some(Kind::Heading1),
            "table" => app.blocks[1].kind = None,
            "empty_id" => app.blocks[1].id.clear(),
            _ => unreachable!(),
        }
        app.key(KeyCode::Char('>').into());
        assert!(app.confirm.is_none() && app.pending_relocation.is_none());
    }
    app.blocks[1] = named("source");
    app.blocks[0].block_type = "table".into();
    app.key(KeyCode::Char('>').into());
    assert!(app.status.contains("previous sibling"));
    app.loading_content = true;
    app.key(KeyCode::Char('>').into());
    app.loading_content = false;
    app.focus = Focus::Pages;
    app.key(KeyCode::Char('>').into());
    app.focus = Focus::Content;
    app.path.last_mut().unwrap().kind = LocationKind::Database;
    app.key(KeyCode::Char('>').into());
    assert!(app.confirm.is_none() && app.pending_relocation.is_none());
    assert!(service.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failure_retains_source_and_known_copy_can_resume_but_unknown_copy_cannot() {
    for failure in [1, 2] {
        let (mut app, mut rx, service) = relocation_app(true, failure);
        app.key(KeyCode::Char('>').into());
        app.key(KeyCode::Char('y').into());
        app.receive(rx.recv().await.unwrap());
        assert!(app.error && !app.saving && !app.loading_content);
        assert_eq!(app.location().unwrap().id, "page");
        assert_eq!(app.blocks[app.block_selected].id, "source");
        assert!(app.relocation_recovery.contains_key("source"));
        // Either key resumes the captured operation, never plans a second move.
        app.key(KeyCode::Char('<').into());
        if failure == 1 {
            assert!(app.confirm == Some(Confirm::Relocate));
            assert!(app.pending_relocation.as_ref().unwrap().indent);
            app.key(KeyCode::Char('y').into());
            app.receive(rx.recv().await.unwrap());
            drain_content(&mut app, &mut rx).await;
            assert!(matches!(
                service.requests.lock().unwrap()[1].state,
                RelocationState::Copied { .. }
            ));
            assert_eq!(app.blocks[app.block_selected].id, "copy");
        } else {
            assert!(app.confirm.is_none() && app.pending_relocation.is_none());
            assert!(app.status.contains("unknown"));
            assert_eq!(service.requests.lock().unwrap().len(), 1);
        }
    }
}

#[test]
fn angle_brackets_in_editor_and_search_remain_text() {
    let (mut app, _) = app();
    app.key(KeyCode::Char('e').into());
    app.key(KeyCode::Char('>').into());
    app.key(KeyCode::Char('<').into());
    assert!(app.editor.as_ref().unwrap().buffer.text.ends_with("><"));
    assert!(app.confirm.is_none());
    app.editor = None;
    app.key(KeyCode::Char('/').into());
    app.key(KeyCode::Char('>').into());
    app.key(KeyCode::Char('<').into());
    assert_eq!(app.search.as_ref().unwrap().text, "><");
    assert!(app.confirm.is_none());
}
