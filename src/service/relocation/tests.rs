use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn block(id: &str, parent: &str, kind: &str) -> Value {
    let mut value = json!({"object":"block", "id":id,
        "created_time":"2026-01-01T00:00:00Z", "last_edited_time":"2026-01-01T00:00:00Z",
        "created_by":{"id":"user"}, "last_edited_by":{"id":"user"},
        "has_children":false, "in_trash":false, "type":kind,
        "parent":{"type":"block_id", "block_id":parent},
        kind:{"rich_text":[{"type":"text", "plain_text":"你好 🦀", "href":"https://example.com/",
            "text":{"content":"你好 🦀", "link":{"url":"https://example.com/"}},
            "annotations":{"bold":true,"italic":true,"underline":true,"strikethrough":true,"code":true,"color":"red"}}],
            "color":"blue_background"}});
    if kind == "to_do" {
        value[kind]["checked"] = json!(true);
    }
    value
}

fn movement(source: &Value, indent: bool) -> Relocation {
    Relocation {
        original: BlockItem::from_value(source),
        source_parent: if indent { "page" } else { "parent" }.into(),
        destination: if indent { "parent" } else { "page" }.into(),
        position: if indent {
            InsertPosition::End
        } else {
            InsertPosition::After("parent".into())
        },
        state: RelocationState::Ready,
    }
}

async fn get(server: &MockServer, id: &str, value: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/blocks/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .mount(server)
        .await;
}

async fn override_block(server: &MockServer, id: &str, value: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/blocks/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn setup(server: &MockServer, source: &Value, indent: bool) {
    get(server, "source", source.clone()).await;
    let mut parent = block("parent", "page", "paragraph");
    parent["parent"] = json!({"type":"page_id", "page_id":"page"});
    get(server, "parent", parent.clone()).await;
    if indent {
        Mock::given(method("GET"))
            .and(path("/blocks/page/children"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object":"list", "results":[parent, source], "has_more":false, "next_cursor":null
            })))
            .mount(server)
            .await;
    }
}

fn expected_copy(source: &Value, indent: bool) -> Value {
    let mut copy = source.clone();
    copy["id"] = json!("copy");
    copy["parent"] = if indent {
        json!({"type":"block_id","block_id":"parent"})
    } else {
        json!({"type":"page_id","page_id":"page"})
    };
    copy
}

async fn append(server: &MockServer, source: &Value, indent: bool, response: ResponseTemplate) {
    let mut payload = json!({"children":[leaf_payload(source).unwrap()]});
    if !indent {
        payload["position"] = json!({"type":"after_block", "after_block":{"id":"parent"}});
    }
    Mock::given(method("PATCH"))
        .and(path(if indent {
            "/blocks/parent/children"
        } else {
            "/blocks/page/children"
        }))
        .and(header("Authorization", "Bearer test"))
        .and(header("Notion-Version", NOTION_API_VERSION))
        .and(body_json(payload))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

async fn assert_writes(server: &MockServer, patches: usize, deletes: usize) {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method.to_string() == "PATCH")
            .count(),
        patches
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method.to_string() == "DELETE")
            .count(),
        deletes
    );
}

#[tokio::test]
async fn indent_and_outdent_preserve_content_and_only_delete_after_verification() {
    for indent in [true, false] {
        for kind in [
            "paragraph",
            "bulleted_list_item",
            "numbered_list_item",
            "to_do",
        ] {
            let server = MockServer::start().await;
            let source = block("source", if indent { "page" } else { "parent" }, kind);
            setup(&server, &source, indent).await;
            let copy = expected_copy(&source, indent);
            append(
                &server,
                &source,
                indent,
                ResponseTemplate::new(200).set_body_json(json!({"results":[copy.clone()]})),
            )
            .await;
            let verified = Arc::new(AtomicBool::new(false));
            let flag = verified.clone();
            Mock::given(method("GET"))
                .and(path("/blocks/copy"))
                .respond_with(move |_: &wiremock::Request| {
                    flag.store(true, Ordering::SeqCst);
                    ResponseTemplate::new(200).set_body_json(copy.clone())
                })
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .and(path("/blocks/source"))
                .and(header("Authorization", "Bearer test"))
                .respond_with(move |request: &wiremock::Request| {
                    assert!(verified.load(Ordering::SeqCst));
                    assert!(request.body.is_empty());
                    ResponseTemplate::new(200).set_body_json(json!({"in_trash":true}))
                })
                .expect(1)
                .mount(&server)
                .await;
            let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
            assert_eq!(
                api.relocate(&mut movement(&source, indent)).await.unwrap(),
                "copy"
            );
            let payload = leaf_payload(&source).unwrap();
            assert_eq!(payload[kind]["color"], "blue_background");
            assert_eq!(payload[kind]["rich_text"][0]["annotations"]["code"], true);
            assert!(payload[kind]["rich_text"][0].get("plain_text").is_none());
            assert_eq!(
                payload[kind]["rich_text"][0]["text"]["link"]["url"],
                "https://example.com/"
            );
            if kind == "to_do" {
                assert_eq!(payload[kind]["checked"], true);
            }
            assert_writes(&server, 1, 1).await;
        }
    }
}

#[tokio::test]
async fn copy_failures_retain_source_and_unknown_results_are_never_replayed() {
    for status in [400, 401, 429, 500, 200] {
        let server = MockServer::start().await;
        let source = block("source", "parent", "paragraph");
        setup(&server, &source, false).await;
        append(
            &server,
            &source,
            false,
            ResponseTemplate::new(status).set_body_json(json!({"message":"failed"})),
        )
        .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        let mut request = movement(&source, false);
        assert!(api.relocate(&mut request).await.is_err());
        if status == 500 || status == 200 {
            assert!(matches!(request.state, RelocationState::Uncertain));
            let count = server.received_requests().await.unwrap().len();
            assert!(
                api.relocate(&mut request)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("unknown")
            );
            assert_eq!(server.received_requests().await.unwrap().len(), count);
        } else {
            assert!(matches!(request.state, RelocationState::Ready));
        }
        assert_writes(&server, 1, 0).await;
    }
}

#[tokio::test]
async fn failed_delete_resumes_known_copy_without_another_append() {
    for lost_delete_response in [false, true] {
        let server = MockServer::start().await;
        let source = block("source", "parent", "to_do");
        setup(&server, &source, false).await;
        let copy = expected_copy(&source, false);
        get(&server, "copy", copy.clone()).await;
        append(
            &server,
            &source,
            false,
            ResponseTemplate::new(200).set_body_json(json!({"results":[copy]})),
        )
        .await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let flag = attempts.clone();
        Mock::given(method("DELETE"))
            .and(path("/blocks/source"))
            .respond_with(move |_: &wiremock::Request| {
                let attempt = flag.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(if attempt == 0 { 500 } else { 200 })
                    .set_body_json(json!({"message":"delete failed"}))
            })
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        let mut request = movement(&source, false);
        let error = format!("{:#}", api.relocate(&mut request).await.unwrap_err());
        assert!(error.contains("Replacement copy exists"));
        assert!(matches!(request.state, RelocationState::Copied { .. }));
        if lost_delete_response {
            let mut trashed_source = source.clone();
            trashed_source["in_trash"] = json!(true);
            override_block(&server, "source", trashed_source).await;
        }
        assert_eq!(api.relocate(&mut request).await.unwrap(), "copy");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            if lost_delete_response { 1 } else { 2 }
        );
        assert_writes(&server, 1, if lost_delete_response { 1 } else { 2 }).await;
    }
}

#[tokio::test]
async fn stale_or_unsupported_sources_never_create_a_copy() {
    for change in [
        "timestamp",
        "parent",
        "children",
        "unknown_field",
        "mention",
    ] {
        let server = MockServer::start().await;
        let source = block("source", "parent", "paragraph");
        let mut changed = source.clone();
        match change {
            "timestamp" => changed["last_edited_time"] = json!("2026-01-02T00:00:00Z"),
            "parent" => changed["parent"]["block_id"] = json!("elsewhere"),
            "children" => changed["has_children"] = json!(true),
            "unknown_field" => changed["paragraph"]["unknown"] = json!("must preserve"),
            "mention" => changed["paragraph"]["rich_text"][0]["type"] = json!("mention"),
            _ => unreachable!(),
        }
        get(&server, "source", changed).await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        assert!(api.relocate(&mut movement(&source, false)).await.is_err());
        assert_writes(&server, 0, 0).await;
    }
}

#[tokio::test]
async fn changed_source_or_copy_after_creation_never_trashes_original() {
    for change in ["source", "copy", "copy_parent", "copy_children"] {
        let server = MockServer::start().await;
        let source = block("source", "parent", "paragraph");
        setup(&server, &source, false).await;
        let mut copy = expected_copy(&source, false);
        match change {
            "copy" => copy["paragraph"]["rich_text"] = json!([]),
            "copy_parent" => copy["parent"]["page_id"] = json!("elsewhere"),
            "copy_children" => copy["has_children"] = json!(true),
            _ => {}
        }
        get(&server, "copy", copy.clone()).await;
        let created = Arc::new(AtomicBool::new(false));
        let flag = created.clone();
        Mock::given(method("PATCH"))
            .and(path("/blocks/page/children"))
            .respond_with(move |_: &wiremock::Request| {
                flag.store(true, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({"results":[copy.clone()]}))
            })
            .expect(1)
            .mount(&server)
            .await;
        if change == "source" {
            let source_value = source.clone();
            Mock::given(method("GET"))
                .and(path("/blocks/source"))
                .respond_with(move |_: &wiremock::Request| {
                    let mut source = source_value.clone();
                    if created.load(Ordering::SeqCst) {
                        source["last_edited_time"] = json!("2026-01-02T00:00:00Z");
                    }
                    ResponseTemplate::new(200).set_body_json(source)
                })
                .with_priority(1)
                .mount(&server)
                .await;
        }
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        let mut request = movement(&source, false);
        assert!(api.relocate(&mut request).await.is_err());
        assert!(matches!(request.state, RelocationState::Copied { .. }));
        assert_writes(&server, 1, 0).await;
    }
}

#[tokio::test]
async fn changed_destination_or_sibling_order_prevents_copy() {
    for change in ["parent_moved", "unsupported_parent", "order"] {
        let server = MockServer::start().await;
        let source = block("source", "page", "paragraph");
        setup(&server, &source, true).await;
        if change == "order" {
            Mock::given(method("GET")).and(path("/blocks/page/children"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "object":"list", "results":[source.clone()], "has_more":false, "next_cursor":null
                }))).with_priority(1).mount(&server).await;
        } else {
            let mut parent = block("parent", "page", "paragraph");
            if change == "parent_moved" {
                parent["parent"]["block_id"] = json!("elsewhere");
            } else {
                parent["type"] = json!("table");
            }
            override_block(&server, "parent", parent).await;
        }
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        assert!(api.relocate(&mut movement(&source, true)).await.is_err());
        assert_writes(&server, 0, 0).await;
    }
}
