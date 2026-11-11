use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::{Client, header};
use rusticnotion::{
    NOTION_API_VERSION, NotionApi,
    ids::{BlockId, PageId},
    models::{Object, PageCreateRequest, paging::Pageable, search::SearchRequest},
};
use serde_json::{Value, json};

use crate::auth::{AuthError, OAuthSession, is_unauthorized};
use crate::model::{BlockItem, Kind, PageItem, append_payload, update_payload};

#[derive(Clone, Debug)]
pub enum Write {
    Update {
        original: BlockItem,
        text: Option<String>,
    },
    Append {
        parent: String,
        after: Option<String>,
        kind: Kind,
        text: String,
    },
    Create {
        parent: String,
        title: String,
    },
}

#[async_trait]
pub trait NotionService: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<PageItem>>;
    async fn page(&self, id: &str) -> Result<PageItem>;
    async fn children(&self, id: &str) -> Result<Vec<BlockItem>>;
    async fn write(&self, write: Write) -> Result<Option<PageItem>>;
}

pub struct ApiService {
    api: NotionApi,
    http: Client,
    base_url: String,
}

impl ApiService {
    pub fn new(token: String) -> Result<Self> {
        Self::with_base_url(token, "https://api.notion.com/v1")
    }

    fn with_base_url(token: String, base: &str) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))?;
        authorization.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, authorization);
        headers.insert(
            "Notion-Version",
            header::HeaderValue::from_static(NOTION_API_VERSION),
        );
        Ok(Self {
            api: NotionApi::with_base_url(token, base)?,
            http: Client::builder()
                .default_headers(headers)
                .timeout(Duration::from_secs(30))
                .build()?,
            base_url: base.trim_end_matches('/').to_owned(),
        })
    }

    async fn patch(&self, path: &str, payload: Value) -> Result<()> {
        let response = self
            .http
            .patch(format!("{}{path}", self.base_url))
            .json(&payload)
            .send()
            .await
            .context("Save response unavailable; inspect Notion before retrying")?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AuthError::Unauthorized.into());
        }
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body: Value = response
            .json()
            .await
            .context("Unreadable save response; inspect Notion before retrying")?;
        if !status.is_success() {
            let message = body["message"].as_str().unwrap_or("Request failed");
            if status.as_u16() == 429 {
                bail!(
                    "Rate limited; wait {} seconds before saving again",
                    retry.as_deref().unwrap_or("a few")
                );
            }
            bail!("Notion {status}: {message}");
        }
        Ok(())
    }
}

/// Shares rotating OAuth credentials while each operation uses a stable client snapshot.
pub struct OAuthService {
    session: OAuthSession,
    clients: tokio::sync::Mutex<(String, Arc<ApiService>)>,
}

impl OAuthService {
    pub async fn new(session: OAuthSession) -> Result<Self> {
        let token = session.token().await;
        let api = Arc::new(ApiService::with_base_url(
            token.clone(),
            session.api_base(),
        )?);
        Ok(Self {
            session,
            clients: tokio::sync::Mutex::new((token, api)),
        })
    }

    async fn client(&self) -> Result<(String, Arc<ApiService>)> {
        let token = self.session.token().await;
        let mut clients = self.clients.lock().await;
        if clients.0 != token {
            *clients = (
                token.clone(),
                Arc::new(ApiService::with_base_url(token, self.session.api_base())?),
            );
        }
        Ok((clients.0.clone(), clients.1.clone()))
    }

    async fn read<T: Send, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: Fn(Arc<ApiService>) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<T>> + Send,
    {
        let (token, api) = self.client().await?;
        match operation(api).await {
            Err(error) if is_unauthorized(&error) => {
                self.session.refresh(&token).await.context("Could not renew OAuth login; saved credentials were preserved. Check your connection or run notion-tui --login")?;
                let (_, api) = self.client().await?;
                operation(api).await.context("Request failed after OAuth renewal; if authorization was rejected, run notion-tui --login")
            }
            result => result,
        }
    }
}

#[async_trait]
impl NotionService for OAuthService {
    async fn search(&self, query: &str) -> Result<Vec<PageItem>> {
        self.read(|api| {
            let query = query.to_owned();
            async move { api.search(&query).await }
        })
        .await
    }
    async fn page(&self, id: &str) -> Result<PageItem> {
        self.read(|api| {
            let id = id.to_owned();
            async move { api.page(&id).await }
        })
        .await
    }
    async fn children(&self, id: &str) -> Result<Vec<BlockItem>> {
        self.read(|api| {
            let id = id.to_owned();
            async move { api.children(&id).await }
        })
        .await
    }
    async fn write(&self, write: Write) -> Result<Option<PageItem>> {
        let (token, api) = self.client().await?;
        match api.write(write).await {
            Err(error) if is_unauthorized(&error) => {
                self.session.refresh(&token).await.context("Could not renew OAuth login. Draft retained; check your connection or run notion-tui --login")?;
                bail!(
                    "OAuth login refreshed. Draft retained; save again when ready. The write was not retried"
                );
            }
            result => result,
        }
    }
}

fn page_item(page: rusticnotion::models::Page) -> PageItem {
    PageItem {
        title: page
            .title()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Untitled".into()),
        id: page.id.to_string(),
    }
}

fn next_cursor<T: serde::Serialize>(
    has_more: bool,
    cursor: Option<T>,
    seen: &mut HashSet<String>,
) -> Result<Option<T>> {
    if !has_more {
        return Ok(None);
    }
    let cursor = cursor.context("Notion returned has_more without a cursor")?;
    if !seen.insert(serde_json::to_string(&cursor)?) {
        bail!("Notion repeated a pagination cursor");
    }
    Ok(Some(cursor))
}

#[async_trait]
impl NotionService for ApiService {
    async fn search(&self, query: &str) -> Result<Vec<PageItem>> {
        let mut pages = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let response = self
                .api
                .search(SearchRequest::pages(query).start_from(cursor))
                .await
                .context("Search failed")?;
            for item in response.results {
                if let Object::Page { page } = item {
                    pages.push(page_item(page));
                }
            }
            cursor = next_cursor(response.has_more, response.next_cursor, &mut seen)?;
            if cursor.is_none() {
                return Ok(pages);
            }
        }
    }

    async fn page(&self, id: &str) -> Result<PageItem> {
        Ok(page_item(self.api.get_page(id.parse::<PageId>()?).await?))
    }

    async fn children(&self, id: &str) -> Result<Vec<BlockItem>> {
        let id: BlockId = id.parse()?;
        let mut blocks = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let response = self.api.get_block_children(&id, cursor).await?;
            for block in response.results {
                blocks.push(BlockItem::from_value(&serde_json::to_value(block)?));
            }
            cursor = next_cursor(response.has_more, response.next_cursor, &mut seen)?;
            if cursor.is_none() {
                return Ok(blocks);
            }
        }
    }

    async fn write(&self, write: Write) -> Result<Option<PageItem>> {
        match write {
            Write::Update { original, text } => {
                let fresh = self
                    .api
                    .get_block_shallow(original.id.parse::<BlockId>()?)
                    .await
                    .context("Could not check for remote changes; draft retained")?;
                let fresh = BlockItem::from_value(&serde_json::to_value(fresh)?);
                if original.modified.is_empty() || fresh.modified != original.modified {
                    bail!(
                        "Conflict: block changed in Notion. Draft retained; cancel and refresh before editing again"
                    );
                }
                self.patch(
                    &format!("/blocks/{}", original.id),
                    update_payload(&original, text.as_deref()),
                )
                .await?;
            }
            Write::Append {
                parent,
                after,
                kind,
                text,
            } => {
                self.patch(
                    &format!("/blocks/{parent}/children"),
                    append_payload(kind, &text, after.as_deref()),
                )
                .await?;
            }
            Write::Create { parent, title } => {
                let request = PageCreateRequest {
                    parent: serde_json::from_value(json!({"type":"page_id","page_id":parent}))?,
                    // The response model requires property IDs and plain_text. Both are valid
                    // optional fields on the create request; use the built-in title identifier.
                    properties: serde_json::from_value(
                        json!({"title":{"id":"title","type":"title",
                        "title":[{"type":"text","text":{"content":title},"plain_text":title}]}}),
                    )?,
                    children: None,
                };
                let page = self
                    .api
                    .create_page(request)
                    .await
                    .context("Page creation failed; inspect Notion before retrying")?;
                return Ok(Some(page_item(page)));
            }
        }
        Ok(None)
    }
}

/// A credential-free, read-only workspace for checking terminal behavior.
pub struct DemoService;

#[async_trait]
impl NotionService for DemoService {
    async fn search(&self, query: &str) -> Result<Vec<PageItem>> {
        Ok(vec![PageItem {
            id: "demo".into(),
            title: "Welcome to Notion TUI".into(),
        }]
        .into_iter()
        .filter(|p| p.title.to_lowercase().contains(&query.to_lowercase()))
        .collect())
    }
    async fn page(&self, id: &str) -> Result<PageItem> {
        Ok(PageItem {
            id: id.into(),
            title: "Welcome to Notion TUI".into(),
        })
    }
    async fn children(&self, id: &str) -> Result<Vec<BlockItem>> {
        let rows = if id == "nested" {
            vec![("paragraph", "Nested content · Backspace goes back", false)]
        } else {
            vec![
                ("heading_1", "Your workspace, at your fingertips", false),
                (
                    "paragraph",
                    "Search pages with /. Open with Enter. Switch panes with Tab.",
                    false,
                ),
                (
                    "to_do",
                    "Use Space to toggle a task in a connected workspace",
                    false,
                ),
                ("toggle", "Explore nested blocks — Enter to open", true),
                ("code", "NOTION_API_TOKEN=… cargo run", false),
                ("paragraph", "Unicode works here: 你好 · café · 🦀", false),
            ]
        };
        let mut blocks: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(i, (kind, text, children))| {
                BlockItem::from_value(
                    &json!({"id":if children { "nested".into() } else { format!("demo-{i}") },
                "type":kind,"last_edited_time":"demo","has_children":children,
                kind:{"rich_text":[{"type":"text","plain_text":text}],"checked":false}}),
                )
            })
            .collect();
        if id != "nested" {
            blocks.push(BlockItem::from_value(
                &json!({"id":"demo-rich-text", "type":"paragraph", "paragraph":{"rich_text":[
                    {"type":"text","plain_text":"Bold", "annotations":{"bold":true}},
                    {"type":"text","plain_text":" / "},
                    {"type":"text","plain_text":"italic", "annotations":{"italic":true}},
                    {"type":"text","plain_text":" / "},
                    {"type":"text","plain_text":"underline", "annotations":{"underline":true}},
                    {"type":"text","plain_text":" / "},
                    {"type":"text","plain_text":"strike", "annotations":{"strikethrough":true}},
                    {"type":"text","plain_text":" / "},
                    {"type":"text","plain_text":"Google", "href":"https://www.google.com/"}
                ]}}),
            ));
        }
        Ok(blocks)
    }
    async fn write(&self, _: Write) -> Result<Option<PageItem>> {
        bail!("Demo is read-only. Connect NOTION_API_TOKEN to save changes")
    }
}

pub type Service = Arc<dyn NotionService>;

#[cfg(test)]
mod tests;
