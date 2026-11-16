use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

fn view() -> View {
    serde_json::from_value(json!({"id":"v","type":"list","data_source_id":"source","configuration":{"type":"list","properties":[{"property_id":"title","visible":true}]}})).unwrap()
}

fn cell_target(kind: &str, mut value: Value) -> crate::editor::CellTarget {
    if matches!(kind, "title" | "rich_text") {
        for run in value.as_array_mut().unwrap() {
            run["plain_text"] = run["text"]["content"].clone();
        }
    }
    if matches!(kind, "select" | "status") {
        value["name"] = json!("Option");
        value["color"] = json!("default");
    }
    if kind == "multi_select" {
        for option in value.as_array_mut().unwrap() {
            option["name"] = json!("Option");
            option["color"] = json!("default");
        }
    }
    let mut page = crate::service::tests::page("page", "Page");
    page["properties"]["P"] = json!({"id":"g%3FNq","type":kind,kind:value});
    crate::editor::CellTarget {
        row: DatabaseRow {
            id: "page".into(),
            title: "Page".into(),
            page,
        },
        column: crate::database::Column {
            id: "g?Nq".into(),
            name: "P".into(),
            width: 20,
            wrap: false,
        },
        kind: kind.into(),
        options: vec![],
    }
}

#[tokio::test]
async fn database_cell_writes_update_only_the_requested_property_using_its_encoded_id() {
    for (kind, value) in [
        ("title", crate::model::rich_text("New title")),
        ("rich_text", crate::model::rich_text("你好\n🦀")),
        ("number", json!(42)),
        ("checkbox", json!(false)),
        ("select", json!({"id":"a"})),
        ("status", json!({"id":"b"})),
        ("multi_select", json!([{"id":"a"}])),
        (
            "date",
            json!({"start":"2026-11-13","end":null,"time_zone":null}),
        ),
        ("url", json!("https://example.com")),
        ("email", json!("a@example.com")),
        ("phone_number", json!("123")),
    ] {
        let server = MockServer::start().await;
        let original = cell_target(kind, value.clone());
        Mock::given(method("GET"))
            .and(path("/pages/page"))
            .respond_with(ResponseTemplate::new(200).set_body_json(original.row.page.clone()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/pages/page"))
            .and(header("Authorization", "Bearer test"))
            .and(header("Notion-Version", NOTION_API_VERSION))
            .and(body_json(json!({"properties":{"g%3FNq":{kind:value}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        api.write(Write::UpdateDatabaseCell {
            original: Box::new(original),
            value,
        })
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn database_cell_conflicts_never_send_a_patch() {
    for change in ["timestamp", "type", "id", "missing"] {
        let server = MockServer::start().await;
        let original = cell_target("number", json!(1));
        let mut fresh = original.row.page.clone();
        match change {
            "timestamp" => fresh["last_edited_time"] = json!("2026-01-02T00:00:00Z"),
            "type" => {
                fresh["properties"]["P"] = json!({"id":"g%3FNq","type":"checkbox","checkbox":false})
            }
            "id" => fresh["properties"]["P"]["id"] = json!("replacement"),
            _ => {
                fresh["properties"].as_object_mut().unwrap().remove("P");
            }
        }
        Mock::given(method("GET"))
            .and(path("/pages/page"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fresh))
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        let error = api
            .write(Write::UpdateDatabaseCell {
                original: Box::new(original),
                value: json!(2),
            })
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Conflict"),
            "{change}: {error:#}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn database_cell_failed_writes_are_never_retried() {
    for status in [401, 429, 500, 200] {
        let server = MockServer::start().await;
        let original = cell_target("number", json!(1));
        Mock::given(method("GET"))
            .and(path("/pages/page"))
            .respond_with(ResponseTemplate::new(200).set_body_json(original.row.page.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let response = if status == 200 {
            ResponseTemplate::new(status).set_body_string("unreadable")
        } else {
            ResponseTemplate::new(status)
                .set_body_json(json!({"message":"Failed"}))
                .insert_header("Retry-After", "2")
        };
        Mock::given(method("PATCH"))
            .and(path("/pages/page"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        assert!(
            api.write(Write::UpdateDatabaseCell {
                original: Box::new(original),
                value: json!(2)
            })
            .await
            .is_err()
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn database_views_keep_api_order_and_each_views_data_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/databases/db")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":"database","id":"db","created_time":"2026-01-01T00:00:00Z","last_edited_time":"2026-01-01T00:00:00Z","title":[{"type":"text","plain_text":"Tasks","text":{"content":"Tasks"}}],"parent":{"type":"page_id","page_id":"parent"},"in_trash":false,"data_sources":[{"id":"first","name":"First"},{"id":"second","name":"Second"}]}))).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/views"))
        .and(query_param("database_id", "db"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"results":[{"id":"list"},{"id":"table"}],"has_more":false,"next_cursor":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    for (kind, source) in [("list", "second"), ("table", "first")] {
        Mock::given(method("GET"))
            .and(path(format!("/views/{kind}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"id":kind,"name":kind,"type":kind,"data_source_id":source,"configuration":if kind == "table" {json!({"type":"table","frozen_column_index":-1})} else {Value::Null}}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let meta = api.database("db").await.unwrap();
    assert_eq!(meta.title, "Tasks");
    assert_eq!(meta.views[0].view_type, "list");
    assert_eq!(
        meta.views[0].data_source_id.as_ref().unwrap().to_string(),
        "second"
    );
    assert_eq!(
        meta.views[1].data_source_id.as_ref().unwrap().to_string(),
        "first"
    );
    assert_eq!(
        meta.views[1]
            .configuration
            .as_ref()
            .unwrap()
            .frozen_column_index,
        Some(0)
    );
}

async fn schema(server: &MockServer) {
    let source = json!({"object":"data_source","id":"source","created_time":"2026-01-01T00:00:00Z","last_edited_time":"2026-01-01T00:00:00Z","title":[],"parent":{"type":"database_id","database_id":"db"},"database_parent":{"type":"workspace","workspace":true},"in_trash":false,"properties":{"Name":{"id":"title","type":"title","title":{}}}});
    Mock::given(method("GET"))
        .and(path("/data_sources/source"))
        .respond_with(ResponseTemplate::new(200).set_body_json(source))
        .mount(server)
        .await;
}
fn result(id: &str) -> Value {
    json!({"id":id,"properties":{"Name":{"id":"title","type":"title","title":[]}}})
}
async fn title(server: &MockServer, id: &str) {
    Mock::given(method("GET")).and(path(format!("/pages/{id}/properties/title")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":"list","results":[{"object":"property_item","id":"title","type":"title","title":{"type":"text","plain_text":id,"text":{"content":id}}}],"has_more":false,"next_cursor":null,"property_item":{"id":"title","type":"title","next_url":null}}))).mount(server).await;
}

#[tokio::test]
async fn table_rows_load_missing_page_icons_and_conflict_timestamps() {
    for inline_metadata in [false, true] {
        let server = MockServer::start().await;
        schema(&server).await;
        title(&server, "one").await;
        let mut row = result("one");
        let mut fresh = crate::service::tests::page("one", "Page");
        fresh["icon"] = json!({"type":"emoji","emoji":"🦀"});
        if inline_metadata {
            row["last_edited_time"] = fresh["last_edited_time"].clone();
            row["icon"] = fresh["icon"].clone();
        }
        Mock::given(method("POST"))
            .and(path("/views/v/queries"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"id":"q","view_id":"v","results":[row],"has_more":false,"next_cursor":null}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/views/v/queries/q"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id":"q","deleted":true})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/pages/one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fresh))
            .expect(if inline_metadata { 0 } else { 1 })
            .mount(&server)
            .await;
        let mut table = view();
        table.view_type = "table".into();
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        let data = api.database_rows(&table, Arc::new(|_| {})).await.unwrap();
        assert_eq!(data.rows[0].page["icon"]["emoji"], "🦀");
        assert_eq!(
            data.rows[0].page["last_edited_time"],
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(data.rows[0].title, "one");
        assert!(
            crate::editor::CellTarget::new(&data, &data.rows[0], data.columns()[0].clone()).is_ok()
        );
    }
}

#[tokio::test]
async fn saved_query_paginates_and_cleans_up_before_hydrating_rows() {
    let server = MockServer::start().await;
    schema(&server).await;
    title(&server, "one").await;
    title(&server, "two").await;
    Mock::given(method("POST")).and(path("/views/v/queries")).and(header("Notion-Version",NOTION_API_VERSION)).and(body_json(json!({"page_size":100})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"q","view_id":"v","results":[result("one")],"has_more":true,"next_cursor":"next"}))).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/views/v/queries/q"))
        .and(query_param("start_cursor", "next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"results":[result("two"),result("one")],"has_more":false,"next_cursor":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE")).and(path("/views/v/queries/q")).respond_with(ResponseTemplate::new(500).set_body_json(json!({"object":"error","status":500,"code":"internal_server_error","message":"cleanup failure"}))).expect(1).mount(&server).await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let data = api.database_rows(&view(), Arc::new(|_| {})).await.unwrap();
    assert_eq!(
        data.rows
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
}

#[tokio::test]
async fn invalid_cursor_fails_and_still_deletes_query_cache() {
    let server = MockServer::start().await;
    schema(&server).await;
    Mock::given(method("POST"))
        .and(path("/views/v/queries"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id":"q","view_id":"v","results":[],"has_more":true,"next_cursor":null}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/views/v/queries/q"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"q","deleted":true})))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    assert!(
        api.database_rows(&view(), Arc::new(|_| {}))
            .await
            .unwrap_err()
            .to_string()
            .contains("cursor")
    );
}

#[tokio::test]
async fn expired_snapshot_restarts_once_and_discards_the_old_results() {
    let server = MockServer::start().await;
    schema(&server).await;
    title(&server, "fresh").await;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = calls.clone();
    Mock::given(method("POST")).and(path("/views/v/queries")).respond_with(move |_:&wiremock::Request| {
        let first=requests.fetch_add(1,Ordering::SeqCst)==0;
        ResponseTemplate::new(200).set_body_json(if first {json!({"id":"old","view_id":"v","results":[result("obsolete")],"has_more":true,"next_cursor":"next"})} else {json!({"id":"new","view_id":"v","results":[result("fresh")],"has_more":false,"next_cursor":null})})
    }).expect(2).mount(&server).await;
    Mock::given(method("GET")).and(path("/views/v/queries/old")).respond_with(ResponseTemplate::new(404).set_body_json(json!({"object":"error","status":404,"code":"object_not_found","message":"Expired query"}))).expect(1).mount(&server).await;
    for id in ["old", "new"] {
        Mock::given(method("DELETE"))
            .and(path(format!("/views/v/queries/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":id,"deleted":true})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let data = api.database_rows(&view(), Arc::new(|_| {})).await.unwrap();
    assert_eq!(data.rows.len(), 1);
    assert_eq!(data.rows[0].id, "fresh");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn id_only_pages_and_multivalued_properties_are_fully_hydrated() {
    let server = MockServer::start().await;
    schema(&server).await;
    Mock::given(method("POST")).and(path("/views/v/queries")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"q","view_id":"v","results":[{"id":"one"}],"has_more":false,"next_cursor":null}))).mount(&server).await;
    Mock::given(method("DELETE"))
        .and(path("/views/v/queries/q"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"q","deleted":true})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/pages/one"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(super::super::tests::page("one", "truncated title")),
        )
        .expect(1)
        .mount(&server)
        .await;
    for (cursor, text, next) in [
        (None, "Complete ", Some("next")),
        (Some("next"), "title", None),
    ] {
        let matcher = Mock::given(method("GET")).and(path("/pages/one/properties/title"));
        // Match the first request by its full query; the second includes a cursor.
        let matcher = if let Some(cursor) = cursor {
            matcher.and(query_param("start_cursor", cursor))
        } else {
            matcher
                .and(|r: &wiremock::Request| !r.url.query_pairs().any(|(k, _)| k == "start_cursor"))
        };
        matcher.respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":"list","results":[{"id":"title","type":"title","title":{"type":"text","plain_text":text,"text":{"content":text}}}],"has_more":next.is_some(),"next_cursor":next,"property_item":{"id":"title","type":"title","next_url":null}}))).expect(1).mount(&server).await;
    }
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let data = api.database_rows(&view(), Arc::new(|_| {})).await.unwrap();
    assert_eq!(data.rows[0].title, "Complete title");
}

#[tokio::test]
async fn explicit_rate_limits_are_bounded_and_do_not_repeat_uncertain_requests() {
    let server = MockServer::start().await;
    schema(&server).await;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    Mock::given(method("POST")).and(path("/views/v/queries")).respond_with(move |_:&wiremock::Request| {
        if counter.fetch_add(1,Ordering::SeqCst)==0 {ResponseTemplate::new(429).insert_header("retry-after","1").set_body_json(json!({"object":"error","status":429,"code":"rate_limited","message":"wait"}))}
        else {ResponseTemplate::new(200).set_body_json(json!({"id":"q","view_id":"v","results":[],"has_more":false,"next_cursor":null}))}
    }).expect(2).mount(&server).await;
    Mock::given(method("DELETE"))
        .and(path("/views/v/queries/q"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"q","deleted":true})))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    assert!(
        api.database_rows(&view(), Arc::new(|_| {}))
            .await
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn user_and_relation_labels_are_cached_and_inaccessible_ids_remain_visible() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/alice"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"object":"user","id":"alice","name":"Alice","person":{}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/users/private")).respond_with(ResponseTemplate::new(403).set_body_json(json!({"object":"error","status":403,"code":"restricted_resource","message":"Not shared"}))).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/pages/related"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(super::super::tests::page("related", "Related page")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let page = json!({"properties":{"People":{"id":"people","type":"people","people":[{"id":"alice"},{"id":"private"}]},"Related":{"id":"relation","type":"relation","relation":[{"id":"related"}]}}});
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let mut labels = HashMap::new();
    let required = HashSet::from(["people".into(), "relation".into()]);
    api.resolve_labels(&page, &required, &mut labels).await;
    api.resolve_labels(&page, &required, &mut labels).await;
    assert_eq!(labels["alice"], "Alice");
    assert_eq!(labels["private"], "private");
    assert_eq!(labels["related"], "Related page");
}

#[tokio::test]
async fn encoded_visible_properties_are_hydrated_without_changing_server_row_order() {
    let server = MockServer::start().await;
    schema(&server).await;
    let mut selected = view();
    selected.configuration.as_mut().unwrap().properties = Some(
        serde_json::from_value(json!([
            {"property_id":"title","visible":true},{"property_id":"g?Nq","visible":true}
        ]))
        .unwrap(),
    );
    let rows: Vec<_> = ["three", "two", "one"]
        .into_iter()
        .map(|id| {
            json!({"id":id,"properties":{
                "Name":{"id":"title","type":"title","title":[]},
                "Text":{"id":"g%3FNq","type":"rich_text","rich_text":[]}
            }})
        })
        .collect();
    Mock::given(method("POST"))
        .and(path("/views/v/queries"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id":"q","view_id":"v","results":rows,"has_more":false,"next_cursor":null}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/views/v/queries/q"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"q","deleted":true})))
        .expect(2)
        .mount(&server)
        .await;
    for id in ["three", "two", "one"] {
        title(&server, id).await;
        Mock::given(method("GET")).and(path(format!("/pages/{id}/properties/g%3FNq")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":"list","results":[
                {"id":"g%3FNq","type":"rich_text","rich_text":{"type":"text","plain_text":"Complete text","text":{"content":"Complete text"}}}
            ],"has_more":false,"next_cursor":null,"property_item":{"id":"g%3FNq","type":"rich_text","next_url":null}})))
            .expect(2).mount(&server).await;
    }
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    let data = api
        .database_rows(&selected, Arc::new(|_| {}))
        .await
        .unwrap();
    assert_eq!(
        data.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["three", "two", "one"]
    );
    for row in &data.rows {
        assert_eq!(
            row.page["properties"]["Text"]["rich_text"][0]["plain_text"],
            "Complete text"
        );
    }
    assert!(data.notices.iter().any(|n| n.starts_with("No saved sort")));
    selected.sorts = Some(vec![json!({"property":"title","direction":"descending"})]);
    let sorted = api
        .database_rows(&selected, Arc::new(|_| {}))
        .await
        .unwrap();
    assert!(
        !sorted
            .notices
            .iter()
            .any(|n| n.starts_with("No saved sort"))
    );
}
