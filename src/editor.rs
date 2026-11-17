use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    comments::{CommentTarget, CommentWrite, validate_text},
    model::{BlockItem, InsertPosition, Kind},
    service::Write,
};

pub mod database;
pub use database::{CellDraft, CellTarget};
pub mod rich;
use rich::{RichDraft, StylePicker};

#[derive(Clone, Debug)]
pub enum Target {
    Comment(CommentTarget),
    Edit(BlockItem),
    DatabaseCell(Box<CellTarget>),
    Append {
        parent: String,
        position: InsertPosition,
    },
    Create(String),
}

#[derive(Clone, Default, Debug)]
pub struct Buffer {
    pub text: String,
    pub cursor: usize,
    pub rich: Option<RichDraft>,
    anchor: Option<usize>,
}

impl Buffer {
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
            ..Self::default()
        }
    }

    pub fn insert(&mut self, text: &str) {
        // Strip terminal control characters, preserving newlines and expanding tabs.
        let clean: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        if clean.is_empty() {
            return;
        }
        self.replace(self.selection().unwrap_or(self.cursor..self.cursor), &clean);
    }

    pub fn position(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        (
            before.chars().filter(|c| *c == '\n').count(),
            UnicodeWidthStr::width(before.rsplit('\n').next().unwrap_or("")),
        )
    }

    fn vertical(&mut self, down: bool) {
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let col = self.text[start..self.cursor].graphemes(true).count();
        let target = if down {
            match self.text[self.cursor..].find('\n') {
                Some(n) => self.cursor + n + 1,
                None => return,
            }
        } else {
            if start == 0 {
                return;
            }
            self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1)
        };
        let line = self.text[target..].split('\n').next().unwrap_or("");
        self.cursor = target
            + line
                .grapheme_indices(true)
                .nth(col)
                .map_or(line.len(), |(i, _)| i);
    }

    pub fn key(&mut self, key: KeyEvent, multiline: bool) {
        // Some terminals send BS (Ctrl-H) instead of DEL for Backspace.
        // Crossterm reports BS as Ctrl-H, so normalize it for all text fields.
        let code = match key.code {
            KeyCode::Char('h') if key.modifiers == KeyModifiers::CONTROL => KeyCode::Backspace,
            code => code,
        };
        let movement = matches!(
            code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
        );
        if !key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(code, KeyCode::Left | KeyCode::Right)
            && let Some(range) = self.selection()
        {
            self.cursor = if code == KeyCode::Left {
                range.start
            } else {
                range.end
            };
            self.anchor = None;
            self.refresh_typing();
            return;
        }
        if movement && self.rich.is_some() {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.anchor.get_or_insert(self.cursor);
            } else {
                self.anchor = None;
            }
        }
        match code {
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&c.to_string())
            }
            KeyCode::Enter if multiline => self.insert("\n"),
            KeyCode::Tab if multiline => self.insert("    "),
            KeyCode::Left => {
                if let Some((i, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
                    self.cursor = i;
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.text[self.cursor..].graphemes(true).next() {
                    self.cursor += c.len();
                }
            }
            KeyCode::Home => {
                self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
            }
            KeyCode::End => {
                self.cursor += self.text[self.cursor..]
                    .find('\n')
                    .unwrap_or(self.text.len() - self.cursor)
            }
            KeyCode::Up => self.vertical(false),
            KeyCode::Down => self.vertical(true),
            KeyCode::Backspace => {
                if let Some(range) = self.selection() {
                    self.replace(range, "");
                } else if let Some((i, _)) =
                    self.text[..self.cursor].grapheme_indices(true).next_back()
                {
                    self.replace(i..self.cursor, "");
                }
            }
            KeyCode::Delete => {
                if let Some(range) = self.selection() {
                    self.replace(range, "");
                } else if let Some(c) = self.text[self.cursor..].graphemes(true).next() {
                    self.replace(self.cursor..self.cursor + c.len(), "");
                }
            }
            _ => {}
        }
        if movement {
            self.refresh_typing();
        }
    }
}

#[derive(Clone, Debug)]
pub struct Editor {
    pub target: Target,
    pub buffer: Buffer,
    pub row: Option<RowDraft>,
    pub cell: Option<CellDraft>,
    pub kind: Kind,
    pub style_picker: Option<StylePicker>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RowDraft {
    pub cells: Vec<Buffer>,
    pub active: usize,
}

impl Editor {
    pub fn new(target: Target) -> Self {
        let cell = match &target {
            Target::DatabaseCell(target) => Some(CellDraft::new(target)),
            _ => None,
        };
        let row = match &target {
            Target::Edit(block) if block.editable_table_row() => Some(RowDraft {
                cells: block
                    .table_cells
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|cell| Buffer::from_rich(cell.input.as_deref().unwrap_or_default()))
                    .collect(),
                active: 0,
            }),
            _ => None,
        };
        let (text, kind) = match &target {
            Target::Edit(b) => (b.text.clone(), b.kind.unwrap_or(Kind::Paragraph)),
            _ => (String::new(), Kind::Paragraph),
        };
        let buffer = match &target {
            Target::Comment(CommentTarget::Edit(original)) => Buffer::from_rich(
                original
                    .input
                    .as_ref()
                    .map_or(&[], |text| text.0.as_slice()),
            ),
            Target::Comment(_) => Buffer::from_rich(&[]),
            Target::Edit(block) if row.is_none() => Buffer::from_rich(
                &block
                    .input
                    .clone()
                    .unwrap_or_else(|| block.text.clone().into())
                    .0,
            ),
            Target::Append { .. } => Buffer::from_rich(&[]),
            _ => Buffer::new(text),
        };
        Self {
            target,
            buffer,
            row,
            cell,
            kind,
            style_picker: None,
            error: None,
        }
    }

    pub fn active_buffer(&self) -> &Buffer {
        if let Some(cell) = &self.cell {
            return &cell.fields[cell.active];
        }
        self.row
            .as_ref()
            .map_or(&self.buffer, |row| &row.cells[row.active])
    }

    fn active_buffer_mut(&mut self) -> &mut Buffer {
        if let Some(cell) = &mut self.cell {
            return &mut cell.fields[cell.active];
        }
        match &mut self.row {
            Some(row) => &mut row.cells[row.active],
            None => &mut self.buffer,
        }
    }

    pub fn insert(&mut self, text: &str) {
        if self.style_picker.is_some() {
            return;
        }
        if self.cell.as_ref().is_some_and(|cell| cell.is_picker()) {
            return;
        }
        let text = if matches!(self.target, Target::Create(_)) {
            text.replace(['\n', '\r'], " ")
        } else {
            text.to_owned()
        };
        self.active_buffer_mut().insert(&text);
    }

    pub fn key(&mut self, key: KeyEvent) {
        if let Some(picker) = &mut self.style_picker {
            match key.code {
                KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
                KeyCode::Down => picker.selected = (picker.selected + 1).min(3),
                KeyCode::Char(' ') => {
                    picker.checked[picker.selected] =
                        Some(picker.checked[picker.selected] != Some(true))
                }
                KeyCode::Enter => {
                    let styles = picker.checked;
                    self.style_picker = None;
                    self.active_buffer_mut().apply_styles(styles);
                }
                KeyCode::Esc => self.style_picker = None,
                _ => {}
            }
            return;
        }
        if self.active_buffer().rich.is_some() {
            if key.code == KeyCode::F(10) && key.modifiers.contains(KeyModifiers::SHIFT) {
                self.style_picker = Some(StylePicker {
                    selected: 0,
                    checked: self.active_buffer().style_states(),
                });
                return;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let index = match key.code {
                    KeyCode::Char('b' | 'B') => Some(0),
                    KeyCode::Char('i' | 'I') => Some(1),
                    KeyCode::Char('u' | 'U') => Some(2),
                    _ => None,
                };
                if let Some(index) = index {
                    self.active_buffer_mut().toggle_style(index);
                    return;
                }
            }
        }
        if let Some(cell) = &mut self.cell {
            cell.key(key);
            return;
        }
        if let Some(row) = &mut self.row {
            if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                let backwards =
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
                row.active = (row.active + if backwards { row.cells.len() - 1 } else { 1 })
                    % row.cells.len();
                return;
            }
        } else if key.code == KeyCode::Tab && !matches!(self.target, Target::Comment(_)) {
            self.cycle_kind();
            return;
        }
        let multiline = !matches!(self.target, Target::Create(_));
        self.active_buffer_mut().key(key, multiline);
    }

    pub fn character_count(&self) -> usize {
        if let Some(cell) = &self.cell {
            return cell.fields.iter().map(|b| b.text.chars().count()).sum();
        }
        self.row.as_ref().map_or_else(
            || self.buffer.text.chars().count(),
            |row| row.cells.iter().map(|cell| cell.text.chars().count()).sum(),
        )
    }

    pub fn changed(&self) -> bool {
        if let Some(cell) = &self.cell {
            return cell.changed();
        }
        match &self.target {
            Target::Comment(CommentTarget::Edit(_)) => self.buffer.rich_changed(),
            Target::Edit(block) => match &self.row {
                Some(row) => row
                    .cells
                    .iter()
                    .zip(block.table_cells.as_ref().unwrap())
                    .any(|(draft, original)| draft.text != original.text || draft.rich_changed()),
                None => self.buffer.text != block.text || self.buffer.rich_changed(),
            },
            _ => true,
        }
    }

    pub fn cycle_kind(&mut self) {
        if matches!(self.target, Target::Append { .. }) {
            let index = Kind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
            self.kind = Kind::ALL[(index + 1) % Kind::ALL.len()];
        }
    }

    pub fn write(&self) -> Result<Write, &'static str> {
        if self.character_count() > 100_000 {
            return Err("Text is limited to 100,000 characters per block");
        }
        Ok(match &self.target {
            Target::Comment(_) => return Err("Use the comment save action"),
            Target::DatabaseCell(original) => Write::UpdateDatabaseCell {
                original: original.clone(),
                value: self.cell.as_ref().unwrap().value()?,
            },
            Target::Edit(original) => match &self.row {
                Some(row) => Write::UpdateTableRow {
                    original: original.clone(),
                    cells: row.cells.iter().map(Buffer::rich_text).collect(),
                },
                None => Write::Update {
                    original: original.clone(),
                    text: Some(self.buffer.rich_text()),
                },
            },
            Target::Append { parent, position } => Write::Append {
                parent: parent.clone(),
                position: position.clone(),
                kind: self.kind,
                text: self.buffer.rich_text(),
            },
            Target::Create(parent) => {
                if self.buffer.text.trim().is_empty() {
                    return Err("A page title is required");
                }
                if self.buffer.text.chars().count() > 2000 {
                    return Err("Page titles are limited to 2,000 characters");
                }
                Write::Create {
                    parent: parent.clone(),
                    title: self.buffer.text.clone(),
                }
            }
        })
    }

    pub fn comment_write(&self) -> Result<CommentWrite, &'static str> {
        let Target::Comment(target) = &self.target else {
            return Err("No comment draft to save");
        };
        let text = self.buffer.rich_text();
        validate_text(&text)?;
        Ok(CommentWrite::Save {
            target: target.clone(),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_row_editing_switches_cells_and_preserves_cursors() {
        let mut editor = Editor::new(Target::Edit(crate::model::tests::table_row()));
        assert!(!editor.changed());
        editor.key(KeyCode::Tab.into());
        assert_eq!(editor.row.as_ref().unwrap().active, 1);
        editor.key(KeyCode::Home.into());
        editor.key(KeyCode::Right.into());
        let cursor = editor.active_buffer().cursor;
        editor.key(KeyCode::BackTab.into());
        editor.insert("\r\n🦀\t\x1b");
        assert!(editor.active_buffer().text.ends_with("\n🦀    "));
        editor.key(KeyCode::Tab.into());
        assert_eq!(editor.active_buffer().cursor, cursor);
        editor.key(KeyCode::Enter.into());
        editor.key(KeyCode::Left.into());
        editor.key(KeyCode::Tab.into());
        editor.key(KeyCode::BackTab.into());
        editor.key(KeyCode::Backspace.into());
        editor.key(KeyCode::BackTab.into());
        editor.key(KeyCode::BackTab.into());
        assert_eq!(editor.row.as_ref().unwrap().active, 2);
        editor.insert("empty no longer");
        let Write::UpdateTableRow { cells, .. } = editor.write().unwrap() else {
            panic!("wrong write");
        };
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[2].plain(), "empty no longer");
    }

    #[test]
    fn table_row_character_limit_counts_all_cells() {
        let mut editor = Editor::new(Target::Edit(crate::model::tests::table_row()));
        let row = editor.row.as_mut().unwrap();
        row.cells = vec![
            Buffer::new("🦀".repeat(50_000)),
            Buffer::new("你".repeat(50_000)),
            Buffer::default(),
        ];
        assert_eq!(editor.character_count(), 100_000);
        assert!(editor.write().is_ok());
        editor.insert("a");
        assert!(editor.write().is_err());
    }
    #[test]
    fn editing_unicode_and_multiline_maintains_byte_boundaries() {
        let mut b = Buffer::new("你好\ncafé".into());
        b.key(KeyCode::Left.into(), true);
        b.key(KeyCode::Backspace.into(), true);
        assert_eq!(b.text, "你好\ncaé");
        b.key(KeyCode::Up.into(), true);
        b.insert("🦀");
        assert_eq!(b.text, "你好🦀\ncaé");
        assert_eq!(b.position(), (0, 6));
        b.key(KeyCode::Down.into(), true);
        assert!(b.text.is_char_boundary(b.cursor));
    }
}
