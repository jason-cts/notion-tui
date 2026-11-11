use super::*;
use crate::database::{DatabaseRow, property_id_key, same_property_id};
use rusticnotion::{
    ids::{DatabaseId, PropertyId, ViewId},
    models::{paging::Paging, property_item::PagePropertyResponse, view::ViewPage},
};
use std::collections::HashMap;

// Retrying an explicit 429 is safe for these reads and temporary query caches.
// Connection failures and content writes are never retried here.
macro_rules! database_read {
    ($request:expr) => {{
        let mut retries = 0;
        loop {
            match $request.await {
                Err(rusticnotion::Error::RateLimited { retry_after_secs }) if retries < 3 => {
                    retries += 1;
                    tokio::time::sleep(Duration::from_secs(retry_after_secs.max(1))).await;
                }
                result => break result,
            }
        }
    }};
}

impl ApiService {
    pub(super) async fn database_metadata(&self, id: &str) -> Result<DatabaseMeta> {
        let id: DatabaseId = id.parse()?;
        let database = database_read!(self.api.get_database(&id))
            .context("Could not open database; check that it is shared with your integration")?;
        let title = database
            .title
            .iter()
            .map(|t| t.plain_text())
            .collect::<String>();
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut views = vec![];
        loop {
            let list = database_read!(self.api.list_views(
                &id,
                Paging {
                    start_cursor: cursor.clone(),
                    page_size: Some(100),
                },
            ))?;
            for reference in list.results {
                views.push(database_read!(self.api.get_view(&reference.id))?);
            }
            cursor = next_cursor(list.has_more, list.next_cursor, &mut seen)?;
            if cursor.is_none() {
                break;
            }
        }
        Ok(DatabaseMeta {
            id: id.to_string(),
            title,
            views,
        })
    }

    pub(super) async fn load_database_rows(
        &self,
        view: &View,
        progress: Progress,
    ) -> Result<DatabaseData> {
        let source = view
            .data_source_id
            .as_ref()
            .context("This view has no data source")?;
        let schema = serde_json::to_value(database_read!(self.api.get_data_source(source))?)?;
        let mut attempts = 0;
        let pages = loop {
            let result = self.query_snapshot(&view.id, &progress).await;
            match result {
                Err(e) if attempts == 0 && query_expired(&e) => {
                    attempts += 1;
                    progress(0);
                }
                result => break result?,
            }
        };
        let mut rows = vec![];
        let mut labels = HashMap::new();
        let mut notices = vec![];
        let required = required_properties(view, &schema);
        for result in pages {
            let mut page = if result.properties.is_none() {
                serde_json::to_value(database_read!(self.api.get_page(&result.id))?)?
            } else {
                serde_json::to_value(result)?
            };
            let id = page["id"].as_str().context("Missing page ID")?.to_owned();
            if let Some(props) = page["properties"].as_object_mut() {
                for p in props.values_mut() {
                    let kind = p["type"].as_str().unwrap_or_default().to_owned();
                    let prop_id = p["id"].as_str().unwrap_or_default().to_owned();
                    // These arrays can be truncated without an inline cursor. Always use
                    // the property endpoint for properties used in this view.
                    if required.contains(&property_id_key(&prop_id))
                        && matches!(kind.as_str(), "title" | "rich_text" | "people" | "relation")
                    {
                        let values = self.complete_property(&id, &prop_id, &kind).await?;
                        p[&kind] = json!(values);
                        p["has_more"] = json!(false);
                    }
                }
            }
            self.resolve_labels(&page, &required, &mut labels).await;
            let title = page["properties"]
                .as_object()
                .and_then(|p| p.values().find(|p| p["type"] == "title"))
                .map(|p| crate::database::plain(p, &labels))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Untitled".into());
            rows.push(DatabaseRow { id, title, page });
            progress(rows.len());
        }
        if let Some(config) = &view.configuration {
            if config
                .group_by
                .as_ref()
                .is_some_and(|g| g.sort.as_ref().is_none_or(|s| s.sort_type == "manual"))
            {
                notices
                    .push("Manual groups use schema/first-seen order; web order may differ".into());
            }
            if config.group_by.as_ref().is_some_and(uses_dates)
                || config.sub_group_by.as_ref().is_some_and(uses_dates)
            {
                notices.push("Date groups use the terminal's local timezone".into());
            }
            if config
                .additional
                .get("subtasks")
                .is_some_and(|v| !v.is_null())
            {
                notices.push("Sub-items are shown as flat rows".into());
            }
            if config.group_by.as_ref().is_some_and(|g| !known_group(g))
                || config
                    .sub_group_by
                    .as_ref()
                    .is_some_and(|g| !known_group(g))
            {
                notices.push("Unsupported grouping; showing a flat list".into());
            }
        }
        if view.sorts.as_ref().is_none_or(Vec::is_empty) {
            notices
                .push("No saved sort; API row order may differ from the web's manual order".into());
        }
        Ok(DatabaseData {
            view: view.clone(),
            schema,
            rows,
            labels,
            notices,
        })
    }

    async fn query_snapshot(&self, view: &ViewId, progress: &Progress) -> Result<Vec<ViewPage>> {
        let first = database_read!(self.api.create_view_query(view, Some(100)))?;
        let query = first.id;
        let result = async {
            let mut pages = first.results;
            progress(pages.len());
            let mut seen = HashSet::new();
            let mut cursor = next_cursor(first.has_more, first.next_cursor, &mut seen)?;
            while cursor.is_some() {
                let next = database_read!(self.api.get_view_query_results(
                    view,
                    &query,
                    Paging {
                        start_cursor: cursor.clone(),
                        page_size: Some(100),
                    },
                ))?;
                pages.extend(next.results);
                progress(pages.len());
                cursor = next_cursor(next.has_more, next.next_cursor, &mut seen)?;
            }
            let mut ids = HashSet::new();
            pages.retain(|p| ids.insert(p.id.to_string()));
            Ok(pages)
        }
        .await;
        // Cleanup errors must not replace the original result, including OAuth expiry.
        let _ = database_read!(self.api.delete_view_query(view, &query));
        result
    }

    async fn complete_property(&self, page: &str, id: &str, kind: &str) -> Result<Vec<Value>> {
        let page = page.parse::<PageId>()?;
        let id = id.parse::<PropertyId>()?;
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut values = vec![];
        loop {
            match database_read!(self.api.get_page_property(
                &page,
                &id,
                Paging {
                    start_cursor: cursor.clone(),
                    page_size: Some(100),
                },
            ))? {
                PagePropertyResponse::PropertyItem { property_item } => {
                    let v = serde_json::to_value(property_item)?;
                    if let Some(a) = v[kind].as_array() {
                        values.extend(a.clone());
                    } else if !v[kind].is_null() {
                        values.push(v[kind].clone());
                    }
                    break;
                }
                PagePropertyResponse::List { list } => {
                    for item in list.results {
                        let v = serde_json::to_value(item)?;
                        if !v[kind].is_null() {
                            values.push(v[kind].clone());
                        }
                    }
                    cursor = next_cursor(list.has_more, list.next_cursor, &mut seen)?;
                    if cursor.is_none() {
                        break;
                    }
                }
            }
        }
        Ok(values)
    }

    async fn resolve_labels(
        &self,
        page: &Value,
        required: &HashSet<String>,
        labels: &mut HashMap<String, String>,
    ) {
        for p in page["properties"]
            .as_object()
            .into_iter()
            .flat_map(|p| p.values())
        {
            if !required.contains(&property_id_key(p["id"].as_str().unwrap_or_default())) {
                continue;
            }
            let kind = p["type"].as_str().unwrap_or_default();
            if kind == "relation" {
                for v in p[kind].as_array().into_iter().flatten() {
                    if let Some(id) = v["id"].as_str()
                        && !labels.contains_key(id)
                    {
                        let title = match database_read!(
                            self.api.get_page(
                                id.parse::<PageId>()
                                    .expect("identifier parsing is infallible")
                            )
                        ) {
                            Ok(page) => page_item(page).title,
                            Err(_) => id.into(),
                        };
                        labels.insert(id.into(), title);
                    }
                }
            } else if matches!(kind, "people" | "created_by" | "last_edited_by") {
                let values: Vec<_> = p[kind]
                    .as_array()
                    .map(|a| a.iter().collect())
                    .unwrap_or_else(|| vec![&p[kind]]);
                for user in values {
                    if let Some(id) = user["id"].as_str()
                        && !labels.contains_key(id)
                    {
                        let name = if let Some(name) = user["name"].as_str() {
                            name.into()
                        } else if let Ok(profile) = database_read!(
                            self.api.get_user(
                                id.parse::<rusticnotion::ids::UserId>()
                                    .expect("identifier parsing is infallible"),
                            )
                        ) {
                            serde_json::to_value(profile)
                                .ok()
                                .and_then(|v| v["name"].as_str().map(str::to_owned))
                                .unwrap_or_else(|| id.into())
                        } else {
                            id.into()
                        };
                        labels.insert(id.into(), name);
                    }
                }
            }
        }
    }
}

fn query_expired(error: &anyhow::Error) -> bool {
    error.downcast_ref::<rusticnotion::Error>().is_some_and(|e|matches!(e,rusticnotion::Error::ApiError{error} if matches!(error.status.code(),404|410)))
}
fn uses_dates(g: &rusticnotion::models::view::GroupBy) -> bool {
    matches!(
        g.property_type.as_str(),
        "date" | "created_time" | "last_edited_time"
    ) || g.group_by.as_ref().is_some_and(|v| v["type"] == "date")
}
pub(crate) fn known_group(g: &rusticnotion::models::view::GroupBy) -> bool {
    if g.property_type == "formula" {
        return g
            .group_by
            .as_ref()
            .and_then(|v| {
                serde_json::from_value::<rusticnotion::models::view::GroupBy>(v.clone()).ok()
            })
            .is_some_and(|nested| {
                matches!(
                    nested.property_type.as_str(),
                    "date" | "text" | "number" | "checkbox"
                )
            });
    }
    matches!(
        g.property_type.as_str(),
        "select"
            | "multi_select"
            | "status"
            | "person"
            | "people"
            | "created_by"
            | "last_edited_by"
            | "relation"
            | "date"
            | "created_time"
            | "last_edited_time"
            | "text"
            | "title"
            | "url"
            | "email"
            | "phone_number"
            | "number"
            | "checkbox"
            | "formula"
    )
}
fn required_properties(view: &View, schema: &Value) -> HashSet<String> {
    let mut ids = HashSet::new();
    let props = schema["properties"].as_object();
    if let Some(config) = &view.configuration {
        for id in config
            .properties
            .iter()
            .flatten()
            .filter(|p| p.visible != Some(false))
            .map(|p| p.property_id.as_str())
            .chain(
                config
                    .group_by
                    .iter()
                    .chain(config.sub_group_by.iter())
                    .map(|g| g.property_id.as_str()),
            )
        {
            let id = props
                .and_then(|p| {
                    p.values()
                        .find(|p| p["id"].as_str().is_some_and(|p| same_property_id(p, id)))
                        .or_else(|| p.get(id))
                })
                .and_then(|p| p["id"].as_str())
                .unwrap_or(id);
            ids.insert(property_id_key(id));
        }
    }
    for (name, p) in props.into_iter().flatten() {
        if p["type"] == "title"
            || view
                .configuration
                .as_ref()
                .is_none_or(|c| c.properties.is_none())
        {
            ids.insert(property_id_key(p["id"].as_str().unwrap_or(name)));
        }
    }
    ids
}

pub(crate) fn demo_metadata(id: &str) -> DatabaseMeta {
    let views=[("list","List"),("board","Board"),("table","Table")].into_iter().map(|(kind,name)| {
        serde_json::from_value(json!({"id":kind,"name":name,"type":kind,"data_source_id":"demo-source","configuration":{"type":kind,"properties":[{"property_id":"title","visible":true,"width":200},{"property_id":"status","visible":true},{"property_id":"url","visible":true}],"group_by":if kind=="board" {json!({"type":"status","property_id":"status","group_by":"option","sort":{"type":"manual"}})} else {Value::Null}}})).unwrap()
    }).collect();
    DatabaseMeta {
        id: id.into(),
        title: "Task database".into(),
        views,
    }
}
pub(crate) fn demo_data(view: View) -> DatabaseData {
    let rows=[("Alpha 🦀","todo","To do"),("Beta","doing","In progress"),("Gamma","done","Done")].into_iter().enumerate().map(|(i,(title,id,name))| {
        DatabaseRow{id:format!("demo-row-{i}"),title:title.into(),page:json!({"id":format!("demo-row-{i}"),"properties":{"Name":{"id":"title","type":"title","title":[{"plain_text":title,"annotations":{"bold":true}}]},"Status":{"id":"status","type":"status","status":{"id":id,"name":name}},"URL":{"id":"url","type":"url","url":"https://example.com/"}}})}
    }).collect();
    DatabaseData {
        view,
        schema: json!({"properties":{"Name":{"id":"title","type":"title"},"Status":{"id":"status","type":"status","status":{"options":[{"id":"todo","name":"To do"},{"id":"doing","name":"In progress"},{"id":"done","name":"Done"}],"groups":[]}},"URL":{"id":"url","type":"url"}}}),
        rows,
        labels: HashMap::new(),
        notices: vec![],
    }
}

#[cfg(test)]
mod tests;
