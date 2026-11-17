use crate::{
    app::{App, Confirm, Focus},
    editor::{Buffer, Editor, Target},
    hyperlinks::{self, Links},
    model::{BlockLabel, InsertPosition},
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
    block_offsets: std::collections::HashMap<String, usize>,
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
            if matches!(app.confirm, Some(Confirm::Delete | Confirm::Relocate)) {
                render_confirmation(frame, app);
                return;
            }
            if let Some(editor) = &app.editor
                && editor.row.is_some()
            {
                render_row_editor(frame, editor, app.saving, app.confirm.is_none());
                render_style_picker(frame, editor);
                render_confirmation(frame, app);
                return;
            }
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
            if app.pending_delete.is_some() {
                "Deleting…"
            } else {
                "Saving…"
            }
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
                if app.database().is_some_and(crate::database::DatabaseState::is_table) { "arrows select cells  e edit  Enter icon opens page  Backspace parent  [ / ] views  < > more columns" }
                else if app.database().is_some() { "[ / ] views  ←/→ columns  Enter page  Backspace parent  v details  o link  r refresh  q quit" }
                else { "Tab panes  / search  Enter open  o link  v read  e edit  d delete  >/< indent/outdent  a after  A before  ? help  q quit" },
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
            if editor.cell.is_some() {
                database::render_cell_editor(frame, editor, app.saving, app.confirm.is_none());
            } else if editor.row.is_some() {
                render_row_editor(frame, editor, app.saving, app.confirm.is_none());
            } else {
                let area = popup(area, 100, frame.area().height.saturating_sub(4).min(30));
                frame.render_widget(Clear, area);
                let name = match &editor.target {
                    Target::Edit(_) => "EDIT BLOCK",
                    Target::DatabaseCell(_) => unreachable!("cell editor rendered separately"),
                    Target::Append {
                        position: InsertPosition::Start,
                        ..
                    } => "INSERT BLOCK · at beginning",
                    Target::Append {
                        position: InsertPosition::After(_),
                        ..
                    } => "INSERT BLOCK",
                    Target::Append {
                        position: InsertPosition::End,
                        ..
                    } => "INSERT BLOCK · first block",
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
                    !app.saving && app.confirm.is_none() && editor.style_picker.is_none(),
                );
                let hint = editor
                    .error
                    .as_deref()
                    .unwrap_or(if editor.buffer.rich.is_some() {
                        "Ctrl-S save · Esc cancel · Shift+F10 styles · Tab block type"
                    } else {
                        "Ctrl-S save · Esc cancel"
                    });
                frame.render_widget(
                    Paragraph::new(clean(hint))
                        .wrap(Wrap { trim: false })
                        .style(Style::default().fg(if editor.error.is_some() {
                            ERROR
                        } else {
                            MUTED
                        })),
                    parts[1],
                );
                frame.render_widget(
                    Paragraph::new(if app.saving {
                        Line::raw("Saving… please wait")
                    } else {
                        style_status(
                            &editor.buffer,
                            format!(
                                "{} characters · Ctrl-S save",
                                editor.buffer.text.chars().count()
                            ),
                        )
                    })
                    .style(Style::default().fg(ACCENT)),
                    parts[2],
                );
            }
            render_style_picker(frame, editor);
        }
        render_confirmation(frame, app);
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
        let location = app.location().map_or("", |location| location.id.as_str());
        if app.blocks.is_empty() {
            if !app.loading_content && !app.error {
                self.block_offsets.remove(location);
            }
            frame.render_widget(Paragraph::new(if app.loading_content { "\n  Loading content…" } else if app.path.is_empty() {
                "\n  Open a page to get started.\n\n  / search your workspace\n  Enter opens a page\n  ? shows all keyboard shortcuts"
            } else { "\n  No blocks here.\n\n  a/A insert a block · n create a child page" }).block(block).wrap(Wrap { trim: false }), area);
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
        let block_offset = self.block_offsets.entry(location.to_owned()).or_default();
        *block_offset = (*block_offset).min(selected);
        while *block_offset < selected
            && rows[*block_offset..=selected]
                .iter()
                .map(Vec::len)
                .sum::<usize>()
                > usize::from(inner.height)
        {
            *block_offset += 1;
        }
        let mut y = inner.y;
        for (index, lines) in rows.iter().enumerate().skip(*block_offset) {
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
            "  ↑/↓           Move selection       g/G      First/last\n",
            "  Tab           Switch panes         Enter    Open page/children\n  [ / ]         Database views       ←/→      Board columns / table cells\n",
            "  Backspace     Back to parent\n",
            "  /             Search page titles\n",
            "  TABLE VIEWS   Arrow keys select a single cell\n",
            "  Enter         Open page from its icon; e edits a property\n",
            "  < / >         More property columns to the left / right\n",
            "  r             Refresh focused pane v        Read full block\n",
            "  o             Open selected block's link in your browser\n",
            "  q / Ctrl-C    Quit                 ?        Help\n\n",
            "  EDIT\n",
            "  e             Edit selected text block, table row, or database cell\n",
            "  d             Delete selected block (confirm with y)\n",
            "  > / <         Indent / outdent selected leaf block (confirm with y)\n",
            "  Space         Toggle selected to-do\n",
            "  a / Shift+A   Insert a block after / before the selected block\n",
            "  n             Create a child of the current page\n",
            "  Ctrl-S        Save draft            Esc      Cancel draft\n",
            "  Tab           Choose inserted block type\n",
            "  Tab/Shift-Tab Move between table-row cells\n",
            "  CELL PICKERS  ↑/↓ choose; Space/Enter select; Ctrl-S save\n",
            "  DATE CELLS    Tab/Shift-Tab fields; Ctrl-D clears the date\n",
            "  Arrows/Home/End move the text cursor; Enter inserts a newline.\n\n",
            "  Shift+Arrows  Select text; typing replaces the selection\n",
            "  Ctrl-B/I/U    Bold / italic / underline\n",
            "  Shift+F10     Bold / italic / underline / strikethrough\n",
            "  B I U S       Highlighted: on; ~: mixed; dim: off\n",
            "  Ctrl-I needs enhanced keys; otherwise use Shift+F10.\n",
            "  Mentions, equations, and unsupported types cannot be edited.\n",
            "  Deleting a container includes its contents; restore in Notion Trash.\n",
            "  Indent/outdent recreates the block, then trashes the original.\n",
            "  A failed save keeps your draft. Check Notion before retrying.\n\n",
            "  ↑/↓        scroll · PageUp/PageDown move ten lines\n",
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

fn render_confirmation(frame: &mut Frame, app: &App) {
    if let Some(confirm) = app.confirm {
        let text = match confirm {
            Confirm::Delete => {
                render_delete_confirmation(frame, app);
                return;
            }
            Confirm::Relocate => {
                render_relocation_confirmation(frame, app);
                return;
            }
            Confirm::Discard => "Discard this draft?\n\ny discard · n keep editing",
            Confirm::Quit => "Quit and discard this draft?\n\ny quit · n keep editing",
        };
        let area = popup(frame.area(), 68, 8);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(text)
                .block(panel("CONFIRM".into(), true))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn render_relocation_confirmation(frame: &mut Frame, app: &App) {
    let Some(pending) = &app.pending_relocation else {
        return;
    };
    let retry = matches!(
        pending.request.state,
        crate::service::RelocationState::Copied { .. }
    );
    let action = if pending.indent {
        "INDENT BLOCK"
    } else {
        "OUTDENT BLOCK"
    };
    let area = popup(frame.area(), 76, 16);
    frame.render_widget(Clear, area);
    let border = panel(action.into(), true);
    let inner = border.inner(area);
    frame.render_widget(border, area);
    let controls = wrapped(
        if inner.width < 26 {
            "y apply\nn/Esc cancel"
        } else {
            "y apply · n/Esc cancel"
        },
        inner.width,
    );
    let body_height = inner.height.saturating_sub(controls.len() as u16);
    let destination = pending.path.last().map_or("", |l| l.title.as_str());
    let text = if retry {
        format!(
            "Resume the pending move to {}?\nThe replacement already exists. Verify it and finish trashing the original; no new copy will be created.",
            clean(destination)
        )
    } else {
        format!(
            "{}?\nCreate a replacement, then move the original to Trash.\nNew block ID; history and comments stay with the original. Links to the original still point there.\nDestination: {}",
            if pending.indent {
                "Nest under the previous sibling"
            } else {
                "Place after the parent"
            },
            clean(destination)
        )
    };
    let preview: String = clean(&pending.request.original.text)
        .chars()
        .take(80)
        .collect();
    let mut lines = wrapped(&text, inner.width);
    lines.extend(wrapped(&format!("Block: {preview}"), inner.width));
    lines.truncate(usize::from(body_height));
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: body_height,
            ..inner
        },
    );
    frame.render_widget(
        Paragraph::new(controls).style(Style::default().fg(ERROR)),
        Rect {
            y: inner.y + body_height,
            height: inner.height - body_height,
            ..inner
        },
    );
}

fn render_delete_confirmation(frame: &mut Frame, app: &App) {
    let Some(target) = &app.pending_delete else {
        return;
    };
    let block = &target.block;
    let includes_contents =
        block.children || block.child_page || block.block_type == "child_database";
    let area = popup(frame.area(), 68, 12);
    frame.render_widget(Clear, area);
    let border = panel("DELETE BLOCK".into(), true);
    let inner = border.inner(area);
    frame.render_widget(border, area);
    let controls = wrapped(
        if inner.width < 26 {
            "y delete\nn/Esc cancel"
        } else {
            "y delete · n/Esc cancel"
        },
        inner.width,
    );
    let body_height = inner.height.saturating_sub(controls.len() as u16);
    let mut lines = wrapped("Move to Trash?", inner.width);
    if includes_contents {
        lines.extend(wrapped("Includes contents.", inner.width));
    }
    lines.extend(wrapped(&format!("Type: {}", block.block_type), inner.width));
    let preview: String = clean(&block.text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect();
    let preview = if preview.is_empty() {
        "(empty)"
    } else {
        &preview
    };
    lines.extend(wrapped(preview, inner.width).into_iter().take(2));
    lines.truncate(usize::from(body_height));
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: body_height,
            ..inner
        },
    );
    frame.render_widget(
        Paragraph::new(controls).style(Style::default().fg(ERROR)),
        Rect {
            y: inner.y + body_height,
            height: inner.height - body_height,
            ..inner
        },
    );
}

fn render_row_editor(frame: &mut Frame, editor: &Editor, saving: bool, cursor: bool) {
    let row = editor.row.as_ref().unwrap();
    let screen = frame.area();
    let area = if screen.height < 10 {
        screen
    } else {
        popup(screen, 100, screen.height.saturating_sub(2).min(30))
    };
    frame.render_widget(Clear, area);
    let inner = if area.height >= 6 && area.width >= 8 {
        let block = panel(
            format!(
                "EDIT TABLE ROW · Cell {}/{}",
                row.active + 1,
                row.cells.len()
            ),
            true,
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    } else {
        area
    };
    let fields = if inner.height >= 5 {
        let parts = Layout::vertical([
            Constraint::Min(2),
            Constraint::Length(if editor.error.is_some() { 2 } else { 1 }),
            Constraint::Length(1),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(clean(editor.error.as_deref().unwrap_or(
                "Tab/Shift-Tab cells · Shift+F10 styles · Ctrl-S save · Esc cancel",
            )))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(if editor.error.is_some() { ERROR } else { MUTED })),
            parts[1],
        );
        frame.render_widget(
            Paragraph::new(if saving {
                Line::raw("Saving… please wait")
            } else {
                style_status(
                    editor.active_buffer(),
                    format!(
                        "Cell {}/{} · {} characters · Ctrl-S save",
                        row.active + 1,
                        row.cells.len(),
                        editor.character_count()
                    ),
                )
            })
            .style(Style::default().fg(ACCENT)),
            parts[2],
        );
        parts[0]
    } else {
        inner
    };
    if fields.height == 0 || fields.width == 0 {
        return;
    }
    // Each visible field gets a label and at least one text line. Keep the active
    // field on screen without allocating space for every column in a wide table.
    let count = (usize::from(fields.height) / 4).max(1).min(row.cells.len());
    let start = row.active.saturating_sub(count - 1);
    let areas = Layout::vertical(vec![Constraint::Fill(1); count]).split(fields);
    for (offset, &area) in areas.iter().enumerate() {
        let index = start + offset;
        let active = index == row.active;
        let text_area = if area.height >= 2 {
            frame.render_widget(
                Paragraph::new(format!(
                    "{}Cell {}",
                    if active { "› " } else { "  " },
                    index + 1
                ))
                .style(Style::default().fg(if active { ACCENT } else { MUTED })),
                Rect { height: 1, ..area },
            );
            Rect {
                y: area.y + 1,
                height: area.height - 1,
                ..area
            }
        } else {
            area
        };
        if active {
            render_buffer(
                frame,
                &row.cells[index],
                text_area,
                cursor && !saving && editor.style_picker.is_none(),
            );
        } else {
            frame.render_widget(
                Paragraph::new(row.cells[index].styled_text()).style(Style::default().fg(MUTED)),
                text_area,
            );
        }
    }
}

fn style_status(buffer: &Buffer, suffix: String) -> Line<'static> {
    if buffer.rich.is_none() {
        return Line::raw(suffix);
    }
    let mut spans = vec![];
    for (i, (name, state)) in ["B", "I", "U", "S"]
        .into_iter()
        .zip(buffer.style_states())
        .enumerate()
    {
        let style = match state {
            Some(true) => Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::REVERSED | crate::editor::rich::STYLE_MODIFIERS[i]),
            Some(false) => Style::default().fg(MUTED).add_modifier(Modifier::DIM),
            None => Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled(
            format!("{name}{} ", if state.is_none() { "~" } else { "" }),
            style,
        ));
    }
    spans.push(Span::raw(format!("· {suffix}")));
    Line::from(spans)
}

fn render_style_picker(frame: &mut Frame, editor: &Editor) {
    let Some(picker) = &editor.style_picker else {
        return;
    };
    let area = popup(frame.area(), 46, 9);
    frame.render_widget(Clear, area);
    let block = panel("TEXT STYLE".into(), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let parts = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(inner);
    let items: Vec<_> = crate::editor::rich::STYLE_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mark = match picker.checked[i] {
                Some(true) => "[x]",
                Some(false) => "[ ]",
                None => "[-]",
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{mark} ")),
                Span::styled(
                    *name,
                    Style::default().add_modifier(crate::editor::rich::STYLE_MODIFIERS[i]),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(picker.selected));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        parts[0],
        &mut state,
    );
    frame.render_widget(
        Paragraph::new("↑/↓ choose · Space toggle\nEnter apply · Esc cancel")
            .wrap(Wrap { trim: false }),
        parts[1],
    );
}

fn render_buffer(frame: &mut Frame, buffer: &Buffer, area: Rect, cursor: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (row, col) = buffer.position();
    let top = row.saturating_sub(usize::from(area.height.saturating_sub(1)));
    let left = col.saturating_sub(usize::from(area.width.saturating_sub(1)));
    frame.render_widget(
        Paragraph::new(buffer.styled_text())
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
    fn editing_renders_combined_styles_selection_and_status() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut buffer = Buffer::from_rich(&crate::model::RichText::from("styled").0);
        buffer.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT), true);
        for i in 0..4 {
            buffer.toggle_style(i);
        }
        let mut terminal = Terminal::new(TestBackend::new(50, 8)).unwrap();
        terminal
            .draw(|frame| {
                render_buffer(frame, &buffer, Rect::new(0, 0, 50, 5), true);
                frame.render_widget(
                    Paragraph::new(style_status(&buffer, "characters".into())),
                    Rect::new(0, 6, 50, 1),
                );
            })
            .unwrap();
        let rendered = terminal.backend().buffer();
        let expected = Modifier::BOLD
            | Modifier::ITALIC
            | Modifier::UNDERLINED
            | Modifier::CROSSED_OUT
            | Modifier::REVERSED;
        assert!(rendered[(0, 0)].modifier.contains(expected));
        for (x, label) in [(0, "B"), (2, "I"), (4, "U"), (6, "S")] {
            assert_eq!(rendered[(x, 6)].symbol(), label);
            assert!(rendered[(x, 6)].modifier.contains(Modifier::REVERSED));
        }
        buffer.key(KeyCode::Right.into(), true);
        assert!(buffer.selection().is_none());
    }

    #[test]
    fn style_picker_and_mixed_status_render_on_small_screens() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut editor = Editor::new(Target::Append {
            parent: "p".into(),
            position: InsertPosition::End,
        });
        editor.insert("a");
        editor.buffer.toggle_style(0);
        editor.insert("b");
        editor.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
        assert_eq!(
            style_status(&editor.buffer, String::new()).to_string(),
            "B~ I U S · "
        );
        editor.key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT));
        for (width, height) in [(100, 30), (45, 20), (16, 7), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_style_picker(frame, &editor))
                .unwrap();
            if width >= 45 {
                let symbols: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(symbols.contains("TEXT STYLE"));
                assert!(symbols.contains("[-] Bold"));
                assert!(symbols.contains("Strikethrough"));
            }
        }
    }

    #[test]
    fn row_dialog_scrolls_active_cell_into_view_and_survives_tiny_terminals() {
        for (width, height) in [(120, 35), (45, 20), (16, 7), (10, 4), (1, 1)] {
            let (tx, _) = mpsc::unbounded_channel();
            let mut app = App::new(Arc::new(DemoService), tx, true);
            let mut row = crate::model::tests::table_row();
            let cell = row.table_cells.as_ref().unwrap()[0].clone();
            row.table_cells = Some(vec![cell; 20]);
            app.editor = Some(Editor::new(Target::Edit(row)));
            let editor = app.editor.as_mut().unwrap();
            for _ in 0..19 {
                editor.key(crossterm::event::KeyCode::Tab.into());
            }
            editor.insert(&format!("\n{}TAIL", "line\n".repeat(100)));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut ui = Ui::default();
            terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
            let contents: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if width >= 10 {
                assert!(contents.contains("TAIL"), "{width}x{height}: {contents}");
            }
            if width >= 45 {
                assert!(contents.contains("Cell 20"), "{contents}");
            }
            let position = terminal.get_cursor_position().unwrap();
            assert!(position.x < width && position.y < height);
            assert!(ui.links.is_empty());
            app.editor.as_mut().unwrap().error =
                Some("Conflict: row changed; draft retained".into());
            terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
            if width >= 45 {
                let contents: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(contents.contains("Conflict"));
                assert!(contents.contains("TAIL"));
            }
        }
    }

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
                position: InsertPosition::End,
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
    fn relocation_prompt_explains_recreation_and_keeps_cancel_controls_visible() {
        for (width, height) in [(120, 35), (45, 20), (22, 12), (16, 7), (10, 4), (1, 1)] {
            for indent in [true, false] {
                let (tx, _) = mpsc::unbounded_channel();
                let mut app = App::new(Arc::new(DemoService), tx, true);
                let location = crate::app::Location {
                    id: "parent".into(),
                    title: "Destination".into(),
                    kind: crate::app::LocationKind::Blocks,
                    page_id: "page".into(),
                    list_depth: 1,
                };
                app.pending_relocation = Some(crate::app::PendingRelocation {
                    request: crate::service::Relocation {
                        original: crate::model::tests::paragraph(),
                        source_parent: "page".into(),
                        destination: "parent".into(),
                        position: InsertPosition::End,
                        state: crate::service::RelocationState::Ready,
                    },
                    path: vec![location],
                    indent,
                });
                app.confirm = Some(Confirm::Relocate);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| Ui::default().draw(f, &app)).unwrap();
                let contents: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                if width >= 22 {
                    assert!(contents.contains(if indent {
                        "INDENT BLOCK"
                    } else {
                        "OUTDENT BLOCK"
                    }));
                    assert!(contents.contains("y apply"));
                    assert!(contents.contains("n/Esc cancel"));
                }
                if width >= 45 {
                    assert!(contents.contains("New block ID"));
                    assert!(contents.contains("Destination"));
                    assert!(contents.contains("Hello"));
                }
            }
        }
    }

    #[test]
    fn deletion_prompt_shows_target_warning_and_cancel_controls_at_narrow_widths() {
        for (width, height) in [(120, 35), (45, 20), (22, 12), (16, 7), (10, 4), (1, 1)] {
            for block_type in ["paragraph", "child_page", "child_database", "toggle"] {
                let (tx, _) = mpsc::unbounded_channel();
                let mut app = App::new(Arc::new(DemoService), tx, true);
                let mut block = crate::model::tests::paragraph();
                block.block_type = block_type.into();
                block.children = block_type == "toggle";
                block.child_page = block_type == "child_page";
                block.text = "你好 🦀 preview\nsecond line ".repeat(50);
                app.pending_delete = Some(crate::app::PendingDelete {
                    block,
                    parent: "page".into(),
                    index: 0,
                });
                app.confirm = Some(Confirm::Delete);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| Ui::default().draw(f, &app)).unwrap();
                let contents: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                if width >= 22 {
                    assert!(contents.contains("DELETE BLOCK"));
                    assert!(contents.contains(block_type));
                    assert!(contents.contains('你') && contents.contains('好'));
                    assert!(contents.contains("y delete"));
                    assert!(contents.contains("n/Esc cancel"));
                    if block_type != "paragraph" {
                        assert!(contents.contains("Includes contents."));
                    }
                }
            }
        }
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
    fn parent_scroll_survives_child_navigation_loading_and_resize() {
        use crate::app::{Location, LocationKind};
        let (tx, _) = mpsc::unbounded_channel();
        let mut app = App::new(Arc::new(DemoService), tx, true);
        app.focus = Focus::Content;
        app.path.push(Location {
            id: "parent".into(),
            title: "Parent".into(),
            kind: LocationKind::Page,
            page_id: "parent".into(),
            list_depth: 0,
        });
        app.blocks = (0..20)
            .map(|index| {
                crate::model::BlockItem::from_value(&serde_json::json!({
                    "id": format!("block-{index}"), "type": "paragraph",
                    "paragraph": {"rich_text": [{"type": "text", "plain_text": format!("parent item{index}")}]}
                }))
            })
            .collect();
        let parent_blocks = app.blocks.clone();
        let mut ui = Ui::default();
        let mut terminal = Terminal::new(TestBackend::new(50, 12)).unwrap();
        app.block_selected = 14;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        // Move up within the viewport so scrolling from the top would produce a different view.
        app.block_selected = 12;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        let saved_offset = ui.block_offsets["parent"];
        let saved_buffer = terminal.backend().buffer().clone();
        assert!(saved_offset > 0);

        app.path.push(Location {
            id: "child".into(),
            title: "Child".into(),
            kind: LocationKind::Blocks,
            page_id: "parent".into(),
            list_depth: 0,
        });
        app.blocks.clear();
        app.block_selected = 0;
        app.loading_content = true;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert_eq!(ui.block_offsets["parent"], saved_offset);
        app.blocks = parent_blocks.clone();
        app.loading_content = false;
        app.block_selected = 14;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert!(ui.block_offsets["child"] > 0);

        app.path.pop();
        app.blocks.clear();
        app.block_selected = 0;
        app.loading_content = true;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert_eq!(ui.block_offsets["parent"], saved_offset);
        app.loading_content = false;
        app.error = true;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert_eq!(ui.block_offsets["parent"], saved_offset);

        app.blocks = parent_blocks;
        app.block_selected = 12;
        app.error = false;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert_eq!(ui.block_offsets["parent"], saved_offset);
        assert_eq!(*terminal.backend().buffer(), saved_buffer);

        let mut smaller = Terminal::new(TestBackend::new(25, 6)).unwrap();
        smaller
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert!(ui.block_offsets["parent"] > saved_offset);
        let contents: String = smaller
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(contents.contains("parent item12"), "{contents}");

        app.path.last_mut().unwrap().id = "child".into();
        app.block_selected = 0;
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert_eq!(ui.block_offsets["child"], 0, "new entries start at the top");
        app.blocks.clear();
        terminal
            .draw(|frame| ui.content(frame, &app, frame.area()))
            .unwrap();
        assert!(!ui.block_offsets.contains_key("child"));
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
