use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

use crate::{
    model::{BlockItem, Kind},
    service::Write,
};

#[derive(Clone, Debug)]
pub enum Target {
    Edit(BlockItem),
    Append {
        parent: String,
        after: Option<String>,
    },
    Create(String),
}

#[derive(Clone, Default, Debug)]
pub struct Buffer {
    pub text: String,
    pub cursor: usize,
}

impl Buffer {
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
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
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
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
        let col = self.text[start..self.cursor].chars().count();
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
        self.cursor = target + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
    }

    pub fn key(&mut self, key: KeyEvent, multiline: bool) {
        // Some terminals send BS (Ctrl-H) instead of DEL for Backspace.
        // Crossterm reports BS as Ctrl-H, so normalize it for all text fields.
        let code = match key.code {
            KeyCode::Char('h') if key.modifiers == KeyModifiers::CONTROL => KeyCode::Backspace,
            code => code,
        };
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
                if let Some((i, _)) = self.text[..self.cursor].char_indices().next_back() {
                    self.cursor = i;
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
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
                if let Some((i, _)) = self.text[..self.cursor].char_indices().next_back() {
                    self.text.replace_range(i..self.cursor, "");
                    self.cursor = i;
                }
            }
            KeyCode::Delete => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.text
                        .replace_range(self.cursor..self.cursor + c.len_utf8(), "");
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct Editor {
    pub target: Target,
    pub buffer: Buffer,
    pub row: Option<RowDraft>,
    pub kind: Kind,
    pub confirmed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RowDraft {
    pub cells: Vec<Buffer>,
    pub active: usize,
}

impl Editor {
    pub fn new(target: Target) -> Self {
        let row = match &target {
            Target::Edit(block) if block.editable_table_row() => Some(RowDraft {
                cells: block
                    .table_cells
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|cell| Buffer::new(cell.text.clone()))
                    .collect(),
                active: 0,
            }),
            _ => None,
        };
        let (text, kind) = match &target {
            Target::Edit(b) => (b.text.clone(), b.kind.unwrap_or(Kind::Paragraph)),
            _ => (String::new(), Kind::Paragraph),
        };
        Self {
            target,
            buffer: Buffer::new(text),
            row,
            kind,
            confirmed: false,
            error: None,
        }
    }

    pub fn active_buffer(&self) -> &Buffer {
        self.row
            .as_ref()
            .map_or(&self.buffer, |row| &row.cells[row.active])
    }

    fn active_buffer_mut(&mut self) -> &mut Buffer {
        match &mut self.row {
            Some(row) => &mut row.cells[row.active],
            None => &mut self.buffer,
        }
    }

    pub fn insert(&mut self, text: &str) {
        let before = self.active_buffer().text.clone();
        let text = if matches!(self.target, Target::Create(_)) {
            text.replace(['\n', '\r'], " ")
        } else {
            text.to_owned()
        };
        self.active_buffer_mut().insert(&text);
        if self.active_buffer().text != before {
            self.confirmed = false;
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        if let Some(row) = &mut self.row {
            if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                let backwards =
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
                row.active = (row.active + if backwards { row.cells.len() - 1 } else { 1 })
                    % row.cells.len();
                return;
            }
        } else if key.code == KeyCode::Tab {
            self.cycle_kind();
            return;
        }
        let before = self.active_buffer().text.clone();
        let multiline = !matches!(self.target, Target::Create(_));
        self.active_buffer_mut().key(key, multiline);
        if self.active_buffer().text != before {
            self.confirmed = false;
        }
    }

    pub fn character_count(&self) -> usize {
        self.row.as_ref().map_or_else(
            || self.buffer.text.chars().count(),
            |row| row.cells.iter().map(|cell| cell.text.chars().count()).sum(),
        )
    }

    pub fn changed(&self) -> bool {
        match &self.target {
            Target::Edit(block) => match &self.row {
                Some(row) => row
                    .cells
                    .iter()
                    .zip(block.table_cells.as_ref().unwrap())
                    .any(|(draft, original)| draft.text != original.text),
                None => self.buffer.text != block.text,
            },
            _ => true,
        }
    }

    pub fn needs_formatting_confirmation(&self) -> bool {
        if self.confirmed {
            return false;
        }
        match &self.target {
            Target::Edit(block) => match &self.row {
                Some(row) => row
                    .cells
                    .iter()
                    .zip(block.table_cells.as_ref().unwrap())
                    .any(|(draft, original)| original.styled && draft.text != original.text),
                None => block.styled && self.changed(),
            },
            _ => false,
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
            Target::Edit(original) => match &self.row {
                Some(row) => Write::UpdateTableRow {
                    original: original.clone(),
                    cells: row.cells.iter().map(|cell| cell.text.clone()).collect(),
                },
                None => Write::Update {
                    original: original.clone(),
                    text: Some(self.buffer.text.clone()),
                },
            },
            Target::Append { parent, after } => Write::Append {
                parent: parent.clone(),
                after: after.clone(),
                kind: self.kind,
                text: self.buffer.text.clone(),
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
        assert!(!editor.needs_formatting_confirmation());
        editor.key(KeyCode::Tab.into());
        assert_eq!(editor.active_buffer().cursor, cursor);
        editor.key(KeyCode::Enter.into());
        assert!(editor.needs_formatting_confirmation());
        editor.confirmed = true;
        editor.key(KeyCode::Left.into());
        editor.key(KeyCode::Tab.into());
        assert!(editor.confirmed);
        editor.key(KeyCode::BackTab.into());
        editor.key(KeyCode::Backspace.into());
        assert!(!editor.confirmed);
        editor.key(KeyCode::BackTab.into());
        editor.key(KeyCode::BackTab.into());
        assert_eq!(editor.row.as_ref().unwrap().active, 2);
        editor.insert("empty no longer");
        let Write::UpdateTableRow { cells, .. } = editor.write().unwrap() else {
            panic!("wrong write");
        };
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[2], "empty no longer");
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
