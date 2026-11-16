use super::Buffer;
use crate::{
    database::{Column, DatabaseData, DatabaseRow, property},
    model::TableCell,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rusticnotion::chrono::{DateTime, NaiveDate, NaiveDateTime};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct CellTarget {
    pub row: DatabaseRow,
    pub column: Column,
    pub kind: String,
    pub options: Vec<Value>,
}

impl CellTarget {
    pub fn new(
        data: &DatabaseData,
        row: &DatabaseRow,
        column: Column,
    ) -> Result<Self, &'static str> {
        let p = property(row, &column.id, &column.name);
        let kind = p["type"]
            .as_str()
            .ok_or("Property is unavailable; refresh before editing")?;
        if !matches!(
            kind,
            "title"
                | "rich_text"
                | "number"
                | "checkbox"
                | "select"
                | "status"
                | "multi_select"
                | "date"
                | "url"
                | "email"
                | "phone_number"
        ) {
            return Err("This property type is read-only");
        }
        if matches!(kind, "title" | "rich_text") && TableCell::from_value(&p[kind]).input.is_none()
        {
            return Err("Mentions, equations, and unrecognized rich text are read-only");
        }
        if p["has_more"] == true {
            return Err("Property is incomplete; refresh before editing");
        }
        let options = data.schema_property(&column.id)[kind]["options"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(Self {
            row: row.clone(),
            column,
            kind: kind.into(),
            options,
        })
    }
}

#[derive(Clone, Debug)]
pub struct CellDraft {
    pub kind: String,
    pub fields: Vec<Buffer>,
    pub active: usize,
    /// Choice 0 clears single-select fields. Multi-select uses only real options.
    pub choices: Vec<(String, Value)>,
    pub checked: Vec<bool>,
    pub cursor: usize,
    initial_fields: Vec<String>,
    initial_checked: Vec<bool>,
}

impl CellDraft {
    pub fn new(target: &CellTarget) -> Self {
        let p = property(&target.row, &target.column.id, &target.column.name);
        let value = &p[&target.kind];
        let kind = target.kind.as_str();
        let fields = match kind {
            "title" | "rich_text" => vec![TableCell::from_value(value).text],
            "number" => vec![if value.is_null() {
                String::new()
            } else {
                value.to_string()
            }],
            "date" => ["start", "end", "time_zone"]
                .iter()
                .map(|k| value[k].as_str().unwrap_or_default().to_owned())
                .collect(),
            _ => vec![value.as_str().unwrap_or_default().to_owned()],
        };
        let mut choices = vec![];
        let mut checked = vec![];
        if kind == "checkbox" {
            choices.push(("Checked".into(), json!(true)));
            checked.push(value == true);
        } else if matches!(kind, "select" | "status" | "multi_select") {
            if kind != "multi_select" {
                choices.push((
                    if kind == "status" {
                        "Reset to default"
                    } else {
                        "No value"
                    }
                    .into(),
                    Value::Null,
                ));
                checked.push(value.is_null());
            }
            for option in &target.options {
                let selected = |v: &Value| {
                    if option["id"].is_string() && v["id"].is_string() {
                        option["id"] == v["id"]
                    } else {
                        option["name"] == v["name"]
                    }
                };
                let is_checked = if kind == "multi_select" {
                    value.as_array().is_some_and(|a| a.iter().any(selected))
                } else {
                    selected(value)
                };
                let input = if option["id"].is_string() {
                    json!({"id":option["id"]})
                } else {
                    json!({"name":option["name"]})
                };
                choices.push((
                    option["name"].as_str().unwrap_or("Unnamed option").into(),
                    input,
                ));
                checked.push(is_checked);
            }
        }
        let cursor = checked.iter().position(|v| *v).unwrap_or(0);
        Self {
            kind: target.kind.clone(),
            initial_fields: fields.clone(),
            fields: if matches!(kind, "title" | "rich_text") {
                vec![Buffer::from_rich(
                    TableCell::from_value(value)
                        .input
                        .as_deref()
                        .unwrap_or_default(),
                )]
            } else {
                fields.into_iter().map(Buffer::new).collect()
            },
            active: 0,
            choices,
            initial_checked: checked.clone(),
            checked,
            cursor,
        }
    }

    pub fn is_picker(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "checkbox" | "select" | "status" | "multi_select"
        )
    }

    pub fn changed(&self) -> bool {
        self.fields
            .iter()
            .map(|f| &f.text)
            .ne(self.initial_fields.iter())
            || self.fields.iter().any(Buffer::rich_changed)
            || self.checked != self.initial_checked
    }

    pub fn key(&mut self, key: KeyEvent) {
        if self.is_picker() {
            match key.code {
                KeyCode::Down => {
                    self.cursor = (self.cursor + 1).min(self.choices.len().saturating_sub(1))
                }
                KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char(' ') if !self.checked.is_empty() => {
                    if matches!(self.kind.as_str(), "select" | "status") {
                        self.checked.fill(false);
                        self.checked[self.cursor] = true;
                    } else {
                        self.checked[self.cursor] = !self.checked[self.cursor];
                    }
                }
                _ => {}
            }
        } else if self.kind == "date" && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            let backwards =
                key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
            self.active = (self.active + if backwards { 2 } else { 1 }) % 3;
        } else if self.kind == "date"
            && key.code == KeyCode::Char('d')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.fields.iter_mut().for_each(|f| *f = Buffer::default());
        } else {
            self.fields[self.active].key(key, matches!(self.kind.as_str(), "title" | "rich_text"));
        }
    }

    /// The value inside the property's type key; no response-only metadata.
    pub fn value(&self) -> Result<Value, &'static str> {
        let text = self.fields[0].text.as_str();
        match self.kind.as_str() {
            "title" | "rich_text" => {
                if text.chars().count() > 100_000 {
                    return Err("Text is limited to 100,000 characters");
                }
                Ok(json!(self.fields[0].rich_text()))
            }
            "number" if text.trim().is_empty() => Ok(Value::Null),
            "number" => text
                .trim()
                .parse::<serde_json::Number>()
                .map(Value::Number)
                .map_err(|_| "Enter a finite number, or leave empty to clear"),
            "checkbox" => Ok(json!(self.checked[0])),
            "select" | "status" => Ok(self
                .choices
                .iter()
                .zip(&self.checked)
                .find(|(_, checked)| **checked)
                .map_or(Value::Null, |((_, value), _)| value.clone())),
            "multi_select" => {
                let values: Vec<_> = self
                    .choices
                    .iter()
                    .zip(&self.checked)
                    .filter(|(_, checked)| **checked)
                    .map(|((_, value), _)| value.clone())
                    .collect();
                if values.len() > 100 {
                    return Err("Select at most 100 options");
                }
                Ok(json!(values))
            }
            "date" => self.date(),
            "url" | "email" | "phone_number" => {
                if text.contains(['\n', '\r']) {
                    return Err("This property must fit on one line");
                }
                let limit = if self.kind == "url" { 2000 } else { 200 };
                if text.chars().count() > limit {
                    return Err("Value exceeds this property's length limit");
                }
                Ok(if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                })
            }
            _ => Err("This property type is read-only"),
        }
    }

    fn date(&self) -> Result<Value, &'static str> {
        let fields: Vec<_> = self.fields.iter().map(|b| b.text.trim()).collect();
        if fields.iter().all(|s| s.is_empty()) {
            return Ok(Value::Null);
        }
        let timezone = fields[2];
        let parse = |s: &str| -> Result<(u8, NaiveDateTime), &'static str> {
            if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                return Ok((0, d.and_hms_opt(0, 0, 0).unwrap()));
            }
            if timezone.is_empty() {
                DateTime::parse_from_rfc3339(s)
                    .map(|d| (1, d.naive_utc()))
                    .map_err(|_| "Use YYYY-MM-DD or an ISO timestamp with an offset")
            } else {
                NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                    .map(|d| (2, d))
                    .map_err(|_| "With a timezone, use a local ISO timestamp without an offset")
            }
        };
        let start = parse(fields[0])?;
        if !fields[1].is_empty() {
            let end = parse(fields[1])?;
            if start.0 != end.0 || end.1 < start.1 {
                return Err("End must match the start's format and be at or after start");
            }
        }
        if !timezone.is_empty()
            && (start.0 != 2
                || timezone
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err("Timezone requires a local timestamp and an IANA name such as Asia/Taipei");
        }
        Ok(
            json!({"start": fields[0], "end": if fields[1].is_empty() { Value::Null } else { json!(fields[1]) }, "time_zone": if timezone.is_empty() { Value::Null } else { json!(timezone) }}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, Target};

    fn target(kind: &str, value: Value) -> CellTarget {
        let options = json!([{"id":"a","name":"Alpha"},{"id":"b","name":"Beta"}]);
        let data = DatabaseData {
            view: serde_json::from_value(json!({"id":"v","type":"table"})).unwrap(),
            schema: json!({"properties":{"P":{"id":"g%3FNq","type":kind, kind:{"options":options}}}}),
            rows: vec![],
            labels: Default::default(),
            notices: vec![],
        };
        let row = DatabaseRow {
            id: "page".into(),
            title: "Page".into(),
            page: json!({"last_edited_time":"2026-01-01T00:00:00Z","properties":{"P":{"id":"g%3FNq","type":kind,kind:value}}}),
        };
        CellTarget::new(
            &data,
            &row,
            Column {
                id: "g?Nq".into(),
                name: "P".into(),
                width: 20,
                wrap: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn text_editing_preserves_drafts_styles_and_links() {
        for kind in ["title", "rich_text"] {
            let target = target(
                kind,
                json!([{"type":"text","text":{"content":"Hello","link":{"url":"https://example.com"}},"annotations":{"bold":true}}]),
            );
            let mut e = Editor::new(Target::DatabaseCell(Box::new(target)));
            assert!(!e.changed());
            e.insert("\r\n你好🦀");
            e.key(KeyCode::Left.into());
            e.insert("x");
            let crate::service::Write::UpdateDatabaseCell { value, .. } = e.write().unwrap() else {
                panic!()
            };
            assert_eq!(value[0]["text"]["content"], e.active_buffer().text);
            assert_eq!(value[0]["annotations"]["bold"], true);
            assert_eq!(value[0]["text"]["link"]["url"], "https://example.com");
            e.cell.as_mut().unwrap().fields[0] = Buffer::new("🦀".repeat(2001));
            assert_eq!(
                e.cell
                    .as_ref()
                    .unwrap()
                    .value()
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
    }

    #[test]
    fn title_and_rich_text_save_formatting_only_changes() {
        for kind in ["title", "rich_text"] {
            let original = target(
                kind,
                json!([{"type":"text","text":{"content":"Hello","link":{"url":"https://example.com/"}},"annotations":{"code":true,"color":"red"}}]),
            );
            let mut editor = Editor::new(Target::DatabaseCell(Box::new(original)));
            editor.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
            editor.key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL));
            assert!(editor.changed());
            let crate::service::Write::UpdateDatabaseCell { value, .. } = editor.write().unwrap()
            else {
                panic!()
            };
            assert_eq!(value[0]["annotations"]["italic"], true);
            assert_eq!(value[0]["annotations"]["code"], true);
            assert_eq!(value[0]["annotations"]["color"], "red");
            assert_eq!(value[0]["text"]["link"]["url"], "https://example.com/");
            let reopened =
                Buffer::from_rich(TableCell::from_value(&value).input.as_deref().unwrap());
            assert_eq!(reopened.style_states()[1], Some(true));
        }
    }

    #[test]
    fn protected_and_unsupported_properties_are_rejected() {
        let t = target("rich_text", json!([]));
        let data = DatabaseData {
            view: serde_json::from_value(json!({"id":"v","type":"table"})).unwrap(),
            schema: Value::Null,
            rows: vec![],
            labels: Default::default(),
            notices: vec![],
        };
        for (kind, value) in [
            ("rich_text", json!([{"type":"mention","mention":{}}])),
            (
                "rich_text",
                json!([{"type":"equation","equation":{"expression":"x"}}]),
            ),
            ("rich_text", json!([{"type":"future"}])),
            ("formula", json!({"type":"number","number":1})),
            ("people", json!([])),
        ] {
            let mut row = t.row.clone();
            row.page["properties"]["P"] = json!({"id":"g%3FNq","type":kind,kind:value});
            assert!(CellTarget::new(&data, &row, t.column.clone()).is_err());
        }
    }

    #[test]
    fn scalar_values_validate_and_clear() {
        for (kind, initial) in [
            ("number", json!(4)),
            ("url", json!("https://example.com")),
            ("email", json!("a@example.com")),
            ("phone_number", json!("123")),
        ] {
            let mut d = CellDraft::new(&target(kind, initial));
            assert!(!d.changed());
            d.fields[0] = Buffer::default();
            assert!(d.changed());
            assert_eq!(d.value().unwrap(), Value::Null);
        }
        let mut d = CellDraft::new(&target("number", json!(0)));
        for invalid in ["NaN", "Infinity", "oops", "1 2"] {
            d.fields[0] = Buffer::new(invalid.into());
            assert!(d.value().is_err());
        }
        d.fields[0] = Buffer::new("-1.5e2".into());
        assert_eq!(d.value().unwrap(), json!(-150.0));
        let mut checkbox = CellDraft::new(&target("checkbox", json!(true)));
        checkbox.key(KeyCode::Char(' ').into());
        assert_eq!(checkbox.value().unwrap(), json!(false));
    }

    #[test]
    fn choice_pickers_use_existing_ids_and_support_clearing() {
        for kind in ["select", "status"] {
            let mut d = CellDraft::new(&target(kind, json!({"id":"a","name":"Alpha"})));
            assert!(!d.changed());
            let cursor = d.cursor;
            for key in ['h', 'j', 'k', 'l'] {
                d.key(KeyCode::Char(key).into());
                assert_eq!(d.cursor, cursor);
            }
            d.key(KeyCode::Down.into());
            assert!(!d.changed());
            d.key(KeyCode::Char('k').into());
            assert_eq!(d.cursor, cursor + 1);
            d.key(KeyCode::Enter.into());
            assert_eq!(d.value().unwrap(), json!({"id":"b"}));
            d.key(KeyCode::Up.into());
            d.key(KeyCode::Up.into());
            d.key(KeyCode::Char(' ').into());
            assert_eq!(d.value().unwrap(), Value::Null);
        }
        let mut d = CellDraft::new(&target("multi_select", json!([{"id":"a","name":"Alpha"}])));
        d.key(KeyCode::Down.into());
        d.key(KeyCode::Enter.into());
        assert_eq!(d.value().unwrap(), json!([{"id":"a"},{"id":"b"}]));
        d.key(KeyCode::Enter.into());
        d.key(KeyCode::Up.into());
        d.key(KeyCode::Enter.into());
        assert_eq!(d.value().unwrap(), json!([]));
    }

    #[test]
    fn dates_validate_offsets_ranges_timezone_and_explicit_clear() {
        let mut d = CellDraft::new(&target(
            "date",
            json!({"start":"2026-11-13","end":null,"time_zone":null}),
        ));
        assert!(!d.changed());
        assert_eq!(d.value().unwrap()["start"], "2026-11-13");
        d.key(KeyCode::Tab.into());
        d.fields[1] = Buffer::new("2026-11-12".into());
        assert!(d.value().is_err());
        d.fields[0] = Buffer::new("2026-11-13T08:00:00+08:00".into());
        d.fields[1] = Buffer::new("2026-11-13T00:00:00.001Z".into());
        assert!(d.value().is_ok());
        d.fields[1] = Buffer::new("2026-11-12T23:59:59Z".into());
        assert!(d.value().is_err());
        d.fields[2] = Buffer::new("Asia/Taipei".into());
        assert!(d.value().is_err());
        d.fields[0] = Buffer::new("2026-11-13T08:00:00".into());
        d.fields[1] = Buffer::default();
        assert_eq!(d.value().unwrap()["time_zone"], "Asia/Taipei");
        d.key(KeyCode::BackTab.into());
        assert_eq!(d.active, 0);
        d.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(d.value().unwrap(), Value::Null);
        d.fields[0] = Buffer::new("2026-02-30".into());
        assert!(d.value().is_err());
    }
}
