//! Database presentation independent of terminal geometry and network requests.
use crate::{
    model::BlockLabel,
    rich_text::{RichRun, external_url},
};
use rusticnotion::{
    chrono::{DateTime, Datelike, Duration, Local, NaiveDate},
    models::view::{GroupBy, View},
};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct DatabaseMeta {
    pub id: String,
    pub title: String,
    pub views: Vec<View>,
}

#[derive(Clone, Debug)]
pub struct DatabaseRow {
    pub id: String,
    pub title: String,
    pub page: Value,
}

#[derive(Clone, Debug)]
pub struct DatabaseData {
    pub view: View,
    pub schema: Value,
    pub rows: Vec<DatabaseRow>,
    pub labels: HashMap<String, String>,
    pub notices: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DatabaseState {
    pub meta: Option<DatabaseMeta>,
    pub view_index: usize,
    pub data: Option<DatabaseData>,
    pub selected: usize,
    pub column: usize,
    pub card: usize,
    pub horizontal: usize,
    pub progress: usize,
    pub error: Option<String>,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub id: String,
    pub name: String,
    pub width: u16,
    pub wrap: bool,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub row: usize,
    pub subgroup: String,
    pub subgroup_key: String,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub key: String,
    pub label: String,
    pub cards: Vec<Card>,
    pub subgroups: Vec<(String, String)>,
}

impl DatabaseState {
    pub fn move_edge(&mut self, last: bool) {
        let Some(data) = &self.data else {
            return;
        };
        if data.view.view_type == "board" && !self.flat_group_fallback() {
            let groups = data.groups(Local::now().date_naive());
            let len = groups.get(self.column).map_or(0, |g| g.cards.len());
            self.card = if last { len.saturating_sub(1) } else { 0 };
            self.selected = groups
                .get(self.column)
                .and_then(|g| g.cards.get(self.card))
                .map_or(usize::MAX, |c| c.row);
        } else {
            self.selected = if last {
                data.rows.len().saturating_sub(1)
            } else {
                0
            };
        }
    }
    fn flat_group_fallback(&self) -> bool {
        self.data.as_ref().is_some_and(|d| {
            d.notices
                .iter()
                .any(|n| n.starts_with("Unsupported grouping"))
        })
    }
    pub fn view(&self) -> Option<&View> {
        self.meta.as_ref()?.views.get(self.view_index)
    }
    pub fn selected_row(&self) -> Option<&DatabaseRow> {
        self.data.as_ref()?.rows.get(self.selected)
    }
    pub fn move_vertical(&mut self, down: bool) {
        let Some(data) = &self.data else { return };
        if data.view.view_type == "board" && !self.flat_group_fallback() {
            let groups = data.groups(Local::now().date_naive());
            if let Some(group) = groups.get(self.column) {
                let index = self.card.min(group.cards.len().saturating_sub(1));
                let next = if down {
                    (index + 1).min(group.cards.len().saturating_sub(1))
                } else {
                    index.saturating_sub(1)
                };
                if let Some(card) = group.cards.get(next) {
                    self.selected = card.row;
                    self.card = next;
                }
            }
        } else if down {
            self.selected = (self.selected + 1).min(data.rows.len().saturating_sub(1));
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }
    pub fn move_horizontal(&mut self, right: bool) {
        let Some(data) = &self.data else { return };
        if data.view.view_type == "board" && !self.flat_group_fallback() {
            let groups = data.groups(Local::now().date_naive());
            let card = self.card;
            self.column = if right {
                (self.column + 1).min(groups.len().saturating_sub(1))
            } else {
                self.column.saturating_sub(1)
            };
            if let Some(group) = groups.get(self.column) {
                if let Some(card) = group
                    .cards
                    .get(card.min(group.cards.len().saturating_sub(1)))
                {
                    self.selected = card.row;
                    self.card = self.card.min(group.cards.len().saturating_sub(1));
                } else {
                    self.selected = usize::MAX;
                    self.card = 0;
                }
            }
        } else {
            let columns = data.columns();
            self.horizontal = if right {
                (self.horizontal + 1).min(columns.len().saturating_sub(1))
            } else {
                self.horizontal.saturating_sub(1)
            };
        }
    }
}

pub fn supported(view: &View) -> bool {
    matches!(view.view_type.as_str(), "list" | "board" | "table")
}

// View configuration uses raw IDs while schemas and page properties may encode
// reserved characters. Decode exactly once; '+' is a literal property-ID byte.
pub fn property_id_key(id: &str) -> String {
    percent_encoding::percent_decode_str(id)
        .decode_utf8_lossy()
        .into_owned()
}

pub fn same_property_id(a: &str, b: &str) -> bool {
    a == b || percent_encoding::percent_decode_str(a).eq(percent_encoding::percent_decode_str(b))
}

pub fn property<'a>(row: &'a DatabaseRow, id: &str, name: &str) -> &'a Value {
    row.page["properties"]
        .as_object()
        .and_then(|p| {
            p.values()
                .find(|p| p["id"].as_str().is_some_and(|p| same_property_id(p, id)))
                .or_else(|| p.get(name))
                .or_else(|| p.get(id))
        })
        .unwrap_or(&Value::Null)
}

fn text_runs(value: &Value) -> Vec<RichRun> {
    value
        .as_array()
        .map(|a| a.iter().map(RichRun::from_value).collect())
        .unwrap_or_default()
}

pub fn plain(value: &Value, labels: &HashMap<String, String>) -> String {
    if value.is_null() {
        return String::new();
    }
    if let Some(s) = value.as_str() {
        return s.into();
    }
    if let Some(a) = value.as_array() {
        return a
            .iter()
            .map(|v| plain(v, labels))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(kind) = value["type"].as_str() {
        if matches!(kind, "title" | "rich_text") {
            return text_runs(&value[kind])
                .iter()
                .map(|r| r.text.as_str())
                .collect();
        }
        if kind == "checkbox" {
            return if value[kind] == true { "☑" } else { "☐" }.into();
        }
        if kind == "unique_id" {
            return format!(
                "{}{}",
                value[kind]["prefix"]
                    .as_str()
                    .map(|p| format!("{p}-"))
                    .unwrap_or_default(),
                value[kind]["number"]
            );
        }
        if let Some(inner) = value.get(kind) {
            return plain(inner, labels);
        }
    }
    if let Some(name) = value["name"].as_str() {
        return name.into();
    }
    if let Some(start) = value["start"].as_str() {
        return match value["end"].as_str() {
            Some(end) => format!("{start} → {end}"),
            None => start.into(),
        };
    }
    if let Some(id) = value["id"].as_str() {
        return labels.get(id).cloned().unwrap_or_else(|| id.into());
    }
    if let Some(n) = value.as_number() {
        return n.to_string();
    }
    if let Some(b) = value.as_bool() {
        return b.to_string();
    }
    String::new()
}

impl DatabaseData {
    pub fn columns(&self) -> Vec<Column> {
        let schema = self.schema["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut all: Vec<_> = schema
            .iter()
            .map(|(name, p)| Column {
                id: p["id"].as_str().unwrap_or(name).into(),
                name: name.clone(),
                width: 20,
                wrap: false,
            })
            .collect();
        all.sort_by_key(|c| (schema[&c.name]["type"] != "title", c.name.to_lowercase()));
        let config = self.view.configuration.as_ref();
        let Some(props) = config.and_then(|c| c.properties.as_ref()) else {
            return all;
        };
        props
            .iter()
            .filter(|p| p.visible != Some(false))
            .filter_map(|p| {
                let mut c = all
                    .iter()
                    .find(|c| same_property_id(&c.id, &p.property_id))
                    .or_else(|| all.iter().find(|c| c.name == p.property_id))?
                    .clone();
                c.width = p.width.map(|w| (w / 8).clamp(8, 40) as u16).unwrap_or(20);
                c.wrap = p
                    .wrap
                    .or(config.and_then(|c| c.wrap_cells))
                    .unwrap_or(false);
                Some(c)
            })
            .collect()
    }
    pub fn cell(&self, row: &DatabaseRow, column: &Column) -> Vec<RichRun> {
        let value = property(row, &column.id, &column.name);
        let kind = value["type"].as_str().unwrap_or("");
        if matches!(kind, "title" | "rich_text") {
            return text_runs(&value[kind]);
        }
        let mut run = RichRun::plain(plain(value, &self.labels));
        if kind == "url" {
            run.url = value[kind].as_str().and_then(external_url);
        }
        vec![run]
    }
    pub fn detail(&self, row: &DatabaseRow) -> BlockLabel {
        let mut runs = vec![RichRun::plain(format!("{}\n\n", row.title))];
        let mut props: Vec<_> = row.page["properties"]
            .as_object()
            .into_iter()
            .flat_map(Map::iter)
            .collect();
        props.sort_by_key(|(name, _)| *name);
        for (name, p) in props {
            runs.push(RichRun::plain(format!("{name}: ")));
            runs.extend(self.cell(
                row,
                &Column {
                    id: p["id"].as_str().unwrap_or(name).into(),
                    name: name.clone(),
                    width: 20,
                    wrap: true,
                },
            ));
            runs.push(RichRun::plain("\n"));
        }
        BlockLabel {
            prefix: String::new(),
            text: runs.iter().map(|r| r.text.as_str()).collect(),
            rich_text: runs,
        }
    }
    pub fn groups(&self, today: NaiveDate) -> Vec<Group> {
        let Some(group) = self
            .view
            .configuration
            .as_ref()
            .and_then(|c| c.group_by.as_ref())
        else {
            return vec![Group {
                key: "all".into(),
                label: "All".into(),
                cards: (0..self.rows.len())
                    .map(|row| Card {
                        row,
                        subgroup: String::new(),
                        subgroup_key: String::new(),
                    })
                    .collect(),
                subgroups: vec![],
            }];
        };
        let sub = self
            .view
            .configuration
            .as_ref()
            .and_then(|c| c.sub_group_by.as_ref());
        let mut groups: Vec<Group> = self
            .seed_groups(group)
            .into_iter()
            .map(|(key, label)| Group {
                key,
                label,
                cards: vec![],
                subgroups: vec![],
            })
            .collect();
        for (row_index, row) in self.rows.iter().enumerate() {
            let keys = self.keys(row, group, today);
            let subkeys = sub
                .map(|s| self.keys(row, s, today))
                .unwrap_or_else(|| vec![(String::new(), String::new())]);
            for (key, label) in keys {
                let index = groups.iter().position(|g| g.key == key).unwrap_or_else(|| {
                    groups.push(Group {
                        key: key.clone(),
                        label,
                        cards: vec![],
                        subgroups: vec![],
                    });
                    groups.len() - 1
                });
                for (subgroup_key, subgroup) in &subkeys {
                    groups[index].cards.push(Card {
                        row: row_index,
                        subgroup: subgroup.clone(),
                        subgroup_key: subgroup_key.clone(),
                    });
                }
            }
        }
        if group.hide_empty_groups == Some(true) {
            groups.retain(|g| !g.cards.is_empty());
        }
        let sort = group
            .sort
            .as_ref()
            .map(|s| s.sort_type.as_str())
            .unwrap_or("manual");
        if sort != "manual" {
            groups.sort_by(|a, b| self.group_order(group, &a.key, &a.label, &b.key, &b.label));
            if sort == "descending" {
                groups.reverse();
            }
        }
        groups.sort_by_key(|g| g.key.is_empty());
        if let Some(sub) = sub {
            for group in &mut groups {
                let mut order = self.seed_groups(sub);
                for card in &group.cards {
                    if !order.iter().any(|(key, _)| *key == card.subgroup_key) {
                        order.push((card.subgroup_key.clone(), card.subgroup.clone()));
                    }
                }
                if sub.hide_empty_groups == Some(true) {
                    order.retain(|(key, _)| group.cards.iter().any(|c| c.subgroup_key == *key));
                }
                let sort = sub
                    .sort
                    .as_ref()
                    .map(|s| s.sort_type.as_str())
                    .unwrap_or("manual");
                if sort != "manual" {
                    order.sort_by(|(ka, la), (kb, lb)| self.group_order(sub, ka, la, kb, lb));
                    if sort == "descending" {
                        order.reverse();
                    }
                }
                order.sort_by_key(|(key, _)| key.is_empty());
                let ranks: HashMap<_, _> = order
                    .iter()
                    .enumerate()
                    .map(|(index, (key, _))| (key.clone(), index))
                    .collect();
                group.cards.sort_by_key(|c| ranks[&c.subgroup_key]);
                group.subgroups = order;
            }
        }
        groups
    }
    fn schema_property(&self, id: &str) -> &Value {
        self.schema["properties"]
            .as_object()
            .and_then(|p| {
                p.values()
                    .find(|p| p["id"].as_str().is_some_and(|p| same_property_id(p, id)))
                    .or_else(|| p.get(id))
            })
            .unwrap_or(&Value::Null)
    }
    fn group_order(
        &self,
        group: &GroupBy,
        ka: &str,
        la: &str,
        kb: &str,
        lb: &str,
    ) -> std::cmp::Ordering {
        // Status columns follow workflow progression, rather than alphabetic
        // labels (which would place Done before Not started).
        if group.property_type == "status" {
            let order = self.seed_groups(group);
            match (
                order.iter().position(|(key, _)| key == ka),
                order.iter().position(|(key, _)| key == kb),
            ) {
                (Some(a), Some(b)) => return a.cmp(&b),
                (Some(_), None) => return std::cmp::Ordering::Less,
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                (None, None) => {}
            }
        }
        group_order(group, ka, la, kb, lb)
    }
    fn seed_groups(&self, group: &GroupBy) -> Vec<(String, String)> {
        let p = self.schema_property(&group.property_id);
        let kind = group.property_type.as_str();
        let field = if kind == "status"
            && group.group_by.as_ref().and_then(Value::as_str) == Some("group")
        {
            "groups"
        } else {
            "options"
        };
        if matches!(kind, "status" | "select" | "multi_select") {
            return p[kind][field]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|v| {
                            (
                                v["id"].as_str().unwrap_or_default().into(),
                                v["name"].as_str().unwrap_or_default().into(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        if kind == "checkbox" {
            return vec![
                ("false".into(), "Unchecked".into()),
                ("true".into(), "Checked".into()),
            ];
        }
        if kind == "formula"
            && let Some(nested) = group
                .group_by
                .as_ref()
                .and_then(|v| serde_json::from_value::<GroupBy>(v.clone()).ok())
        {
            return self.seed_groups(&nested);
        }
        if kind == "number"
            && group.hide_empty_groups != Some(true)
            && let (Some(start), Some(end), Some(size)) = (
                group.range_start,
                group.range_end,
                group.range_size.filter(|s| *s >= 1.0),
            )
        {
            let mut groups = vec![];
            let mut low = start;
            while low < end && groups.len() < 10_000 {
                groups.push((
                    format!("n:{low}"),
                    format!("{low} – {}", (low + size).min(end)),
                ));
                low += size;
            }
            return groups;
        }
        vec![]
    }
    fn keys(&self, row: &DatabaseRow, group: &GroupBy, today: NaiveDate) -> Vec<(String, String)> {
        let p = property(row, &group.property_id, &group.property_id);
        let kind = group.property_type.as_str();
        let actual = match kind {
            "person" => "people",
            "text" => "rich_text",
            k => k,
        };
        let value = &p[actual];
        if kind == "formula"
            && let Some(nested) = group
                .group_by
                .as_ref()
                .and_then(|g| serde_json::from_value::<GroupBy>(g.clone()).ok())
        {
            return group_scalar(&nested, value, today, &self.labels);
        }
        if matches!(
            kind,
            "select"
                | "status"
                | "multi_select"
                | "person"
                | "people"
                | "created_by"
                | "last_edited_by"
                | "relation"
        ) {
            let values: Vec<_> = if let Some(a) = value.as_array() {
                a.iter().collect()
            } else if value.is_null() {
                vec![]
            } else {
                vec![value]
            };
            let mut keys = vec![];
            for v in values {
                let mut id = v["id"].as_str().unwrap_or_default().to_owned();
                let mut label = plain(v, &self.labels);
                if kind == "status"
                    && group.group_by.as_ref().and_then(Value::as_str) == Some("group")
                    && let Some(g) = self.schema_property(&group.property_id)["status"]["groups"]
                        .as_array()
                        .and_then(|groups| {
                            groups.iter().find(|g| {
                                g["option_ids"]
                                    .as_array()
                                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(&id)))
                            })
                        })
                {
                    id = g["id"].as_str().unwrap_or_default().into();
                    label = g["name"].as_str().unwrap_or_default().into();
                }
                if id.is_empty() {
                    id = label.clone();
                }
                if !keys.iter().any(|(key, _)| key == &id) {
                    keys.push((id, label));
                }
            }
            if keys.is_empty() {
                keys.push((String::new(), "No value".into()));
            }
            return keys;
        }
        group_scalar(group, value, today, &self.labels)
    }
}

fn compare_group_keys(a: &str, b: &str) -> std::cmp::Ordering {
    if let (Some(a), Some(b)) = (
        a.strip_prefix("n:").and_then(|s| s.parse::<f64>().ok()),
        b.strip_prefix("n:").and_then(|s| s.parse::<f64>().ok()),
    ) {
        return a.total_cmp(&b);
    }
    a.to_lowercase().cmp(&b.to_lowercase())
}

fn group_order(group: &GroupBy, ka: &str, la: &str, kb: &str, lb: &str) -> std::cmp::Ordering {
    if ka.starts_with("n:") || ka.starts_with("d:") {
        compare_group_keys(ka, kb)
    } else if group.property_type == "checkbox"
        || group
            .group_by
            .as_ref()
            .is_some_and(|v| v["type"] == "checkbox")
    {
        ka.cmp(kb)
    } else {
        la.to_lowercase().cmp(&lb.to_lowercase())
    }
}

fn group_scalar(
    group: &GroupBy,
    raw: &Value,
    today: NaiveDate,
    labels: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let kind = group.property_type.as_str();
    let raw = if raw["type"].is_string() {
        &raw[raw["type"].as_str().unwrap()]
    } else {
        raw
    };
    if raw.is_null() {
        return vec![(String::new(), "No value".into())];
    }
    let mode = group
        .group_by
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or("exact");
    let (key, label) = match kind {
        "checkbox" => {
            let v = raw.as_bool().unwrap_or(false);
            (
                v.to_string(),
                if v { "Checked" } else { "Unchecked" }.into(),
            )
        }
        "number" => {
            let Some(n) = raw.as_f64() else {
                return vec![(String::new(), "No value".into())];
            };
            if let Some(size) = group.range_size.filter(|n| *n >= 1.0) {
                let start = group.range_start.unwrap_or(0.0);
                if n < start {
                    (format!("n:{}", f64::NEG_INFINITY), format!("< {start}"))
                } else if group.range_end.is_some_and(|end| n >= end) {
                    (
                        format!("n:{}", f64::INFINITY),
                        format!("≥ {}", group.range_end.unwrap()),
                    )
                } else {
                    let low = start + ((n - start) / size).floor() * size;
                    let high = (low + size).min(group.range_end.unwrap_or(f64::INFINITY));
                    (format!("n:{low}"), format!("{low} – {high}"))
                }
            } else {
                (format!("n:{n}"), n.to_string())
            }
        }
        "date" | "created_time" | "last_edited_time" => {
            let s = raw["start"]
                .as_str()
                .or_else(|| raw.as_str())
                .unwrap_or_default();
            let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().or_else(|| {
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|d| d.with_timezone(&Local).date_naive())
            });
            let Some(date) = date else {
                return vec![(String::new(), "No value".into())];
            };
            date_group(date, today, mode, group.start_day_of_week.unwrap_or(1))
        }
        _ => {
            let mut s = if raw.is_array() {
                text_runs(raw).iter().map(|r| r.text.as_str()).collect()
            } else {
                plain(raw, labels)
            };
            if mode == "alphabet_prefix" {
                s = s
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
            }
            (s.clone(), s)
        }
    };
    if key.is_empty() {
        vec![(String::new(), "No value".into())]
    } else {
        vec![(key, label)]
    }
}

fn date_group(date: NaiveDate, today: NaiveDate, mode: &str, start: u8) -> (String, String) {
    let week = |d: NaiveDate| {
        d - Duration::days(
            (i64::from(d.weekday().num_days_from_sunday()) - i64::from(start)).rem_euclid(7),
        )
    };
    if mode == "relative" {
        let delta = (date - today).num_days();
        let month = |d: NaiveDate| i64::from(d.year()) * 12 + i64::from(d.month0());
        let (order, label) = if delta == 0 {
            (4, "Today")
        } else if delta == -1 {
            (3, "Yesterday")
        } else if delta == 1 {
            (5, "Tomorrow")
        } else if week(date) == week(today) {
            (6, "This week")
        } else if week(date) == week(today) - Duration::weeks(1) {
            (2, "Last week")
        } else if week(date) == week(today) + Duration::weeks(1) {
            (7, "Next week")
        } else if month(date) >= month(today) - 1 && date < today {
            (1, "Last month")
        } else if month(date) <= month(today) + 1 && date > today {
            (8, "Next month")
        } else if date < today {
            (0, "Older")
        } else {
            (9, "Later")
        };
        return (format!("d:{order}"), label.into());
    }
    let label = match mode {
        "year" => date.format("%Y").to_string(),
        "month" => date.format("%Y-%m").to_string(),
        "week" => week(date).format("%Y-%m-%d").to_string(),
        _ => date.format("%Y-%m-%d").to_string(),
    };
    (label.clone(), label)
}

#[cfg(test)]
mod tests;
