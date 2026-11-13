use crate::rich_text::{RichRun, external_url};
use rusticnotion::models::{
    requests::{RichTextContent, RichTextInput, TextInput},
    text::{Annotations, Text},
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct TableCell {
    pub text: String,
    pub styled: bool,
    // None means a mention, equation, or unrecognized run cannot safely round-trip.
    pub input: Option<Vec<RichTextInput>>,
}

impl TableCell {
    fn from_value(value: &Value) -> Self {
        let runs = value.as_array();
        let input = runs.and_then(|runs| {
            runs.iter()
                .map(|run| {
                    if run["type"] != "text" {
                        return None;
                    }
                    let text: Text = serde_json::from_value(run["text"].clone()).ok()?;
                    let annotations: Option<Annotations> =
                        serde_json::from_value(run["annotations"].clone()).ok()?;
                    Some(RichTextInput {
                        content: RichTextContent::Text {
                            text: TextInput {
                                content: text.content,
                                link: text.link,
                            },
                        },
                        annotations,
                    })
                })
                .collect()
        });
        Self {
            text: runs
                .map(|runs| {
                    runs.iter()
                        .map(|run| RichRun::from_value(run).text)
                        .collect()
                })
                .unwrap_or_default(),
            styled: runs.is_some_and(|runs| runs.iter().any(styled_run)),
            input,
        }
    }
}

fn styled_run(run: &Value) -> bool {
    !run["text"]["link"].is_null()
        || !run["href"].is_null()
        || run["annotations"].as_object().is_some_and(|annotations| {
            annotations.iter().any(|(key, value)| {
                if key == "color" {
                    value.as_str().is_some_and(|color| color != "default")
                } else {
                    value.as_bool() == Some(true)
                }
            })
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageItem {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Bullet,
    Number,
    Todo,
}

impl Kind {
    pub const ALL: [Self; 7] = [
        Self::Paragraph,
        Self::Heading1,
        Self::Heading2,
        Self::Heading3,
        Self::Bullet,
        Self::Number,
        Self::Todo,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::Heading1 => "heading_1",
            Self::Heading2 => "heading_2",
            Self::Heading3 => "heading_3",
            Self::Bullet => "bulleted_list_item",
            Self::Number => "numbered_list_item",
            Self::Todo => "to_do",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BlockItem {
    pub id: String,
    pub modified: String,
    pub block_type: String,
    pub text: String,
    pub rich_text: Vec<RichRun>,
    pub table_cells: Option<Vec<TableCell>>,
    pub children: bool,
    pub checked: bool,
    pub kind: Option<Kind>,
    pub styled: bool,
    pub protected: bool,
    pub child_page: bool,
}

impl BlockItem {
    pub fn from_value(value: &Value) -> Self {
        let block_type = value["type"].as_str().unwrap_or("unsupported").to_owned();
        let body = &value[&block_type];
        let runs = body["rich_text"].as_array();
        let mut rich_text: Vec<RichRun> = runs
            .map(|runs| runs.iter().map(RichRun::from_value).collect())
            .unwrap_or_default();
        let mut text = runs
            .map(|runs| {
                runs.iter()
                    .map(|r| {
                        r["plain_text"]
                            .as_str()
                            .or_else(|| r["text"]["content"].as_str())
                            .or_else(|| r["equation"]["expression"].as_str())
                            .unwrap_or("[mention]")
                    })
                    .collect::<String>()
            })
            .unwrap_or_default();
        if text.is_empty() {
            text = body["title"]
                .as_str()
                .or_else(|| body["url"].as_str())
                .or_else(|| body["expression"].as_str())
                .unwrap_or("")
                .to_owned();
        }
        if block_type == "table_row" {
            rich_text.clear();
            if let Some(cells) = body["cells"].as_array() {
                for (index, cell) in cells.iter().enumerate() {
                    if index > 0 {
                        rich_text.push(RichRun::plain(" │ "));
                    }
                    if let Some(runs) = cell.as_array() {
                        rich_text.extend(runs.iter().map(RichRun::from_value));
                    }
                }
            }
            text = rich_text.iter().map(|run| run.text.as_str()).collect();
        }
        if rich_text.is_empty()
            || rich_text
                .iter()
                .map(|run| run.text.as_str())
                .collect::<String>()
                != text
        {
            let mut run = RichRun::plain(text.clone());
            run.url = body["url"].as_str().and_then(external_url);
            rich_text = vec![run];
        }
        let table_cells = (block_type == "table_row")
            .then(|| {
                body["cells"]
                    .as_array()
                    .map(|cells| cells.iter().map(TableCell::from_value).collect::<Vec<_>>())
            })
            .flatten();
        let styled = runs.is_some_and(|runs| runs.iter().any(styled_run))
            || table_cells
                .as_ref()
                .is_some_and(|cells| cells.iter().any(|cell| cell.styled));
        let protected = runs.is_some_and(|runs| runs.iter().any(|r| r["type"] != "text"))
            || table_cells
                .as_ref()
                .is_some_and(|cells| cells.iter().any(|cell| cell.input.is_none()));
        Self {
            id: value["id"].as_str().unwrap_or_default().to_owned(),
            modified: value["last_edited_time"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            kind: Kind::ALL.into_iter().find(|k| k.name() == block_type),
            children: value["has_children"].as_bool().unwrap_or(false),
            checked: body["checked"].as_bool().unwrap_or(false),
            child_page: block_type == "child_page",
            block_type,
            text,
            rich_text,
            table_cells,
            styled,
            protected,
        }
    }

    pub fn editable_table_row(&self) -> bool {
        self.block_type == "table_row"
            && !self.protected
            && self
                .table_cells
                .as_ref()
                .is_some_and(|cells| !cells.is_empty())
    }

    pub fn label(&self, number: usize, depth: usize) -> BlockLabel {
        let marker = match self.block_type.as_str() {
            "numbered_list_item" => format!("{}. ", ordered_marker(number, depth)),
            "heading_1" => "# ".into(),
            "heading_2" => "## ".into(),
            "heading_3" => "### ".into(),
            "bulleted_list_item" => "• ".into(),
            "to_do" if self.checked => "[x] ".into(),
            "to_do" => "[ ] ".into(),
            "quote" => "│ ".into(),
            "code" => "⌘ ".into(),
            "child_page" => "▣ ".into(),
            "divider" => "────────".into(),
            "toggle" => "▸ ".into(),
            _ => String::new(),
        };
        let text = if self.text.is_empty() {
            format!("[{}]", self.block_type)
        } else {
            self.text.clone()
        };
        let expandable = self.children || self.block_type == "child_database";
        BlockLabel {
            prefix: format!("{}{marker}", "  ".repeat(depth)),
            text: format!("{text}{}", if expandable { "  ›" } else { "" }),
            rich_text: {
                let mut runs = if !self.text.is_empty()
                    && self
                        .rich_text
                        .iter()
                        .map(|run| run.text.as_str())
                        .collect::<String>()
                        == self.text
                {
                    self.rich_text.clone()
                } else {
                    vec![RichRun::plain(text)]
                };
                if expandable {
                    runs.push(RichRun::plain("  ›"));
                }
                runs
            },
        }
    }

    pub fn external_links(&self) -> Vec<(String, String)> {
        crate::rich_text::external_links(&self.rich_text)
    }
}

#[derive(Clone, Debug)]
pub struct BlockLabel {
    pub prefix: String,
    pub text: String,
    pub rich_text: Vec<RichRun>,
}

impl BlockLabel {
    pub fn plain(&self) -> String {
        format!("{}{}", self.prefix, self.text)
    }
}

fn ordered_marker(mut number: usize, depth: usize) -> String {
    match depth % 3 {
        0 => number.to_string(),
        1 => {
            let mut letters = Vec::new();
            while number > 0 {
                number -= 1;
                letters.push((b'a' + (number % 26) as u8) as char);
                number /= 26;
            }
            letters.into_iter().rev().collect()
        }
        _ => {
            let mut roman = String::new();
            for (value, digits) in [
                (1000, "m"),
                (900, "cm"),
                (500, "d"),
                (400, "cd"),
                (100, "c"),
                (90, "xc"),
                (50, "l"),
                (40, "xl"),
                (10, "x"),
                (9, "ix"),
                (5, "v"),
                (4, "iv"),
                (1, "i"),
            ] {
                while number >= value {
                    roman.push_str(digits);
                    number -= value;
                }
            }
            roman
        }
    }
}

/// Number the complete sibling sequence before viewport clipping or wrapping.
pub fn block_labels(blocks: &[BlockItem], depth: usize) -> Vec<BlockLabel> {
    let mut number = 0;
    blocks
        .iter()
        .map(|block| {
            if block.kind == Some(Kind::Number) {
                number += 1;
            } else {
                number = 0;
            }
            block.label(number, depth)
        })
        .collect()
}

pub fn rich_text(text: &str) -> Value {
    // Notion limits each rich-text content string to 2,000 characters.
    let chars: Vec<char> = text.chars().collect();
    Value::Array(
        chars
            .chunks(2000)
            .map(|chunk| {
                json!({
                    "type": "text", "text": {"content": chunk.iter().collect::<String>()}
                })
            })
            .collect(),
    )
}

pub fn update_payload(block: &BlockItem, text: Option<&str>) -> Value {
    let mut fields = json!({});
    if let Some(text) = text {
        fields["rich_text"] = rich_text(text);
    }
    if block.kind == Some(Kind::Todo) && text.is_none() {
        fields["checked"] = json!(!block.checked);
    }
    json!({block.block_type.clone(): fields})
}

pub fn table_row_payload(block: &BlockItem, texts: &[String]) -> Result<Value, &'static str> {
    if !block.editable_table_row() {
        return Err("This table row contains unsupported content or has no cells");
    }
    let cells = block.table_cells.as_ref().unwrap();
    if cells.len() != texts.len() {
        return Err("Table row cell count changed; cancel and refresh before editing again");
    }
    if texts.iter().map(|text| text.chars().count()).sum::<usize>() > 100_000 {
        return Err("Text is limited to 100,000 characters per block");
    }
    let cells = cells
        .iter()
        .zip(texts)
        .map(|(cell, text)| {
            if cell.text == *text {
                serde_json::to_value(cell.input.as_ref().unwrap())
                    .map_err(|_| "Could not preserve original cell formatting")
            } else {
                Ok(rich_text(text))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"table_row":{"cells":cells}}))
}

pub fn append_payload(kind: Kind, text: &str, after: Option<&str>) -> Value {
    let mut fields = json!({"rich_text": rich_text(text)});
    if kind == Kind::Todo {
        fields["checked"] = json!(false);
    }
    let mut payload =
        json!({"children": [{"object": "block", "type": kind.name(), kind.name(): fields}]});
    if let Some(after) = after {
        payload["position"] = json!({"type":"after_block","after_block":{"id":after}});
    }
    payload
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn paragraph() -> BlockItem {
        BlockItem::from_value(
            &json!({"id":"block", "type":"paragraph", "last_edited_time":"before",
            "paragraph":{"rich_text":[{"type":"text","plain_text":"Hello","text":{"content":"Hello"}}]}}),
        )
    }

    pub fn table_row() -> BlockItem {
        BlockItem::from_value(
            &json!({"id":"block", "type":"table_row", "last_edited_time":"2026-01-01T00:00:00Z",
            "table_row":{"cells":[
                [{"type":"text","plain_text":"left │ side\n你好","text":{"content":"left │ side\n你好"}}],
                [{"type":"text","plain_text":"Styled","href":"https://example.com/","text":{"content":"Styled","link":{"url":"https://example.com/"}},"annotations":{"bold":true,"italic":true,"underline":true,"strikethrough":true,"code":true,"color":"red"}},
                 {"type":"text","plain_text":" tail","text":{"content":" tail"}}],
                []
            ]}}),
        )
    }

    #[test]
    fn table_rows_keep_cell_boundaries_and_only_write_input_fields() {
        let row = table_row();
        let cells = row.table_cells.as_ref().unwrap();
        assert!(row.editable_table_row());
        assert!(row.styled);
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].text, "left │ side\n你好");
        assert_eq!(row.text, "left │ side\n你好 │ Styled tail │ ");
        assert!(row.rich_text.iter().any(|run| run.url.is_some()));
        let mut texts = cells
            .iter()
            .map(|cell| cell.text.clone())
            .collect::<Vec<_>>();
        texts[0] = "new\n🦀 │ text".into();
        let payload = table_row_payload(&row, &texts).unwrap();
        assert_eq!(
            payload,
            json!({"table_row":{"cells":[
                [{"type":"text","text":{"content":"new\n🦀 │ text"}}],
                [{"type":"text","text":{"content":"Styled","link":{"url":"https://example.com/"}},"annotations":{"bold":true,"italic":true,"underline":true,"strikethrough":true,"code":true,"color":"red"}},
                 {"type":"text","text":{"content":" tail"}}], []
            ]}})
        );
        texts[1] = "plain".into();
        assert_eq!(
            table_row_payload(&row, &texts).unwrap()["table_row"]["cells"][1],
            rich_text("plain")
        );
        texts.pop();
        assert!(table_row_payload(&row, &texts).is_err());
    }

    #[test]
    fn unsupported_or_malformed_table_rows_are_read_only() {
        for cells in [
            json!([]),
            json!(null),
            json!([null]),
            json!([[{"type":"mention","plain_text":"@person","mention":{"type":"user"}}]]),
            json!([[{"type":"equation","plain_text":"x","equation":{"expression":"x"}}]]),
            json!([[{"type":"future","plain_text":"future"}]]),
            json!([[{"type":"text","plain_text":"missing content"}]]),
        ] {
            let row =
                BlockItem::from_value(&json!({"type":"table_row","table_row":{"cells":cells}}));
            assert!(!row.editable_table_row());
            assert!(table_row_payload(&row, &[]).is_err());
        }
    }

    #[test]
    fn table_row_chunks_unicode_and_preserves_empty_cells() {
        let row = table_row();
        let texts = vec!["🦀".repeat(4001), "".into(), "".into()];
        let payload = table_row_payload(&row, &texts).unwrap();
        let chunks = payload["table_row"]["cells"][0].as_array().unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks[0]["text"]["content"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            2000
        );
        assert_eq!(payload["table_row"]["cells"][1], json!([]));
        assert_eq!(payload["table_row"]["cells"][2], json!([]));
        assert!(table_row_payload(&row, &["x".repeat(100_001), "".into(), "".into()]).is_err());
    }

    #[test]
    fn child_databases_show_the_child_indicator_without_block_children() {
        for has_children in [None, Some(false), Some(true)] {
            let mut value = json!({
                "id": "db", "type": "child_database",
                "child_database": {"title": "Tasks"}
            });
            if let Some(has_children) = has_children {
                value["has_children"] = json!(has_children);
            }
            let label = BlockItem::from_value(&value).label(1, 0);
            assert_eq!(label.plain(), "Tasks  ›");
            assert_eq!(
                label
                    .rich_text
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<String>(),
                "Tasks  ›"
            );
        }
        assert_eq!(paragraph().label(1, 0).plain(), "Hello");
    }

    #[test]
    fn ordered_siblings_advance_and_restart_after_other_blocks() {
        let mut ordered = paragraph();
        ordered.kind = Some(Kind::Number);
        ordered.block_type = "numbered_list_item".into();
        let blocks = vec![
            ordered.clone(),
            ordered.clone(),
            ordered.clone(),
            paragraph(),
            ordered,
        ];
        let labels = block_labels(&blocks, 0);
        assert_eq!(
            labels.iter().map(|label| label.plain()).collect::<Vec<_>>(),
            ["1. Hello", "2. Hello", "3. Hello", "Hello", "1. Hello"]
        );
    }

    #[test]
    fn nested_markers_cycle_decimal_letters_and_roman() {
        let mut ordered = paragraph();
        ordered.kind = Some(Kind::Number);
        ordered.block_type = "numbered_list_item".into();
        for (depth, expected) in [
            (0, "1. Hello"),
            (1, "  a. Hello"),
            (2, "    i. Hello"),
            (3, "      1. Hello"),
        ] {
            assert_eq!(block_labels(&[ordered.clone()], depth)[0].plain(), expected);
        }
        for (number, expected) in [(26, "z"), (27, "aa"), (52, "az"), (53, "ba")] {
            assert_eq!(ordered_marker(number, 1), expected);
        }
        for (number, expected) in [(3, "iii"), (4, "iv"), (9, "ix"), (14, "xiv"), (49, "xlix")] {
            assert_eq!(ordered_marker(number, 2), expected);
        }
    }

    #[test]
    fn payloads_never_replace_children_or_send_response_fields() {
        let block = paragraph();
        assert_eq!(
            update_payload(&block, Some("new")),
            json!({"paragraph":{"rich_text":[
            {"type":"text","text":{"content":"new"}}]}})
        );
        let mut todo = block;
        todo.block_type = "to_do".into();
        todo.kind = Some(Kind::Todo);
        assert_eq!(
            update_payload(&todo, None),
            json!({"to_do":{"checked":true}})
        );
        assert_eq!(
            append_payload(Kind::Todo, "task", None)["children"][0]["to_do"]["checked"],
            false
        );
    }

    #[test]
    fn unicode_chunks_and_rich_content_safety() {
        let text = "界".repeat(2001);
        let runs = rich_text(&text);
        assert_eq!(runs.as_array().unwrap().len(), 2);
        assert_eq!(
            runs[0]["text"]["content"].as_str().unwrap().chars().count(),
            2000
        );
        let b = BlockItem::from_value(&json!({"type":"paragraph","paragraph":{"rich_text":[
            {"type":"mention","plain_text":"person","annotations":{"bold":true}}
        ]}}));
        assert!(b.protected && b.styled);
    }
}
