use crate::rich_text::{RichRun, external_url};
use serde_json::{Value, json};

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
        let styled = runs.is_some_and(|runs| {
            runs.iter().any(|r| {
                !r["text"]["link"].is_null()
                    || !r["href"].is_null()
                    || r["annotations"].as_object().is_some_and(|a| {
                        a.iter().any(|(k, v)| {
                            if k == "color" {
                                v.as_str().is_some_and(|s| s != "default")
                            } else {
                                v.as_bool() == Some(true)
                            }
                        })
                    })
            })
        });
        let protected = runs.is_some_and(|runs| runs.iter().any(|r| r["type"] != "text"));
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
            styled,
            protected,
        }
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
        BlockLabel {
            prefix: format!("{}{marker}", "  ".repeat(depth)),
            text: format!("{text}{}", if self.children { "  ›" } else { "" }),
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
                if self.children {
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
