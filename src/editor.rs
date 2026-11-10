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
    pub kind: Kind,
    pub confirmed: bool,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(target: Target) -> Self {
        let (text, kind) = match &target {
            Target::Edit(b) => (b.text.clone(), b.kind.unwrap_or(Kind::Paragraph)),
            _ => (String::new(), Kind::Paragraph),
        };
        Self {
            target,
            buffer: Buffer::new(text),
            kind,
            confirmed: false,
            error: None,
        }
    }

    pub fn cycle_kind(&mut self) {
        if matches!(self.target, Target::Append { .. }) {
            let index = Kind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
            self.kind = Kind::ALL[(index + 1) % Kind::ALL.len()];
        }
    }

    pub fn write(&self) -> Result<Write, &'static str> {
        if self.buffer.text.chars().count() > 100_000 {
            return Err("Text is limited to 100,000 characters per block");
        }
        Ok(match &self.target {
            Target::Edit(original) => Write::Update {
                original: original.clone(),
                text: Some(self.buffer.text.clone()),
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
