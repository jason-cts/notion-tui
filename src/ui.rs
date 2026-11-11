use crate::{
    app::{App, Confirm, Focus},
    editor::{Buffer, Target},
    hyperlinks::{self, Links},
    model::BlockLabel,
    rich_text::{self, RichLine, RichRun},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

mod database;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ACCENT: Color = Color::Rgb(104, 211, 195);
const INK: Color = Color::Rgb(221, 228, 235);
const MUTED: Color = Color::Rgb(143, 158, 174);
const BG: Color = Color::Rgb(20, 25, 33);
const PANEL: Color = Color::Rgb(27, 34, 44);
const ERROR: Color = Color::Rgb(255, 154, 145);

#[derive(Default)]
pub struct Ui {
    pages: ListState,
    block_offset: usize,
    database_offsets: std::collections::HashMap<String, usize>,
    pub links: Links,
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

fn wrapped(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for input in clean(text).split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for c in input.chars() {
            let size = c.width().unwrap_or(0);
            if used + size > width && !line.is_empty() {
                lines.push(Line::raw(std::mem::take(&mut line)));
                used = 0;
            }
            line.push(c);
            used += size;
        }
        lines.push(Line::raw(line));
    }
    lines
}

fn wrapped_block(label: &BlockLabel, width: u16) -> Vec<RichLine> {
    if label.rich_text.is_empty() {
        return rich_text::wrap(&[RichRun::plain(label.plain())], width);
    }
    let prefix = clean(&label.prefix);
    let indent = UnicodeWidthStr::width(prefix.as_str());
    if indent >= usize::from(width) {
        let mut runs = vec![RichRun::plain(prefix)];
        runs.extend(label.rich_text.clone());
        return rich_text::wrap(&runs, width);
    }
    rich_text::wrap(&label.rich_text, width - indent as u16)
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            let prefix = if index == 0 {
                prefix.clone()
            } else {
                " ".repeat(indent)
            };
            line.runs.insert(0, RichRun::plain(prefix));
            line
        })
        .collect()
}

fn panel(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(if focused { ACCENT } else { MUTED }))
        .style(Style::default().bg(PANEL).fg(INK))
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

impl Ui {
    pub fn draw(&mut self, frame: &mut Frame, app: &App) {
        self.links.clear();
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().bg(BG).fg(INK)),
            area,
        );
        if area.width < 16 || area.height < 7 {
            frame.render_widget(
                Paragraph::new("Resize terminal\nq to quit").style(Style::default().fg(ACCENT)),
                area,
            );
            return;
        }
        let regions = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);
        let connection = if app.demo {
            " DEMO · read-only"
        } else {
            " CONNECTED"
        };
        let title = Line::from(vec![
            Span::styled(
                "  N / ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "NOTION",
                Style::default().fg(INK).add_modifier(Modifier::BOLD),
            ),
            Span::styled(connection, Style::default().fg(MUTED)),
        ]);
        let path = if app.path.is_empty() {
            "  Your shared workspace".into()
        } else {
            format!(
                "  {}",
                app.path
                    .iter()
                    .map(|p| clean(&p.title))
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        };
        frame.render_widget(
            Paragraph::new(vec![title, Line::styled(path, Style::default().fg(MUTED))]),
            regions[0],
        );
        if area.width >= 80 {
            let panes =
                Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .split(regions[1]);
            self.pages(frame, app, panes[0]);
            self.content(frame, app, panes[1]);
        } else {
            match app.focus {
                Focus::Pages => self.pages(frame, app, regions[1]),
                Focus::Content => self.content(frame, app, regions[1]),
            }
        }
        let busy = if app.saving {
            "Saving…"
        } else if app.loading_content || app.loading_search {
            "Loading…"
        } else {
            &app.status
        };
        frame.render_widget(
            Paragraph::new(clean(busy))
                .style(Style::default().fg(if app.error { ERROR } else { MUTED }))
                .wrap(Wrap { trim: false }),
            Rect {
                height: 2,
                ..regions[2]
            },
        );
        frame.render_widget(
            Paragraph::new(
                if app.database().is_some() { "[ / ] views  h/l columns  Enter page  ←/Backspace parent  v details  o link  r refresh  q quit" }
                else { "Tab panes  / search  Enter open  o link  v read  e edit  a append  ? help  q quit" },
            )
            .style(Style::default().fg(ACCENT)),
            Rect {
                y: regions[2].y + 2,
                height: 1,
                ..regions[2]
            },
        );
        if app.help
            || app.viewer.is_some()
            || app.search.is_some()
            || app.editor.is_some()
            || app.confirm.is_some()
            || app.link_picker.is_some()
        {
            self.links.clear();
        }
        if app.help {
            self.help(frame, app.help_scroll);
        }
        if let Some((label, scroll)) = &app.viewer {
            let area = popup(frame.area(), 100, frame.area().height.saturating_sub(2));
            frame.render_widget(Clear, area);
            let block = panel("FULL BLOCK · ↑/↓ scroll · o link · Esc close".into(), true);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            for (row, line) in wrapped_block(label, inner.width)
                .iter()
                .skip(usize::from(*scroll))
                .take(usize::from(inner.height))
                .enumerate()
            {
                let row = Rect {
                    y: inner.y + row as u16,
                    height: 1,
                    ..inner
                };
                frame.render_widget(Paragraph::new(line.line()), row);
                hyperlinks::record(&mut self.links, line, row);
            }
        }
        if let Some(picker) = &app.link_picker {
            self.links.clear();
            let area = popup(
                frame.area(),
                100,
                frame.area().height.saturating_sub(2).min(24),
            );
            frame.render_widget(Clear, area);
            let items: Vec<_> = picker
                .links
                .iter()
                .map(|(label, url)| {
                    ListItem::new(vec![
                        Line::raw(clean(label)),
                        Line::styled(url.clone(), Style::default().fg(ACCENT)),
                    ])
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(picker.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(panel(
                        "OPEN LINK · ↑/↓ choose · Enter browser · Esc cancel".into(),
                        true,
                    ))
                    .highlight_symbol("▌ ")
                    .highlight_style(Style::default().bg(Color::Rgb(43, 53, 66))),
                area,
                &mut state,
            );
        }
        if let Some(buffer) = &app.search {
            let area = popup(area, 70, 5);
            frame.render_widget(Clear, area);
            frame.render_widget(
                panel("SEARCH PAGES · Enter search · Esc cancel".into(), true),
                area,
            );
            render_buffer(
                frame,
                buffer,
                Rect {
                    x: area.x + 2,
                    y: area.y + 2,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
                true,
            );
        }
        if let Some(editor) = &app.editor {
            let area = popup(area, 100, frame.area().height.saturating_sub(4).min(30));
            frame.render_widget(Clear, area);
            let name = match &editor.target {
                Target::Edit(_) => "EDIT BLOCK",
                Target::Append { after: Some(_), .. } => "APPEND BLOCK · after selected block",
                Target::Append { after: None, .. } => "APPEND BLOCK · first block",
                Target::Create(_) => "NEW CHILD PAGE",
            };
            frame.render_widget(
                panel(format!("{name} · {}", editor.kind.name()), true),
                area,
            );
            let parts = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(if editor.error.is_some() { 4 } else { 1 }),
                Constraint::Length(1),
            ])
            .split(area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            }));
            render_buffer(
                frame,
                &editor.buffer,
                parts[0],
                !app.saving && app.confirm.is_none(),
            );
            let hint = editor
                .error
                .as_deref()
                .unwrap_or("Ctrl-S save · Esc cancel · Tab changes appended block type");
            frame.render_widget(
                Paragraph::new(clean(hint))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(if editor.error.is_some() { ERROR } else { MUTED })),
                parts[1],
            );
            frame.render_widget(
                Paragraph::new(if app.saving {
                    "Saving… please wait".into()
                } else {
                    format!(
                        "{} characters · Ctrl-S save",
                        editor.buffer.text.chars().count()
                    )
                })
                .style(Style::default().fg(ACCENT)),
                parts[2],
            );
        }
        if let Some(confirm) = app.confirm {
            let area = popup(area, 68, 8);
            frame.render_widget(Clear, area);
            let text = match confirm {
                Confirm::Formatting => {
                    "Saving replaces styled text and links with plain text.\nBlock color and child blocks are preserved.\n\nContinue? y / n"
                }
                Confirm::Discard => "Discard this draft?\n\ny discard · n keep editing",
                Confirm::Quit => "Quit and discard this draft?\n\ny quit · n keep editing",
            };
            frame.render_widget(
                Paragraph::new(text)
                    .block(panel("CONFIRM".into(), true))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
    }

    fn pages(&mut self, frame: &mut Frame, app: &App, area: Rect) {
        let title = if app.query.is_empty() {
            format!("PAGES · {}", app.pages.len())
        } else {
            format!("PAGES · /{}", clean(&app.query))
        };
        let block = panel(title, app.focus == Focus::Pages);
        if app.pages.is_empty() {
            frame.render_widget(Paragraph::new(if app.loading_search { "\n  Searching…" }
                else { "\n  No pages found.\n\n  Share pages with your integration,\n  then press r to refresh." }).block(block).wrap(Wrap { trim: false }), area);
        } else {
            let items: Vec<_> = app
                .pages
                .iter()
                .map(|p| ListItem::new(wrapped(&p.title, area.width.saturating_sub(5))))
                .collect();
            self.pages.select(Some(app.page_selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_symbol("▌ ")
                    .highlight_style(
                        Style::default()
                            .bg(Color::Rgb(43, 64, 72))
                            .fg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                area,
                &mut self.pages,
            );
        }
    }

    fn content(&mut self, frame: &mut Frame, app: &App, area: Rect) {
        if app.database().is_some() {
            self.database_content(frame, app, area);
            return;
        }
        let title = app
            .blocks
            .get(app.block_selected)
            .map_or("CONTENT".into(), |b| {
                format!(
                    "CONTENT · {} · {}/{}",
                    app.content_type(b),
                    app.block_selected + 1,
                    app.blocks.len()
                )
            });
        let block = panel(title, app.focus == Focus::Content);
        let inner = block.inner(area);
        if app.blocks.is_empty() {
            self.block_offset = 0;
            frame.render_widget(Paragraph::new(if app.loading_content { "\n  Loading content…" } else if app.path.is_empty() {
                "\n  Open a page to get started.\n\n  / search your workspace\n  Enter opens a page\n  ? shows all keyboard shortcuts"
            } else { "\n  No blocks here.\n\n  a append a block · n create a child page" }).block(block).wrap(Wrap { trim: false }), area);
            return;
        }
        frame.render_widget(block, area);
        let rows: Vec<_> = app
            .block_labels()
            .iter()
            .map(|label| {
                let mut lines = wrapped_block(label, inner.width.saturating_sub(3));
                lines.push(RichLine::default());
                lines
            })
            .collect();
        let selected = app.block_selected.min(rows.len() - 1);
        self.block_offset = self.block_offset.min(selected);
        while self.block_offset < selected
            && rows[self.block_offset..=selected]
                .iter()
                .map(Vec::len)
                .sum::<usize>()
                > usize::from(inner.height)
        {
            self.block_offset += 1;
        }
        let mut y = inner.y;
        for (index, lines) in rows.iter().enumerate().skip(self.block_offset) {
            let color = if app.blocks[index].block_type.starts_with("heading_") {
                ACCENT
            } else {
                INK
            };
            let style = Style::default().fg(color).bg(if index == selected {
                Color::Rgb(43, 53, 66)
            } else {
                PANEL
            });
            for (row_index, line) in lines.iter().enumerate() {
                if y >= inner.bottom() {
                    return;
                }
                let row = Rect {
                    y,
                    height: 1,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(if index == selected && row_index == 0 {
                        "▌"
                    } else {
                        ""
                    })
                    .style(style),
                    row,
                );
                let text_area = Rect {
                    x: inner.x + 2,
                    width: inner.width.saturating_sub(3),
                    ..row
                };
                frame.render_widget(Paragraph::new(line.line()).style(style), text_area);
                hyperlinks::record(&mut self.links, line, text_area);
                y += 1;
            }
        }
    }

    fn help(&self, frame: &mut Frame, scroll: u16) {
        let area = popup(frame.area(), 76, 25);
        frame.render_widget(Clear, area);
        let text = concat!(
            "\n  NAVIGATE\n",
            "  ↑/↓ or j/k    Move selection       g/G      First/last\n",
            "  Tab           Switch panes         Enter    Open page/children\n  [ / ]         Database views       h/l      Columns / table scroll\n",
            "  ←/Backspace   Back to parent        /        Search page titles\n",
            "  r             Refresh focused pane v        Read full block\n",
            "  o             Open selected block's link in your browser\n",
            "  q / Ctrl-C    Quit                 ?        Help\n\n",
            "  EDIT\n",
            "  e             Edit selected text block\n",
            "  Space         Toggle selected to-do\n",
            "  a             Insert a block after the selected block\n",
            "  n             Create a child of the current page\n",
            "  Ctrl-S        Save draft            Esc      Cancel draft\n",
            "  Tab           Choose appended block type\n",
            "  Arrows/Home/End move the text cursor; Enter inserts a newline.\n\n",
            "  Styled text requires confirmation before replacement.\n",
            "  Mentions, equations, and unsupported types are read-only.\n",
            "  A failed save keeps your draft. Check Notion before retrying.\n\n",
            "  ↑/↓ or j/k scroll · PageUp/PageDown move ten lines\n",
            "  Any other key closes help."
        );
        frame.render_widget(
            Paragraph::new(text)
                .block(panel("HELP · ↑/↓ scroll · Esc close".into(), true))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            area,
        );
    }
}

fn render_buffer(frame: &mut Frame, buffer: &Buffer, area: Rect, cursor: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (row, col) = buffer.position();
    let top = row.saturating_sub(usize::from(area.height.saturating_sub(1)));
    let left = col.saturating_sub(usize::from(area.width.saturating_sub(1)));
    frame.render_widget(
        Paragraph::new(buffer.text.as_str())
            .scroll((
                top.min(u16::MAX as usize) as u16,
                left.min(u16::MAX as usize) as u16,
            ))
            .style(Style::default().fg(INK).bg(PANEL)),
        area,
    );
    if cursor {
        frame.set_cursor_position((area.x + (col - left) as u16, area.y + (row - top) as u16));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::DemoService;
    use ratatui::{Terminal, backend::TestBackend};
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn renders_normal_narrow_and_tiny_with_unicode_and_editor() {
        for (width, height) in [(120, 35), (45, 20), (16, 7), (10, 4), (1, 1)] {
            let (tx, _) = mpsc::unbounded_channel();
            let mut app = App::new(Arc::new(DemoService), tx, true);
            app.pages = vec![crate::model::PageItem {
                id: "p".into(),
                title: "你好 🦀 café".into(),
            }];
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut ui = Ui::default();
            terminal.draw(|f| ui.draw(f, &app)).unwrap();
            let contents: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            if width >= 45 {
                assert!(contents.contains('你') && contents.contains('好'));
                assert!(contents.contains("NOTION"));
            }
            app.focus = Focus::Content;
            app.blocks = vec![crate::model::tests::paragraph()];
            terminal.draw(|f| ui.draw(f, &app)).unwrap();
            if width >= 45 {
                let contents: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(contents.contains("Hello"));
            }
            app.editor = Some(crate::editor::Editor::new(Target::Append {
                parent: "p".into(),
                after: None,
            }));
            app.editor
                .as_mut()
                .unwrap()
                .buffer
                .insert("你好 🦀\nsecond line");
            terminal.draw(|f| ui.draw(f, &app)).unwrap();
            app.confirm = Some(Confirm::Discard);
            terminal.draw(|f| ui.draw(f, &app)).unwrap();
            app.confirm = None;
            app.editor = None;
            app.viewer = Some((
                BlockLabel {
                    prefix: String::new(),
                    text: "Long text\n".repeat(100),
                    rich_text: vec![RichRun::plain("Long text\n".repeat(100))],
                },
                30,
            ));
            terminal.draw(|f| ui.draw(f, &app)).unwrap();
            app.viewer = None;
            app.help = true;
            terminal.draw(|f| ui.draw(f, &app)).unwrap();
        }
    }

    #[test]
    fn wrapping_counts_terminal_cells() {
        let lines = wrapped("你好🦀abc", 4);
        assert!(
            lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.to_string().as_str()) <= 4)
        );
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn list_continuations_align_after_the_marker() {
        let label = BlockLabel {
            prefix: "    iii. ".into(),
            text: "你好世界\nnext line".into(),
            rich_text: vec![RichRun::plain("你好世界\nnext line")],
        };
        let lines = wrapped_block(&label, 13)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            [
                "    iii. 你好",
                "         世界",
                "         next",
                "          lin",
                "         e"
            ]
        );
        assert!(
            lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= 13)
        );
        assert!(!wrapped_block(&label, 3).is_empty());
    }

    #[test]
    fn scrolled_numbered_lists_and_reader_share_contextual_labels() {
        for (depth, marker) in [(0, "12."), (1, "l."), (2, "xii.")] {
            for width in [45, 120] {
                let (tx, _) = mpsc::unbounded_channel();
                let mut app = App::new(Arc::new(DemoService), tx, true);
                app.focus = Focus::Content;
                app.path.push(crate::app::Location {
                    id: "list".into(),
                    title: "List".into(),
                    kind: crate::app::LocationKind::Blocks,
                    page_id: "page".into(),
                    list_depth: depth,
                });
                app.blocks = (1..=12).map(|number| crate::model::BlockItem::from_value(&serde_json::json!({
                    "id":number.to_string(), "type":"numbered_list_item",
                    "numbered_list_item":{"rich_text":[{"type":"text","plain_text":format!("item{number}")}]}
                }))).collect();
                app.block_selected = 11;
                let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
                terminal
                    .draw(|frame| Ui::default().draw(frame, &app))
                    .unwrap();
                let contents: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(contents.contains(&format!("{marker} item12")), "{contents}");
                app.key(crossterm::event::KeyCode::Char('v').into());
                assert_eq!(
                    app.viewer.unwrap().0.plain(),
                    format!("{}{marker} item12", "  ".repeat(depth))
                );
            }
        }
    }
    #[test]
    fn rich_styles_and_links_survive_wrapping_scrolling_reader_and_overlays() {
        let (tx, _) = mpsc::unbounded_channel();
        let mut app = App::new(Arc::new(DemoService), tx, true);
        app.focus = Focus::Content;
        let styled = crate::model::BlockItem::from_value(
            &serde_json::json!({"type":"paragraph", "paragraph":{"rich_text":[
                {"type":"text","plain_text":"plain "},
                {"type":"text","plain_text":"BoldItalicStrike", "href":"https://example.com/", "annotations":{"bold":true,"italic":true,"strikethrough":true}},
                {"type":"text","plain_text":" after"}
            ]}}),
        );
        app.blocks = vec![crate::model::tests::paragraph(); 20];
        app.blocks.push(styled);
        app.block_selected = 20;
        let required =
            Modifier::BOLD | Modifier::ITALIC | Modifier::CROSSED_OUT | Modifier::UNDERLINED;
        for width in [25, 45, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
            let mut ui = Ui::default();
            for reader in [false, true] {
                if reader {
                    app.key(crossterm::event::KeyCode::Char('v').into());
                }
                terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
                assert!(!ui.links.is_empty());
                for (&position, url) in &ui.links {
                    let cell = &terminal.backend().buffer()[position];
                    assert_eq!(url, "https://example.com/");
                    assert!(cell.modifier.contains(required), "{cell:?}");
                    assert!(!cell.symbol().contains('\x1b'));
                }
                let plain_cells: Vec<_> = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .filter(|cell| cell.symbol() == "p")
                    .collect();
                assert!(
                    plain_cells
                        .iter()
                        .any(|cell| !cell.modifier.intersects(required))
                );
            }
            app.viewer = None;
            app.help = true;
            terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
            assert!(
                ui.links.is_empty(),
                "Hidden content must not leave clickable links on a popup"
            );
            app.help = false;
        }
    }
}
