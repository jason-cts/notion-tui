use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, header, method, path, query_param},
};

fn page(id: &str, title: &str) -> Value {
    json!({"object":"page","id":id,"created_time":"2026-01-01T00:00:00Z",
        "last_edited_time":"2026-01-01T00:00:00Z","in_trash":false,"icon":null,
        "parent":{"type":"workspace","workspace":true},
        "properties":{"title":{"id":"title","type":"title","title":[
            {"type":"text","plain_text":title,"text":{"content":title}}]}}})
}

fn block(modified: &str) -> Value {
    json!({"object":"block","id":"block","created_time":"2026-01-01T00:00:00Z",
        "last_edited_time":modified,"created_by":{"id":"user"},"last_edited_by":{"id":"user"},
        "has_children":true,"in_trash":false,"type":"paragraph","paragraph":{"rich_text":[],"color":"default"}})
}

fn list(results: Value, cursor: Option<&str>) -> Value {
    json!({"object":"list","results":results,"has_more":cursor.is_some(),"next_cursor":cursor})
}

#[tokio::test]
async fn file_blocks_with_nested_captions_do_not_prevent_loading_page_contents() {
    let server = MockServer::start().await;
    let attachments: Vec<_> = [
        json!({"type":"file", "file":{"url":"https://example.com/internal.txt", "expiry_time":"2026-11-11T00:00:00Z"},
            "caption":[], "name":"internal.txt"}),
        json!({"type":"external", "external":{"url":"https://example.com/external.txt"},
            "caption":[{"type":"text","plain_text":"A caption","text":{"content":"A caption"}}], "name":"external.txt"}),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, file)| {
        let mut value = block("2026-01-01T00:00:00Z");
        value.as_object_mut().unwrap().remove("paragraph");
        value["id"] = json!(format!("attachment-{index}"));
        value["type"] = json!("file");
        value["has_children"] = json!(false);
        value["file"] = file;
        value
    })
    .collect();
    Mock::given(method("GET"))
        .and(path("/blocks/page/children"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(json!(attachments), None)))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let blocks = api.children("page").await.unwrap();
    assert_eq!(blocks.len(), 2);
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(block.id, format!("attachment-{index}"));
        assert_eq!(block.block_type, "file");
        assert!(!block.children);
    }
}

#[tokio::test]
async fn pages_with_block_parents_are_searchable_and_openable() {
    let server = MockServer::start().await;
    let mut nested = page("nested-page", "Page inside a toggle");
    nested["parent"] = json!({"type":"block_id","block_id":"toggle-block"});
    nested["properties"]["Verification"] = json!({
        "id":"verification", "type":"verification",
        "verification":{"state":"unverified","verified_by":null,"date":null}
    });
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(
            json!([page("root", "Root page"), nested.clone()]),
            None,
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pages/nested-page"))
        .respond_with(ResponseTemplate::new(200).set_body_json(nested))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let pages = api.search("").await.unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].id, "nested-page");
    assert_eq!(pages[1].title, "Page inside a toggle");
    assert_eq!(api.page("nested-page").await.unwrap(), pages[1]);
}

#[tokio::test]
async fn search_decode_errors_do_not_suggest_changing_token_permissions() {
    let server = MockServer::start().await;
    let mut malformed = page("page", "Page");
    malformed["parent"] = json!({"type":"block_id"});
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(json!([malformed]), None)))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let error = api.search("").await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.starts_with("Search failed:"), "{message}");
    assert!(message.contains("Error parsing json response"), "{message}");
    assert!(!message.contains("token permissions"), "{message}");
    assert!(!crate::auth::is_unauthorized(&error));
}

#[tokio::test]
async fn search_and_blocks_follow_all_pages_with_opaque_cursors() {
    let server = MockServer::start().await;
    let mut data_source_page = page("2", "Notes 二");
    data_source_page["parent"] = json!({
        "type":"data_source_id", "data_source_id":"source", "database_id":"database"
    });
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("Notion-Version", "2026-03-11"))
        .and(body_json(
            json!({"query":"Notes","filter":{"property":"object","value":"page"}}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(list(json!([page("1", "Notes 一")]), Some("a&b+c"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(json!({"start_cursor":"a&b+c"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(list(json!([data_source_page]), None)),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blocks/page/children"))
        .and(header("Notion-Version", "2026-03-11"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(list(json!([block("2026-01-01T00:00:00Z")]), Some("a&b+c"))),
        )
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blocks/page/children"))
        .and(query_param("start_cursor", "a&b+c"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(json!([]), None)))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let pages = api.search("Notes").await.unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].title, "Notes 二");
    assert_eq!(api.children("page").await.unwrap().len(), 1);
}

#[tokio::test]
async fn conflict_prevents_patch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/blocks/block"))
        .respond_with(ResponseTemplate::new(200).set_body_json(block("2026-02-01T00:00:00Z")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let original = BlockItem::from_value(&block("2026-01-01T00:00:00Z"));
    let error = api
        .write(Write::Update {
            original,
            text: Some("draft".into()),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Conflict"));
}

#[tokio::test]
async fn update_checks_version_and_sends_minimal_patch() {
    let server = MockServer::start().await;
    let value = block("2026-01-01T00:00:00Z");
    Mock::given(method("GET"))
        .and(path("/blocks/block"))
        .respond_with(ResponseTemplate::new(200).set_body_json(value.clone()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/blocks/block"))
        .and(header("Notion-Version", "2026-03-11"))
        .and(header("Authorization", "Bearer test"))
        .and(body_json(
            json!({"paragraph":{"rich_text":[{"type":"text","text":{"content":"new"}}]}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(value.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    api.write(Write::Update {
        original: BlockItem::from_value(&value),
        text: Some("new".into()),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_append_is_not_retried_and_rate_limit_is_actionable() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/blocks/page/children"))
        .and(body_json(append_payload(
            Kind::Todo,
            "task",
            Some("anchor"),
        )))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "12")
                .set_body_json(json!({"message":"rate limited"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let error = api
        .write(Write::Append {
            parent: "page".into(),
            after: Some("anchor".into()),
            kind: Kind::Todo,
            text: "task".into(),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("12 seconds"));
}

#[tokio::test]
async fn insertion_uses_selected_anchor_and_focuses_the_new_sibling_after_refresh() {
    use crate::{
        app::{App, Focus, Location},
        editor::Target,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for (parent, is_page) in [("page", true), ("nested", false)] {
        let server = MockServer::start().await;
        let make_block = |id: &str| {
            let mut value = block("2026-01-01T00:00:00Z");
            value["id"] = json!(id);
            value
        };
        Mock::given(method("PATCH"))
            .and(path(format!("/blocks/{parent}/children")))
            .and(header("Notion-Version", "2026-03-11"))
            .and(body_json(json!({"position":{"type":"after_block","after_block":{"id":"middle"}}, "children":[{
                "object":"block", "type":"paragraph", "paragraph":{"rich_text":[
                    {"type":"text","text":{"content":"new sibling"}}
                ]}
            }]})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(list(json!([make_block("inserted")]), None)),
            )
            .expect(1)
            .mount(&server)
            .await;
        if is_page {
            Mock::given(method("GET"))
                .and(path("/pages/page"))
                .respond_with(ResponseTemplate::new(200).set_body_json(page("page", "Page")))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path(format!("/blocks/{parent}/children")))
            .respond_with(ResponseTemplate::new(200).set_body_json(list(
                json!([
                    make_block("first"),
                    make_block("middle"),
                    make_block("inserted"),
                    make_block("last")
                ]),
                None,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let api = Arc::new(ApiService::with_base_url("test".into(), &server.uri()).unwrap());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(api, tx, false);
        app.path.push(Location {
            id: parent.into(),
            title: parent.into(),
            is_page,
            page_id: "page".into(),
            list_depth: usize::from(!is_page),
        });
        app.focus = Focus::Content;
        app.blocks = ["first", "middle", "last"]
            .into_iter()
            .map(|id| BlockItem::from_value(&make_block(id)))
            .collect();
        app.block_selected = 1;
        app.key(KeyCode::Char('a').into());
        assert!(
            matches!(&app.editor.as_ref().unwrap().target, Target::Append { after: Some(after), .. } if after == "middle")
        );
        app.paste("new sibling");
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        for _ in 0..2 {
            let message = tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .unwrap()
                .unwrap();
            app.receive(message);
        }
        assert!(app.editor.is_none());
        assert!(!app.loading_content);
        assert_eq!(app.blocks[app.block_selected].id, "inserted");
        assert_eq!(app.block_selected, 2);
    }
}

#[tokio::test]
async fn insertion_into_an_empty_container_omits_the_anchor() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/blocks/empty/children"))
        .and(body_json(
            json!({"children":[{"object":"block", "type":"paragraph",
            "paragraph":{"rich_text":[{"type":"text","text":{"content":"first"}}]}}]}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(list(json!([]), None)))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    api.write(Write::Append {
        parent: "empty".into(),
        after: None,
        kind: Kind::Paragraph,
        text: "first".into(),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn creates_child_page_with_rusticnotion() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/pages"))
        .and(body_partial_json(json!({"parent":{"type":"page_id","page_id":"parent"},
            "properties":{"title":{"title":[{"type":"text","text":{"content":"Child"},"plain_text":"Child"}]}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(page("child","Child"))).expect(1).mount(&server).await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let created = api
        .write(Write::Create {
            parent: "parent".into(),
            title: "Child".into(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.id, "child");
}

#[test]
fn rejects_missing_and_repeated_cursors() {
    let mut seen = HashSet::new();
    assert!(next_cursor::<String>(true, None, &mut seen).is_err());
    assert!(next_cursor(true, Some("cursor".to_owned()), &mut seen).is_ok());
    assert!(next_cursor(true, Some("cursor".to_owned()), &mut seen).is_err());
}
