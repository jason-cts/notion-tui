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
use crate::comments::{Comment, CommentWrite};
use crate::database::{DatabaseData, DatabaseMeta};
use crate::model::{
    BlockItem, InsertPosition, Kind, PageItem, RichText, append_payload, append_table_row_payload,
    table_row_payload, update_payload,
};
use rusticnotion::models::view::View;

mod comments;
pub use comments::CommentReadError;
pub(crate) mod database;
mod relocation;
pub use relocation::{Relocation, RelocationState};
mod table;
pub use table::{ColumnChange, ColumnReplacement};
pub type Progress = Arc<dyn Fn(usize) + Send + Sync>;

#[derive(Clone, Debug)]
pub enum Write {
    UpdateDatabaseCell {
        original: Box<crate::editor::CellTarget>,
        value: Value,
    },
    Delete {
        original: BlockItem,
    },
    Update {
        original: BlockItem,
        text: Option<RichText>,
    },
    UpdateTableRow {
        original: BlockItem,
        cells: Vec<RichText>,
    },
    Append {
        parent: String,
        position: InsertPosition,
        kind: Kind,
        text: RichText,
    },
    AppendTableRow {
        parent: String,
        position: InsertPosition,
        cells: Vec<RichText>,
    },
    Create {
        parent: String,
        title: String,
    },
}

#[async_trait]
pub trait NotionService: Send + Sync {
    async fn has_comments(&self, _id: &str) -> Result<bool> {
        bail!("Comments are unavailable in this service")
    }
    async fn comments(&self, _id: &str) -> Result<Vec<Comment>> {
        bail!("Comments are unavailable in this service")
    }
    async fn comment_write(&self, _write: CommentWrite) -> Result<String> {
        bail!("Comment changes are unavailable in this service")
    }
    async fn search(&self, query: &str) -> Result<Vec<PageItem>>;
    async fn page(&self, id: &str) -> Result<PageItem>;
    async fn children(&self, id: &str) -> Result<Vec<BlockItem>>;
    async fn write(&self, write: Write) -> Result<Option<PageItem>>;
    async fn relocate(&self, _relocation: &mut Relocation) -> Result<String> {
        bail!("Indent/outdent is unavailable in this service")
    }
    async fn replace_table_column(&self, _replacement: &mut ColumnReplacement) -> Result<String> {
        bail!("Column changes are unavailable in this service")
    }
    async fn database(&self, _id: &str) -> Result<DatabaseMeta> {
        bail!("Database browsing is unavailable")
    }
    async fn database_rows(&self, _view: &View, _progress: Progress) -> Result<DatabaseData> {
        bail!("Database views are unavailable")
    }
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
        self.send_write(
            self.http
                .patch(format!("{}{path}", self.base_url))
                .json(&payload),
            false,
        )
        .await
    }

    async fn send_write(&self, request: reqwest::RequestBuilder, deleting: bool) -> Result<()> {
        let response = request.send().await.context(if deleting {
            "Delete response unavailable; inspect Notion before retrying"
        } else {
            "Save response unavailable; inspect Notion before retrying"
        })?;
        self.write_response(response, deleting).await.map(|_| ())
    }

    async fn write_response(&self, response: reqwest::Response, deleting: bool) -> Result<Value> {
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AuthError::Unauthorized.into());
        }
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body: Value = response.json().await.context(if deleting {
            "Unreadable delete response; inspect Notion before retrying"
        } else {
            "Unreadable save response; inspect Notion before retrying"
        })?;
        if !status.is_success() {
            let message = body["message"].as_str().unwrap_or("Request failed");
            if status.as_u16() == 429 {
                bail!(
                    "Rate limited; wait {} seconds before {} again",
                    retry.as_deref().unwrap_or("a few"),
                    if deleting { "deleting" } else { "saving" }
                );
            }
            bail!("Notion {status}: {message}");
        }
        Ok(body)
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
    async fn has_comments(&self, id: &str) -> Result<bool> {
        self.read(|api| {
            let id = id.to_owned();
            async move { api.has_comments(&id).await }
        })
        .await
    }
    async fn comments(&self, id: &str) -> Result<Vec<Comment>> {
        self.read(|api| {
            let id = id.to_owned();
            async move { api.comments(&id).await }
        })
        .await
    }
    async fn comment_write(&self, write: CommentWrite) -> Result<String> {
        let (token, api) = self.client().await?;
        match api.comment_write(write).await {
            Err(error) if is_unauthorized(&error) => {
                self.session.refresh(&token).await.context("Could not renew OAuth login. Comment draft retained; the write was not retried")?;
                bail!(
                    "OAuth login refreshed. Comment write was not retried; draft retained. Submit again when ready"
                )
            }
            result => result,
        }
    }
    async fn relocate(&self, relocation: &mut Relocation) -> Result<String> {
        let (token, api) = self.client().await?;
        match api.relocate(relocation).await {
            Err(error) if is_unauthorized(&error) => {
                self.session
                    .refresh(&token)
                    .await
                    .context("Could not renew OAuth login. Indent/outdent was not retried")?;
                bail!(
                    "OAuth login refreshed. Indent/outdent was not retried; select the original block and confirm again"
                )
            }
            result => result,
        }
    }
    async fn replace_table_column(&self, replacement: &mut ColumnReplacement) -> Result<String> {
        let (token, api) = self.client().await?;
        let result = api.replace_table_column(replacement).await;
        if let Err(error) = &result
            && is_unauthorized(error)
        {
            self.session
                .refresh(&token)
                .await
                .context("Could not renew OAuth login after the table replacement error")?;
            return result.context("OAuth login refreshed. Check the table before retrying");
        }
        result
    }
    async fn database(&self, id: &str) -> Result<DatabaseMeta> {
        self.read(|api| {
            let id = id.to_owned();
            async move { api.database(&id).await }
        })
        .await
    }
    async fn database_rows(&self, view: &View, progress: Progress) -> Result<DatabaseData> {
        self.read(|api| {
            let view = view.clone();
            let progress = progress.clone();
            async move { api.database_rows(&view, progress).await }
        })
        .await
    }
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
        let deleting = matches!(&write, Write::Delete { .. });
        let (token, api) = self.client().await?;
        match api.write(write).await {
            Err(error) if is_unauthorized(&error) => {
                if deleting {
                    self.session.refresh(&token).await.context(
                        "Could not renew OAuth login. Deletion was not retried; check your connection or run notion-tui --login",
                    )?;
                    bail!(
                        "OAuth login refreshed. Deletion was not retried; press d and confirm again when ready"
                    );
                }
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
    async fn has_comments(&self, id: &str) -> Result<bool> {
        self.comment_presence(id).await
    }
    async fn comments(&self, id: &str) -> Result<Vec<Comment>> {
        self.list_comments(id).await
    }
    async fn comment_write(&self, write: CommentWrite) -> Result<String> {
        self.mutate_comment(write).await
    }
    async fn relocate(&self, relocation: &mut Relocation) -> Result<String> {
        self.relocate_block(relocation).await
    }
    async fn replace_table_column(&self, replacement: &mut ColumnReplacement) -> Result<String> {
        self.replace_column(replacement).await
    }
    async fn database(&self, id: &str) -> Result<DatabaseMeta> {
        self.database_metadata(id).await
    }
    async fn database_rows(&self, view: &View, progress: Progress) -> Result<DatabaseData> {
        self.load_database_rows(view, progress).await
    }
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
            Write::UpdateDatabaseCell { original, value } => {
                let modified = original.row.page["last_edited_time"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .context(
                        "Cannot save without a page modification timestamp; cancel and refresh",
                    )?;
                let fresh = serde_json::to_value(
                    self.api
                        .get_page(original.row.id.parse::<PageId>()?)
                        .await
                        .context("Could not check for remote changes; draft retained")?,
                )?;
                let row = crate::database::DatabaseRow {
                    id: original.row.id.clone(),
                    title: String::new(),
                    page: fresh,
                };
                let prop =
                    crate::database::property(&row, &original.column.id, &original.column.name);
                if row.page["last_edited_time"].as_str() != Some(modified)
                    || prop["type"].as_str() != Some(&original.kind)
                    || !prop["id"].as_str().is_some_and(|id| {
                        crate::database::same_property_id(id, &original.column.id)
                    })
                {
                    bail!(
                        "Conflict: page or property changed in Notion. Draft retained; cancel and refresh before editing again"
                    );
                }
                // Use the response's encoded ID exactly as returned. Only this
                // property is sent, leaving every other page value unchanged.
                let id = prop["id"].as_str().unwrap();
                self.patch(
                    &format!("/pages/{}", original.row.id),
                    json!({"properties": {id: {&original.kind: value}}}),
                )
                .await?;
            }
            Write::Delete { original } => {
                if original.id.is_empty() || original.modified.is_empty() {
                    bail!(
                        "Cannot delete a block without an ID and modification timestamp; refresh before deleting"
                    );
                }
                let fresh = self
                    .api
                    .get_block_shallow(original.id.parse::<BlockId>()?)
                    .await
                    .context("Could not check for remote changes; block was not deleted")?;
                let fresh = BlockItem::from_value(&serde_json::to_value(fresh)?);
                if fresh.modified != original.modified || fresh.block_type != original.block_type {
                    bail!("Conflict: block changed in Notion. Refresh before deleting again");
                }
                self.send_write(
                    self.http
                        .delete(format!("{}/blocks/{}", self.base_url, original.id)),
                    true,
                )
                .await?;
            }
            Write::UpdateTableRow { original, cells } => {
                let payload = table_row_payload(&original, &cells).map_err(anyhow::Error::msg)?;
                let fresh = self
                    .api
                    .get_block_shallow(original.id.parse::<BlockId>()?)
                    .await
                    .context("Could not check for remote changes; draft retained")?;
                let fresh = BlockItem::from_value(&serde_json::to_value(fresh)?);
                if original.modified.is_empty()
                    || fresh.modified != original.modified
                    || fresh.block_type != "table_row"
                    || fresh.table_cells.as_ref().map(Vec::len) != Some(cells.len())
                {
                    bail!(
                        "Conflict: table row changed in Notion. Draft retained; cancel and refresh before editing again"
                    );
                }
                self.patch(&format!("/blocks/{}", original.id), payload)
                    .await?;
            }
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
                    update_payload(&original, text.as_ref()),
                )
                .await?;
            }
            Write::Append {
                parent,
                position,
                kind,
                text,
            } => {
                self.patch(
                    &format!("/blocks/{parent}/children"),
                    append_payload(kind, &text, &position),
                )
                .await?;
            }
            Write::AppendTableRow {
                parent,
                position,
                cells,
            } => {
                let payload =
                    append_table_row_payload(&cells, &position).map_err(anyhow::Error::msg)?;
                self.patch(&format!("/blocks/{parent}/children"), payload)
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
    async fn has_comments(&self, id: &str) -> Result<bool> {
        Ok(!comments::demo_comments(id).is_empty())
    }
    async fn comments(&self, id: &str) -> Result<Vec<Comment>> {
        Ok(comments::demo_comments(id))
    }
    async fn comment_write(&self, _write: CommentWrite) -> Result<String> {
        bail!("Demo is read-only. Connect NOTION_API_TOKEN to change comments")
    }
    async fn database(&self, id: &str) -> Result<DatabaseMeta> {
        Ok(database::demo_metadata(id))
    }
    async fn database_rows(&self, view: &View, progress: Progress) -> Result<DatabaseData> {
        let data = database::demo_data(view.clone());
        progress(data.rows.len());
        Ok(data)
    }
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
        if id == "demo-table" {
            return Ok(vec![
                BlockItem::from_value(
                    &json!({"id":"demo-row-plain","type":"table_row","last_edited_time":"demo",
                    "table_row":{"cells":[
                        [{"type":"text","plain_text":"Alpha","text":{"content":"Alpha"}}],
                        [{"type":"text","plain_text":"Beta","text":{"content":"Beta"}}], []
                    ]}}),
                ),
                BlockItem::from_value(
                    &json!({"id":"demo-row-styled","type":"table_row","last_edited_time":"demo",
                    "table_row":{"cells":[
                        [{"type":"text","plain_text":"Styled","text":{"content":"Styled"},"annotations":{"bold":true}}],
                        [{"type":"text","plain_text":"Link","text":{"content":"Link","link":{"url":"https://example.com/"}},"href":"https://example.com/"}], []
                    ]}}),
                ),
            ]);
        }
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
                kind:{"rich_text":[{"type":"text","plain_text":text,"text":{"content":text}}],"checked":false}}),
                )
            })
            .collect();
        if id == "nested" {
            blocks.push(BlockItem::from_value(
                &json!({"id":"demo-table","type":"table","has_children":true,
                "table":{"table_width":3,"has_column_header":false,"has_row_header":false}}),
            ));
        } else {
            blocks.push(BlockItem::from_value(
                &json!({"id":"demo-rich-text", "type":"paragraph", "paragraph":{"rich_text":[
                    {"type":"text","plain_text":"Bold","text":{"content":"Bold"}, "annotations":{"bold":true}},
                    {"type":"text","plain_text":" / ","text":{"content":" / "}},
                    {"type":"text","plain_text":"italic","text":{"content":"italic"}, "annotations":{"italic":true}},
                    {"type":"text","plain_text":" / ","text":{"content":" / "}},
                    {"type":"text","plain_text":"underline","text":{"content":"underline"}, "annotations":{"underline":true}},
                    {"type":"text","plain_text":" / ","text":{"content":" / "}},
                    {"type":"text","plain_text":"strike","text":{"content":"strike"}, "annotations":{"strikethrough":true}},
                    {"type":"text","plain_text":" / ","text":{"content":" / "}},
                    {"type":"text","plain_text":"Google", "href":"https://www.google.com/", "text":{"content":"Google","link":{"url":"https://www.google.com/"}}}
                ]}}),
            ));
            blocks.insert(blocks.len().saturating_sub(1),BlockItem::from_value(&json!({"id":"demo-database","type":"child_database","child_database":{"title":"Task database"},"has_children":false})));
        }
        Ok(blocks)
    }
    async fn write(&self, write: Write) -> Result<Option<PageItem>> {
        if matches!(write, Write::Delete { .. }) {
            bail!("Demo is read-only. Connect NOTION_API_TOKEN to delete blocks");
        }
        bail!("Demo is read-only. Connect NOTION_API_TOKEN to save changes")
    }
}

pub type Service = Arc<dyn NotionService>;

#[cfg(test)]
mod tests;
