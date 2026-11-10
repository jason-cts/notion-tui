//! Keep OSC 8 metadata outside ratatui's cell symbols and diff/width calculations.
use crate::rich_text::RichLine;
use crossterm::{
    cursor::{RestorePosition, SavePosition},
    queue,
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    buffer::Buffer,
    layout::Rect,
};
use std::{
    collections::BTreeMap,
    io::{self, Write},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const CLOSE: &[u8] = b"\x1b]8;;\x1b\\";
pub type Links = BTreeMap<(u16, u16), String>;

pub fn record(links: &mut Links, line: &RichLine, area: Rect) {
    let mut x = area.x;
    for run in &line.runs {
        for grapheme in run.text.graphemes(true) {
            let width = UnicodeWidthStr::width(grapheme) as u16;
            if x.saturating_add(width) > area.right() {
                return;
            }
            if let Some(url) = &run.url {
                for cell in x..x + width {
                    links.insert((cell, area.y), url.clone());
                }
            }
            x += width;
        }
    }
}

#[derive(Default)]
pub struct Output {
    previous: Links,
}

impl Output {
    /// Repaint linked cells after ratatui's diff, retaining their final styles.
    /// Also repaint removed links, even if their label and style did not change.
    pub fn emit(
        &mut self,
        writer: &mut impl Write,
        buffer: &Buffer,
        links: &Links,
    ) -> io::Result<()> {
        if links.is_empty() && self.previous.is_empty() {
            return Ok(());
        }
        queue!(writer, SavePosition)?;
        writer.write_all(CLOSE)?;
        for y in buffer.area.y..buffer.area.bottom() {
            let mut x = buffer.area.x;
            while x < buffer.area.right() {
                let cell = &buffer[(x, y)];
                let width = (UnicodeWidthStr::width(cell.symbol()).max(1) as u16)
                    .min(buffer.area.right() - x);
                let affected = (x..x + width).any(|col| {
                    links.contains_key(&(col, y)) || self.previous.contains_key(&(col, y))
                });
                if affected {
                    if let Some(url) = links.get(&(x, y)) {
                        write!(writer, "\x1b]8;;{url}\x1b\\")?;
                    }
                    CrosstermBackend::new(&mut *writer).draw(std::iter::once((x, y, cell)))?;
                    writer.write_all(CLOSE)?;
                }
                x += width;
            }
        }
        queue!(writer, RestorePosition)?;
        writer.flush()?;
        self.previous = links.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::RichRun;
    use ratatui::style::{Modifier, Style};

    #[test]
    fn output_emits_osc8_and_styles_without_modifying_buffer_symbols() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 2));
        let style = Style::default().add_modifier(
            Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::CROSSED_OUT,
        );
        buffer.set_string(0, 0, "你好ab", style);
        let original = buffer.clone();
        let links: Links = (0..6)
            .map(|x| ((x, 0), "https://example.com/".into()))
            .collect();
        let mut output = Output::default();
        let mut bytes = Vec::new();
        output.emit(&mut bytes, &buffer, &links).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text.matches("\x1b]8;;https://example.com/\x1b\\").count(),
            4
        );
        for code in ["\x1b[1m", "\x1b[3m", "\x1b[4m", "\x1b[9m"] {
            assert!(text.contains(code), "{text:?}");
        }
        assert_eq!(buffer, original);
        assert!(text.ends_with("\x1b8")); // Saved cursor is restored after the overlay.
    }

    #[test]
    fn changed_and_removed_targets_are_updated_even_when_text_does_not_change() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer.set_string(0, 0, "same", Style::default());
        let mut output = Output::default();
        let mut links = Links::from([((0, 0), "https://old.example/".into())]);
        output.emit(&mut Vec::new(), &buffer, &links).unwrap();
        links.insert((0, 0), "https://new.example/".into());
        let mut bytes = Vec::new();
        output.emit(&mut bytes, &buffer, &links).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("https://new.example/"));
        assert!(!text.contains("old.example"));
        let mut bytes = Vec::new();
        output.emit(&mut bytes, &buffer, &Links::new()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains('s'));
        assert!(!text.contains("https://"));
        let mut bytes = Vec::new();
        output.emit(&mut bytes, &buffer, &Links::new()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn recording_clips_links_and_removed_links_respect_wide_cell_boundaries() {
        let mut run = RichRun::plain("你好ab");
        run.url = Some("https://example.com/".into());
        let mut links = Links::new();
        record(
            &mut links,
            &RichLine { runs: vec![run] },
            Rect::new(1, 1, 4, 1),
        );
        assert_eq!(links.len(), 4);
        assert!(links.keys().all(|&(x, y)| (1..5).contains(&x) && y == 1));
        let mut output = Output {
            previous: Links::from([((1, 0), "https://old.example/".into())]),
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer.set_string(0, 0, "你", Style::default());
        let mut bytes = Vec::new();
        output.emit(&mut bytes, &buffer, &Links::new()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.matches('你').count(), 1);
        assert!(!text.contains("https://"));
        // Old positions beyond a resized frame are discarded without being painted.
        output
            .emit(
                &mut Vec::new(),
                &Buffer::empty(Rect::new(0, 0, 1, 1)),
                &Links::new(),
            )
            .unwrap();
    }
}
