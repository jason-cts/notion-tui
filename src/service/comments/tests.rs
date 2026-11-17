use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param, query_param_is_missing},
};

pub(crate) fn comment_value(id: &str) -> Value {
    json!({"object":"comment", "id":id, "discussion_id":"thread",
        "created_time":"2026-01-01T00:00:00Z", "last_edited_time":"2026-01-01T00:00:00Z",
        "display_name":{"resolved_name":"Author"}, "created_by":{"id":"bot"},
        "rich_text":[{"type":"text","plain_text":"Hello","text":{"content":"Hello","link":{"url":"https://example.com/"}},
            "annotations":{"bold":true,"color":"blue","code":true}}]})
}

#[tokio::test]
async fn presence_requests_only_one_comment_and_listing_paginates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/comments"))
        .and(query_param("block_id", "block"))
        .and(query_param("page_size", "1"))
        .and(header("Authorization", "Bearer test"))
        .and(header("Notion-Version", NOTION_API_VERSION))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"results":[comment_value("first")],"has_more":false,"next_cursor":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/comments"))
        .and(query_param("page_size", "100"))
        .and(query_param_is_missing("start_cursor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"results":[comment_value("first")],"has_more":true,"next_cursor":"next"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/comments"))
        .and(query_param("page_size", "100"))
        .and(query_param("start_cursor", "next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"results":[comment_value("second")],"has_more":false,"next_cursor":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    assert!(api.has_comments("block").await.unwrap());
    let comments = api.comments("block").await.unwrap();
    assert_eq!(
        comments.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(comments[0].author, "Author");
    assert_eq!(comments[0].input.as_ref().unwrap().plain(), "Hello");
}

#[tokio::test]
async fn creates_block_page_and_discussion_comments_with_rich_text() {
    for target in [
        CommentTarget::Block("block".into()),
        CommentTarget::Page("page".into()),
        CommentTarget::Discussion("thread".into()),
    ] {
        let server = MockServer::start().await;
        let mut body = json!({"rich_text":[{"type":"text","text":{"content":"你好\nComment"}}]});
        match &target {
            CommentTarget::Block(id) => body["parent"] = json!({"block_id":id}),
            CommentTarget::Page(id) => body["parent"] = json!({"page_id":id}),
            CommentTarget::Discussion(id) => body["discussion_id"] = json!(id),
            _ => unreachable!(),
        }
        Mock::given(method("POST"))
            .and(path("/comments"))
            .and(body_json(body))
            // A write-only integration may return a partial object.
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"object":"comment","id":"new"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        assert_eq!(
            api.comment_write(CommentWrite::Save {
                target,
                text: "你好\nComment".into()
            })
            .await
            .unwrap(),
            "new"
        );
    }
}

#[tokio::test]
async fn edits_preserve_links_colors_and_code_and_deletes_check_snapshots() {
    for deleting in [false, true] {
        let server = MockServer::start().await;
        let original = Comment::from_value(&comment_value("comment")).unwrap();
        Mock::given(method("GET"))
            .and(path("/comments/comment"))
            .respond_with(ResponseTemplate::new(200).set_body_json(comment_value("comment")))
            .expect(1)
            .mount(&server)
            .await;
        let write = if deleting {
            CommentWrite::Delete(Box::new(original))
        } else {
            let mut editor = crate::editor::Editor::new(crate::editor::Target::Comment(
                CommentTarget::Edit(Box::new(original)),
            ));
            editor.buffer.insert("!");
            editor.comment_write().unwrap()
        };
        let mut mock = Mock::given(method(if deleting { "DELETE" } else { "PATCH" }))
            .and(path("/comments/comment"));
        if !deleting {
            mock = mock.and(body_json(json!({"rich_text":[{"type":"text","text":{"content":"Hello!","link":{"url":"https://example.com/"}},
                "annotations":{"bold":true,"color":"blue","code":true}}]})));
        }
        mock.respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"object":"comment","id":"comment"})),
        )
        .expect(1)
        .mount(&server)
        .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        assert_eq!(api.comment_write(write).await.unwrap(), "comment");
    }
}

#[tokio::test]
async fn stale_comments_and_invalid_drafts_never_write() {
    let server = MockServer::start().await;
    let original = Comment::from_value(&comment_value("comment")).unwrap();
    let mut changed = comment_value("comment");
    changed["last_edited_time"] = json!("2026-02-01T00:00:00Z");
    Mock::given(method("GET"))
        .and(path("/comments/comment"))
        .respond_with(ResponseTemplate::new(200).set_body_json(changed))
        .expect(2)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    for write in [
        CommentWrite::Save {
            target: CommentTarget::Edit(Box::new(original.clone())),
            text: "draft".into(),
        },
        CommentWrite::Delete(Box::new(original)),
    ] {
        assert!(
            api.comment_write(write)
                .await
                .unwrap_err()
                .to_string()
                .contains("Conflict")
        );
    }
    assert!(
        api.comment_write(CommentWrite::Save {
            target: CommentTarget::Block("block".into()),
            text: "  \n".into()
        })
        .await
        .unwrap_err()
        .to_string()
        .contains("blank")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn malformed_pages_and_repeated_cursors_fail_instead_of_appearing_empty() {
    for body in [
        json!({}),
        json!({"results":[{"id":"incomplete"}],"has_more":false}),
        json!({"results":[],"has_more":true,"next_cursor":"again"}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        assert!(api.comments("block").await.is_err());
    }
}

#[tokio::test]
async fn permissions_rate_limits_and_unreadable_write_responses_are_actionable() {
    for status in [401, 403, 429, 500] {
        let server = MockServer::start().await;
        let code = match status {
            401 => "unauthorized",
            403 => "restricted_resource",
            429 => "rate_limited",
            _ => "internal_server_error",
        };
        Mock::given(method("GET"))
            .and(path("/comments"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("Retry-After", "7")
                    .set_body_json(json!({"object":"error","status":status,"code":code,"message":"Unavailable"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
        let error = api.has_comments("block").await.unwrap_err();
        match status {
            401 => assert!(is_unauthorized(&error)),
            403 => assert!(format!("{error:#}").contains("Read comments")),
            429 => assert!(matches!(
                error.downcast_ref::<rusticnotion::Error>(),
                Some(rusticnotion::Error::RateLimited {
                    retry_after_secs: 7
                })
            )),
            _ => assert!(format!("{error:#}").contains("Unavailable")),
        }
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_string("unreadable"))
        .expect(1)
        .mount(&server)
        .await;
    let api = ApiService::with_base_url("test".into(), &server.uri()).unwrap();
    assert!(
        api.comment_write(CommentWrite::Save {
            target: CommentTarget::Block("block".into()),
            text: "draft".into()
        })
        .await
        .unwrap_err()
        .to_string()
        .contains("inspect Notion")
    );
}
