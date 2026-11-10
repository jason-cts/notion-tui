use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, header, method, path, query_param},
};

fn page(id: &str, title: &str) -> Value {
    json!({"object":"page","id":id,"created_time":"2026-01-01T00:00:00Z",
        "last_edited_time":"2026-01-01T00:00:00Z","archived":false,"icon":null,
        "parent":{"type":"workspace","workspace":true},
        "properties":{"title":{"id":"title","type":"title","title":[
            {"type":"text","plain_text":title,"text":{"content":title}}]}}})
}

fn block(modified: &str) -> Value {
    json!({"object":"block","id":"block","created_time":"2026-01-01T00:00:00Z",
        "last_edited_time":modified,"created_by":{"id":"user"},"last_edited_by":{"id":"user"},
        "has_children":true,"type":"paragraph","paragraph":{"rich_text":[],"color":"default"}})
}

fn list(results: Value, cursor: Option<&str>) -> Value {
    json!({"object":"list","results":results,"has_more":cursor.is_some(),"next_cursor":cursor})
}

#[tokio::test]
async fn search_and_blocks_follow_all_pages_with_opaque_cursors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
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
            ResponseTemplate::new(200).set_body_json(list(json!([page("2", "Notes 二")]), None)),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blocks/page/children"))
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
        .and(header("Notion-Version", "2022-06-28"))
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
        .and(body_json(append_payload(Kind::Todo, "task")))
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
            kind: Kind::Todo,
            text: "task".into(),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("12 seconds"));
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
