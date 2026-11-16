use std::ops::Range;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use rusticnotion::models::{
    requests::{RichTextContent, RichTextInput, TextInput},
    text::{Annotations, Link},
};
use unicode_segmentation::UnicodeSegmentation;

use super::Buffer;
use crate::model::RichText;

pub const STYLE_NAMES: [&str; 4] = ["Bold", "Italic", "Underline", "Strikethrough"];
pub const STYLE_MODIFIERS: [Modifier; 4] = [
    Modifier::BOLD,
    Modifier::ITALIC,
    Modifier::UNDERLINED,
    Modifier::CROSSED_OUT,
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attributes {
    pub annotations: Option<Annotations>,
    pub link: Option<Link>,
}

impl Attributes {
    fn flags(&self) -> [bool; 4] {
        self.annotations.as_ref().map_or([false; 4], |a| {
            [
                a.bold == Some(true),
                a.italic == Some(true),
                a.underline == Some(true),
                a.strikethrough == Some(true),
            ]
        })
    }

    fn set(&mut self, index: usize, enabled: bool) {
        let a = self.annotations.get_or_insert(Annotations {
            bold: None,
            italic: None,
            underline: None,
            strikethrough: None,
            code: None,
            color: None,
        });
        let field = match index {
            0 => &mut a.bold,
            1 => &mut a.italic,
            2 => &mut a.underline,
            _ => &mut a.strikethrough,
        };
        *field = Some(enabled);
    }

    fn style(&self) -> Style {
        self.flags()
            .iter()
            .zip(STYLE_MODIFIERS)
            .fold(Style::default(), |style, (on, modifier)| {
                if *on {
                    style.add_modifier(modifier)
                } else {
                    style
                }
            })
    }
}

#[derive(Clone, Debug)]
pub struct FormatSpan {
    pub range: Range<usize>,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, Default)]
pub struct RichDraft {
    pub spans: Vec<FormatSpan>,
    pub typing: Attributes,
    pub initial: RichText,
}

impl RichDraft {
    fn compact(&mut self) {
        let mut merged: Vec<FormatSpan> = vec![];
        for span in self.spans.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.range.end == span.range.start
                && last.attributes == span.attributes
            {
                last.range.end = span.range.end;
            } else {
                merged.push(span);
            }
        }
        self.spans = merged;
    }
}

#[derive(Clone, Debug)]
pub struct StylePicker {
    pub selected: usize,
    pub checked: [Option<bool>; 4],
}

impl Buffer {
    pub fn from_rich(input: &[RichTextInput]) -> Self {
        let mut buffer = Self::default();
        let mut draft = RichDraft::default();
        for run in input {
            if let RichTextContent::Text { text } = &run.content {
                let start = buffer.text.len();
                buffer.text.push_str(&text.content);
                if start != buffer.text.len() {
                    draft.spans.push(FormatSpan {
                        range: start..buffer.text.len(),
                        attributes: Attributes {
                            annotations: run.annotations.clone(),
                            link: text.link.clone(),
                        },
                    });
                }
            }
        }
        buffer.cursor = buffer.text.len();
        buffer.rich = Some(draft);
        buffer.refresh_typing();
        buffer.rich.as_mut().unwrap().initial = buffer.rich_text();
        buffer
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor
            .filter(|anchor| *anchor != self.cursor)
            .map(|anchor| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn refresh_typing(&mut self) {
        if let Some(draft) = &mut self.rich {
            draft.typing = draft
                .spans
                .iter()
                .find(|span| {
                    if self.cursor == 0 {
                        span.range.start == 0
                    } else {
                        span.range.start < self.cursor && self.cursor <= span.range.end
                    }
                })
                .map(|span| span.attributes.clone())
                .unwrap_or_default();
        }
    }

    pub fn rich_text(&self) -> RichText {
        let Some(draft) = &self.rich else {
            return self.text.clone().into();
        };
        // Plain buffers may still be replaced directly by callers preparing drafts.
        if draft.spans.last().map_or(0, |s| s.range.end) != self.text.len() {
            return self.text.clone().into();
        }
        let mut runs: Vec<RichTextInput> = vec![];
        for span in &draft.spans {
            let text = &self.text[span.range.clone()];
            if let Some(last) = runs.last_mut()
                && last.annotations == span.attributes.annotations
                && let RichTextContent::Text { text: previous } = &mut last.content
                && previous.link == span.attributes.link
            {
                previous.content.push_str(text);
                continue;
            }
            runs.push(RichTextInput {
                content: RichTextContent::Text {
                    text: TextInput {
                        content: text.into(),
                        link: span.attributes.link.clone(),
                    },
                },
                annotations: span.attributes.annotations.clone(),
            });
        }
        RichText(runs).chunked()
    }

    pub fn rich_changed(&self) -> bool {
        self.rich
            .as_ref()
            .is_some_and(|draft| self.rich_text().normalized() != draft.initial.normalized())
    }

    pub fn style_states(&self) -> [Option<bool>; 4] {
        let Some(draft) = &self.rich else {
            return [Some(false); 4];
        };
        let Some(range) = self.selection() else {
            return draft.typing.flags().map(Some);
        };
        let mut flags = draft
            .spans
            .iter()
            .filter(|s| s.range.start < range.end && range.start < s.range.end)
            .map(|s| s.attributes.flags());
        let Some(first) = flags.next() else {
            return [Some(false); 4];
        };
        let mut result = first.map(Some);
        for flags in flags {
            for i in 0..4 {
                if result[i] != Some(flags[i]) {
                    result[i] = None;
                }
            }
        }
        result
    }

    pub fn toggle_style(&mut self, index: usize) {
        let mut styles = [None; 4];
        styles[index] = Some(self.style_states()[index] != Some(true));
        self.apply_styles(styles);
    }

    pub fn apply_styles(&mut self, styles: [Option<bool>; 4]) {
        let selection = self.selection();
        let Some(draft) = &mut self.rich else { return };
        if let Some(range) = selection {
            let mut spans = vec![];
            for span in &draft.spans {
                if span.range.end <= range.start || span.range.start >= range.end {
                    spans.push(span.clone());
                    continue;
                }
                if span.range.start < range.start {
                    spans.push(FormatSpan {
                        range: span.range.start..range.start,
                        attributes: span.attributes.clone(),
                    });
                }
                let mut attributes = span.attributes.clone();
                for (i, value) in styles.iter().enumerate() {
                    if let Some(on) = value {
                        attributes.set(i, *on);
                    }
                }
                spans.push(FormatSpan {
                    range: span.range.start.max(range.start)..span.range.end.min(range.end),
                    attributes,
                });
                if span.range.end > range.end {
                    spans.push(FormatSpan {
                        range: range.end..span.range.end,
                        attributes: span.attributes.clone(),
                    });
                }
            }
            draft.spans = spans;
            draft.compact();
            draft.typing = draft
                .spans
                .iter()
                .find(|span| span.range.contains(&range.start))
                .map(|span| span.attributes.clone())
                .unwrap_or_default();
        } else {
            for (i, value) in styles.iter().enumerate() {
                if let Some(on) = value {
                    draft.typing.set(i, *on);
                }
            }
        }
    }

    pub(super) fn replace(&mut self, range: Range<usize>, text: &str) {
        if let Some(draft) = &mut self.rich {
            let mut spans = vec![];
            for span in &draft.spans {
                if span.range.start < range.start {
                    spans.push(FormatSpan {
                        range: span.range.start..span.range.end.min(range.start),
                        attributes: span.attributes.clone(),
                    });
                }
            }
            if !text.is_empty() {
                spans.push(FormatSpan {
                    range: range.start..range.start + text.len(),
                    attributes: draft.typing.clone(),
                });
            }
            for span in &draft.spans {
                if span.range.end > range.end {
                    let start = span.range.start.max(range.end);
                    spans.push(FormatSpan {
                        range: start - range.len() + text.len()
                            ..span.range.end - range.len() + text.len(),
                        attributes: span.attributes.clone(),
                    });
                }
            }
            draft.spans = spans;
            draft.compact();
        }
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = None;
    }

    pub fn styled_text(&self) -> Text<'static> {
        let Some(draft) = &self.rich else {
            return Text::raw(self.text.clone());
        };
        let selection = self.selection();
        let mut lines = vec![];
        let mut line: Vec<Span<'static>> = vec![];
        let mut spans = draft.spans.iter().peekable();
        for (offset, grapheme) in self.text.grapheme_indices(true) {
            if grapheme == "\n" {
                lines.push(Line::from(std::mem::take(&mut line)));
                continue;
            }
            while spans.peek().is_some_and(|span| span.range.end <= offset) {
                spans.next();
            }
            let mut style = spans
                .peek()
                .filter(|span| span.range.contains(&offset))
                .map_or(Style::default(), |s| s.attributes.style());
            if selection
                .as_ref()
                .is_some_and(|range| range.contains(&offset))
            {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if let Some(last) = line.last_mut()
                && last.style == style
            {
                last.content.to_mut().push_str(grapheme);
            } else {
                line.push(Span::styled(grapheme.to_owned(), style));
            }
        }
        lines.push(Line::from(line));
        Text::from(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        editor::{Editor, Target},
        model::{InsertPosition, TableCell},
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;

    fn rich(value: serde_json::Value) -> Buffer {
        Buffer::from_rich(TableCell::from_value(&value).input.as_deref().unwrap())
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn editor() -> Editor {
        Editor::new(Target::Append {
            parent: "page".into(),
            position: InsertPosition::End,
        })
    }

    #[test]
    fn range_formatting_preserves_links_code_color_and_other_runs() {
        let mut b = rich(json!([
            {"type":"text","text":{"content":"plain "}},
            {"type":"text","text":{"content":"界🦀","link":{"url":"https://example.com/"}},"annotations":{"bold":true,"code":true,"color":"blue"}}
        ]));
        b.key(shift(KeyCode::Left), true);
        b.toggle_style(1);
        let value = json!(b.rich_text());
        assert_eq!(value[0]["text"]["content"], "plain ");
        assert_eq!(value[1]["text"]["content"], "界");
        assert_eq!(value[2]["text"]["content"], "🦀");
        assert_eq!(value[2]["annotations"]["bold"], true);
        assert_eq!(value[2]["annotations"]["italic"], true);
        assert_eq!(value[2]["annotations"]["code"], true);
        assert_eq!(value[2]["annotations"]["color"], "blue");
        assert_eq!(value[2]["text"]["link"]["url"], "https://example.com/");
        let reopened = Buffer::from_rich(&b.rich_text().0);
        assert_eq!(json!(reopened.rich_text()), value);
        assert!(!reopened.rich_changed());
    }

    #[test]
    fn mixed_selection_toggles_uniformly_and_noop_formatting_is_unchanged() {
        let mut b = rich(json!([
            {"type":"text","text":{"content":"a"},"annotations":{"bold":true}},
            {"type":"text","text":{"content":"b"}}
        ]));
        b.key(shift(KeyCode::Home), true);
        assert_eq!(b.style_states()[0], None);
        b.toggle_style(0);
        assert_eq!(b.style_states()[0], Some(true));
        b.toggle_style(0);
        assert_eq!(b.style_states()[0], Some(false));
        let mut plain = Buffer::from_rich(&RichText::from("abc").0);
        plain.key(shift(KeyCode::Home), true);
        plain.toggle_style(0);
        assert!(plain.rich_changed());
        plain.toggle_style(0);
        assert!(!plain.rich_changed());
        plain.key(KeyCode::Right.into(), true);
        assert_eq!(plain.cursor, 3);
        assert!(plain.selection().is_none());
    }

    #[test]
    fn typing_toggles_combine_and_moves_follow_surrounding_formatting() {
        let mut e = editor();
        for key in ['b', 'i', 'u'] {
            e.key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL));
        }
        assert!(!e.buffer.rich_changed());
        e.insert("styled");
        let value = json!(e.buffer.rich_text());
        for name in ["bold", "italic", "underline"] {
            assert_eq!(value[0]["annotations"][name], true);
        }
        e.buffer.toggle_style(3);
        e.insert("strike");
        assert_eq!(
            json!(e.buffer.rich_text())[1]["annotations"]["strikethrough"],
            true
        );
        e.key(KeyCode::Home.into());
        assert_eq!(e.buffer.style_states()[3], Some(false));
        e.key(KeyCode::End.into());
        assert_eq!(e.buffer.style_states()[3], Some(true));
    }

    #[test]
    fn selection_replaces_and_deletes_whole_graphemes_across_lines() {
        let mut b = Buffer::from_rich(&RichText::from("a👨‍👩‍👧‍👦e\u{301}\n界z").0);
        b.key(shift(KeyCode::Home), true);
        assert_eq!(&b.text[b.selection().unwrap()], "界z");
        b.insert("🦀\r\n\t\x1b");
        assert_eq!(b.text, "a👨‍👩‍👧‍👦e\u{301}\n🦀\n    ");
        b.key(KeyCode::Home.into(), true);
        b.key(KeyCode::Backspace.into(), true);
        b.key(KeyCode::Home.into(), true);
        b.key(KeyCode::Backspace.into(), true);
        b.key(KeyCode::Backspace.into(), true);
        assert_eq!(b.text, "a👨‍👩‍👧‍👦🦀    ");
        b.key(shift(KeyCode::Home), true);
        b.key(KeyCode::Delete.into(), true);
        assert_eq!(b.text, "🦀    ");
        assert_eq!(b.cursor, 0);
        assert_eq!(b.rich_text().plain(), b.text);
    }

    #[test]
    fn style_picker_stages_changes_and_legacy_tab_keeps_block_type_switching() {
        let mut e = editor();
        e.insert("abc");
        e.key(shift(KeyCode::Home));
        e.key(shift(KeyCode::F(10)));
        e.key(KeyCode::Char(' ').into());
        e.insert("ignored");
        assert_eq!(e.buffer.text, "abc");
        e.key(KeyCode::Esc.into());
        assert!(
            !e.buffer.rich_text().0[0]
                .annotations
                .as_ref()
                .is_some_and(|a| a.bold == Some(true))
        );
        e.key(shift(KeyCode::F(10)));
        for _ in 0..3 {
            e.key(KeyCode::Down.into());
        }
        e.key(KeyCode::Char(' ').into());
        e.key(KeyCode::Enter.into());
        assert_eq!(
            json!(e.buffer.rich_text())[0]["annotations"]["strikethrough"],
            true
        );
        e.key(KeyCode::Tab.into());
        assert_eq!(e.kind, crate::model::Kind::Heading1);
    }

    #[test]
    fn styled_long_runs_are_chunked_without_losing_metadata() {
        let mut b = Buffer::from_rich(&[]);
        b.toggle_style(0);
        b.insert(&"🦀".repeat(4001));
        let result = b.rich_text();
        assert_eq!(result.0.len(), 3);
        assert_eq!(result.plain(), b.text);
        for run in result.0 {
            assert_eq!(run.annotations.unwrap().bold, Some(true));
            let RichTextContent::Text { text } = run.content else {
                panic!()
            };
            assert!(text.content.chars().count() <= 2000);
        }
    }
}
