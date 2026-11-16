use super::*;
use crate::database::{Column, DatabaseData, DatabaseRow, DatabaseState};
use rusticnotion::chrono::Local;

struct TableLayout {
    icon_width: u16,
    columns: Vec<(usize, Rect)>,
    left: Option<u16>,
    right: Option<u16>,
    hidden_left: bool,
    hidden_right: bool,
}

/// Geometry is derived from focus so resize and cursor changes always agree.
fn table_layout(data: &DatabaseData, cell: usize, area: Rect) -> TableLayout {
    let columns = data.columns();
    let icon_width = area.width.min(3);
    let remaining = area.width.saturating_sub(icon_width);
    let mut layout = TableLayout {
        icon_width,
        columns: vec![],
        left: None,
        right: None,
        hidden_left: false,
        hidden_right: false,
    };
    if remaining < 3 {
        return layout;
    }
    layout.left = Some(area.x + icon_width);
    layout.right = Some(area.right() - 1);
    let width = remaining - 2;
    let requested_frozen = data
        .view
        .configuration
        .as_ref()
        .and_then(|c| c.frozen_column_index)
        .unwrap_or(0)
        .min(columns.len().saturating_sub(1));
    let mut frozen = 0;
    let mut frozen_width = 0u16;
    for c in columns.iter().take(requested_frozen) {
        if frozen_width.saturating_add(c.width + 1).saturating_add(8) > width {
            break;
        }
        frozen += 1;
        frozen_width += c.width + 1;
    }
    let mut start = frozen;
    if let Some(focus) = cell
        .checked_sub(1)
        .filter(|i| *i >= frozen && *i < columns.len())
    {
        while start < focus
            && columns[start..=focus]
                .iter()
                .map(|c| u32::from(c.width) + 1)
                .sum::<u32>()
                .saturating_sub(1)
                > u32::from(width - frozen_width)
        {
            start += 1;
        }
    }
    layout.hidden_left = start > frozen;
    let mut x = area.x + icon_width + 1;
    let right = area.right() - 1;
    for index in (0..frozen).chain(start..columns.len()) {
        if x >= right {
            break;
        }
        let w = columns[index].width.min(right - x);
        layout.columns.push((index, Rect::new(x, area.y, w, 1)));
        x = x.saturating_add(w + 1);
    }
    layout.hidden_right = layout
        .columns
        .last()
        .is_some_and(|(i, r)| *i + 1 < columns.len() || r.width < columns[*i].width);
    layout
}

pub(super) fn render_cell_editor(frame: &mut Frame, editor: &Editor, saving: bool, cursor: bool) {
    let cell = editor.cell.as_ref().unwrap();
    let Target::DatabaseCell(target) = &editor.target else {
        return;
    };
    let screen = frame.area();
    let area = if screen.height < 10 {
        screen
    } else {
        popup(screen, 100, screen.height.saturating_sub(2).min(30))
    };
    frame.render_widget(Clear, area);
    let block = panel(
        format!("EDIT CELL · {} · {}", clean(&target.column.name), cell.kind),
        true,
    );
    let inner = if area.width >= 5 && area.height >= 5 {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    } else {
        area
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let footer_height = inner.height.min(if editor.error.is_some() { 4 } else { 2 });
    let body = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(footer_height),
    );
    if cell.is_picker() {
        let items: Vec<_> = cell
            .choices
            .iter()
            .zip(&cell.checked)
            .map(|((label, _), checked)| {
                ListItem::new(format!(
                    "{} {}",
                    if *checked { "☑" } else { "☐" },
                    clean(label)
                ))
            })
            .collect();
        if items.is_empty() {
            frame.render_widget(Paragraph::new("No existing options"), body);
        } else {
            let mut state = ListState::default().with_selected(Some(cell.cursor));
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("▌ ")
                    .highlight_style(Style::default().bg(Color::Rgb(43, 53, 66))),
                body,
                &mut state,
            );
        }
    } else if cell.kind == "date" {
        let labels = [
            "Start · YYYY-MM-DD or ISO timestamp",
            "End · optional",
            "Timezone · optional IANA name",
        ];
        let field_height = (body.height / 3).max(2);
        let offset = (cell.active as u16 * field_height + field_height).saturating_sub(body.height);
        for (i, field) in cell.fields.iter().enumerate() {
            let top = i as u16 * field_height;
            if top < offset || top - offset >= body.height {
                continue;
            }
            let y = body.y + top - offset;
            frame.render_widget(
                Paragraph::new(labels[i]).style(Style::default().fg(if i == cell.active {
                    ACCENT
                } else {
                    MUTED
                })),
                Rect::new(body.x, y, body.width, 1),
            );
            if y + 1 < body.bottom() {
                render_buffer(
                    frame,
                    field,
                    Rect::new(
                        body.x,
                        y + 1,
                        body.width,
                        field_height.saturating_sub(1).min(body.bottom() - y - 1),
                    ),
                    cursor && !saving && i == cell.active,
                );
            }
        }
    } else {
        render_buffer(
            frame,
            &cell.fields[0],
            body,
            cursor && !saving && editor.style_picker.is_none(),
        );
    }
    let hint = if saving {
        "Saving… please wait"
    } else if cell.is_picker() {
        "↑/↓ choose · Space/Enter select · Ctrl-S save · Esc cancel"
    } else if cell.kind == "date" {
        "Tab fields · Ctrl-D clear · Ctrl-S save · Esc cancel"
    } else if cell.fields[0].rich.is_none() {
        "Ctrl-S save · Esc cancel · empty value clears"
    } else {
        "Ctrl-S save · Esc cancel · Shift+F10 styles · empty value clears"
    };
    frame.render_widget(
        Paragraph::new(style_status(
            &cell.fields[0],
            clean(editor.error.as_deref().unwrap_or(hint)),
        ))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(if editor.error.is_some() {
            ERROR
        } else {
            ACCENT
        })),
        Rect::new(inner.x, body.bottom(), inner.width, footer_height),
    );
}

impl Ui {
    pub(super) fn database_content(&mut self, frame: &mut Frame, app: &App, area: Rect) {
        let db = app.database().unwrap();
        let view = db.view();
        let title = format!(
            "CONTENT · child_database ({}) · {}",
            view.map(|v| v.view_type.as_str()).unwrap_or("…"),
            app.location()
                .map(|l| l.title.as_str())
                .unwrap_or("Loading database")
        );
        let block = panel(title, app.focus == Focus::Content);
        let mut inner = block.inner(area);
        frame.render_widget(block, area);
        let tabs = db
            .meta
            .as_ref()
            .map(|m| {
                m.views
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        if i == db.view_index {
                            format!("[{} ({})]", v.name, v.view_type)
                        } else {
                            format!("{} ({})", v.name, v.view_type)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            })
            .unwrap_or_default();
        // Keep the active tab visible when a database has many views.
        let tabs = if UnicodeWidthStr::width(tabs.as_str()) > usize::from(inner.width) {
            format!(
                "[ / ]  {} ({}/{})",
                view.map(|v| v.name.as_str()).unwrap_or_default(),
                db.view_index + 1,
                db.meta.as_ref().map_or(0, |m| m.views.len())
            )
        } else {
            tabs
        };
        frame.render_widget(
            Paragraph::new(clean(&tabs)).style(Style::default().fg(ACCENT)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        inner.y += 1;
        inner.height = inner.height.saturating_sub(1);
        if app.loading_content {
            frame.render_widget(
                Paragraph::new(format!("\n  Loading database… {} rows", db.progress)),
                inner,
            );
            return;
        }
        if let Some(error) = &db.error {
            frame.render_widget(
                Paragraph::new(clean(error))
                    .style(Style::default().fg(ERROR))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }
        let Some(data) = &db.data else { return };
        if !data.notices.is_empty() {
            frame.render_widget(
                Paragraph::new(clean(&data.notices.join(" · "))).style(Style::default().fg(MUTED)),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            inner.y += 1;
            inner.height = inner.height.saturating_sub(1);
        }
        if data.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "\n  No rows match this saved view.\n  [ / ] switch views · r refresh",
                ),
                inner,
            );
            return;
        }
        let key = format!("{}:{}", app.location().unwrap().id, data.view.id);
        if data.view.view_type != "table"
            && data
                .notices
                .iter()
                .any(|n| n.starts_with("Unsupported grouping"))
        {
            self.database_list(frame, data, db.selected, inner, &key);
            return;
        }
        match data.view.view_type.as_str() {
            "board" => self.database_board(frame, data, db, inner, &key),
            "table" => self.database_table(frame, data, db, inner, &key),
            _ => self.database_list(frame, data, db.selected, inner, &key),
        }
    }

    fn rich_row(&mut self, frame: &mut Frame, line: &RichLine, area: Rect, style: Style) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        frame.render_widget(Paragraph::new(line.line()).style(style), area);
        hyperlinks::record(&mut self.links, line, area);
    }

    fn database_list(
        &mut self,
        frame: &mut Frame,
        data: &DatabaseData,
        selected: usize,
        area: Rect,
        key: &str,
    ) {
        let rows: Vec<_> = data
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let mut runs = title_runs(data, row);
                for c in data.columns().iter().filter(|c| {
                    row.page["properties"]
                        .as_object()
                        .and_then(|p| p.get(&c.name))
                        .is_none_or(|p| p["type"] != "title")
                }) {
                    let cell = data.cell(row, c);
                    if cell.iter().any(|r| !r.text.is_empty()) {
                        runs.push(RichRun::plain(format!("  · {}: ", c.name)));
                        runs.extend(cell);
                    }
                }
                (index, rich_text::wrap(&runs, area.width.saturating_sub(2)))
            })
            .collect();
        self.database_lines(frame, &rows, selected, area, key);
    }

    fn database_lines(
        &mut self,
        frame: &mut Frame,
        rows: &[(usize, Vec<RichLine>)],
        selected: usize,
        area: Rect,
        key: &str,
    ) {
        if area.height == 0 {
            return;
        }
        let target = rows.iter().position(|(id, _)| *id == selected).unwrap_or(0);
        let offset = self.database_offsets.entry(key.into()).or_default();
        *offset = (*offset).min(target);
        while *offset < target
            && rows[*offset..=target]
                .iter()
                .map(|(_, r)| r.len() + 1)
                .sum::<usize>()
                > usize::from(area.height)
        {
            *offset += 1;
        }
        let offset = *offset;
        let mut y = area.y;
        for (id, lines) in rows.iter().skip(offset) {
            let selected = *id == selected;
            let style = Style::default().fg(INK).bg(if selected {
                Color::Rgb(43, 53, 66)
            } else {
                PANEL
            });
            for (i, line) in lines.iter().enumerate() {
                if y >= area.bottom() {
                    return;
                }
                frame.render_widget(
                    Paragraph::new(if selected && i == 0 { "▌" } else { " " }).style(style),
                    Rect::new(area.x, y, area.width, 1),
                );
                self.rich_row(
                    frame,
                    line,
                    Rect::new(area.x + 2, y, area.width.saturating_sub(2), 1),
                    style,
                );
                y += 1;
            }
            y += 1;
        }
    }

    fn database_board(
        &mut self,
        frame: &mut Frame,
        data: &DatabaseData,
        db: &DatabaseState,
        area: Rect,
        key: &str,
    ) {
        let groups = data.groups(Local::now().date_naive());
        if groups.is_empty() {
            return;
        }
        // Fit more workflow columns before falling back to horizontal scrolling.
        // A fixed width of 30 left room unused when three columns could fit.
        let count = usize::from((area.width / 24).max(1)).min(groups.len());
        let width = (area.width / count as u16).clamp(1, 30);
        let first = self
            .database_offsets
            .entry(format!("{key}:columns"))
            .or_default();
        *first = (*first).min(db.column);
        if db.column >= *first + count {
            *first = db.column + 1 - count;
        }
        let first = *first;
        for (position, group) in groups.iter().enumerate().skip(first).take(count) {
            let x = area.x + ((position - first) as u16) * width;
            let column = Rect::new(
                x,
                area.y,
                width.min(area.right().saturating_sub(x)),
                area.height,
            );
            let block = panel(
                format!("{} ({})", clean(&group.label), group.cards.len()),
                position == db.column,
            );
            let inner = if column.height < 4 {
                if column.height > 1 {
                    frame.render_widget(
                        Paragraph::new(format!("{} ({})", clean(&group.label), group.cards.len()))
                            .style(Style::default().fg(ACCENT)),
                        Rect::new(column.x, column.y, column.width, 1),
                    );
                    Rect::new(column.x, column.y + 1, column.width, column.height - 1)
                } else {
                    column
                }
            } else {
                let inner = block.inner(column);
                frame.render_widget(block, column);
                inner
            };
            let mut rows = vec![];
            if group.subgroups.is_empty() {
                for (index, card) in group.cards.iter().enumerate() {
                    rows.push((index, card_lines(data, &data.rows[card.row], inner.width)));
                }
            } else {
                for (key, label) in &group.subgroups {
                    let count = group
                        .cards
                        .iter()
                        .filter(|c| c.subgroup_key == *key)
                        .count();
                    rows.push((
                        usize::MAX - 1,
                        rich_text::wrap(
                            &[RichRun::plain(format!("── {label} ({count}) ──"))],
                            inner.width.saturating_sub(2),
                        ),
                    ));
                    for (index, card) in group
                        .cards
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.subgroup_key == *key)
                    {
                        rows.push((index, card_lines(data, &data.rows[card.row], inner.width)));
                    }
                }
            }
            self.database_lines(
                frame,
                &rows,
                if position == db.column {
                    db.card
                } else {
                    usize::MAX
                },
                inner,
                &format!("{key}:{}", group.key),
            );
        }
    }

    fn database_table(
        &mut self,
        frame: &mut Frame,
        data: &DatabaseData,
        db: &DatabaseState,
        area: Rect,
        key: &str,
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let columns = data.columns();
        let layout = table_layout(data, db.table_cell, area);
        let visible = &layout.columns;
        let header_style = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        for (index, rect) in visible {
            frame.render_widget(
                Paragraph::new(clean(&columns[*index].name)).style(header_style),
                *rect,
            );
        }
        if let Some(x) = layout.left {
            frame.render_widget(
                Paragraph::new(if layout.hidden_left { "<" } else { " " }).style(header_style),
                Rect::new(x, area.y, 1, 1),
            );
        }
        if let Some(x) = layout.right {
            frame.render_widget(
                Paragraph::new(if layout.hidden_right { ">" } else { " " }).style(header_style),
                Rect::new(x, area.y, 1, 1),
            );
        }
        let content = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        let rows = data.table_rows();
        let target = db.table_row.min(rows.len().saturating_sub(1));
        let heights: Vec<_> = rows
            .iter()
            .map(|r| {
                visible
                    .iter()
                    .map(|(i, rect)| {
                        cell_lines(data, &data.rows[r.row], &columns[*i], rect.width).len()
                    })
                    .max()
                    .unwrap_or(1)
            })
            .collect();
        let offset = self
            .database_offsets
            .entry(format!("{key}:rows"))
            .or_default();
        *offset = (*offset).min(target);
        while *offset < target
            && heights[*offset..=target].iter().sum::<usize>()
                + (if content.height > 1 {
                    (*offset..=target)
                        .filter(|i| {
                            !rows[*i].label.is_empty()
                                && (*i == *offset || rows[*i].group != rows[*i - 1].group)
                        })
                        .count()
                } else {
                    0
                })
                > usize::from(content.height)
        {
            *offset += 1;
        }
        let offset = *offset;
        let mut y = content.y;
        let mut last_group = None;
        let normal = Style::default().bg(PANEL).fg(INK);
        let selected = Style::default().bg(Color::Rgb(43, 53, 66)).fg(INK);
        for (position, row) in rows.iter().enumerate().skip(offset) {
            if y >= content.bottom() {
                break;
            }
            if !row.label.is_empty() && last_group != Some(&row.group) && content.height > 1 {
                frame.render_widget(
                    Paragraph::new(clean(&row.label)).style(Style::default().fg(MUTED)),
                    Rect::new(area.x, y, area.width, 1),
                );
                last_group = Some(&row.group);
                y += 1;
            }
            let height =
                heights[position].min(usize::from(content.bottom().saturating_sub(y))) as u16;
            frame.render_widget(
                Block::default().style(normal),
                Rect::new(area.x, y, area.width, height),
            );
            let icon_style = if position == target && db.table_cell == 0 {
                selected
            } else {
                normal
            };
            let icon_rect = Rect::new(area.x, y, layout.icon_width, height);
            frame.render_widget(Block::default().style(icon_style), icon_rect);
            let icon = data.rows[row.row].page["icon"]["emoji"]
                .as_str()
                .unwrap_or("📄");
            frame.render_widget(
                Paragraph::new(clean(icon)).style(icon_style),
                Rect::new(area.x, y, layout.icon_width, height.min(1)),
            );
            for (index, rect) in visible {
                let style = if position == target && db.table_cell == index + 1 {
                    selected
                } else {
                    normal
                };
                frame.render_widget(
                    Block::default().style(style),
                    Rect::new(rect.x, y, rect.width, height),
                );
                for (line_index, line) in
                    cell_lines(data, &data.rows[row.row], &columns[*index], rect.width)
                        .iter()
                        .take(usize::from(height))
                        .enumerate()
                {
                    self.rich_row(
                        frame,
                        line,
                        Rect::new(rect.x, y + line_index as u16, rect.width, 1),
                        style,
                    );
                }
                if rect.right() < area.right().saturating_sub(1)
                    && data
                        .view
                        .configuration
                        .as_ref()
                        .is_none_or(|c| c.show_vertical_lines != Some(false))
                {
                    for dy in 0..height {
                        frame.render_widget(
                            Paragraph::new("│").style(Style::default().fg(MUTED)),
                            Rect::new(rect.right(), y + dy, 1, 1),
                        );
                    }
                }
            }
            y += height;
        }
    }
}

fn title_runs(data: &DatabaseData, row: &DatabaseRow) -> Vec<RichRun> {
    let title = row.page["properties"]
        .as_object()
        .and_then(|p| p.iter().find(|(_, p)| p["type"] == "title"));
    title
        .map(|(name, p)| {
            data.cell(
                row,
                &Column {
                    id: p["id"].as_str().unwrap_or("title").into(),
                    name: name.clone(),
                    width: 20,
                    wrap: true,
                },
            )
        })
        .filter(|runs| runs.iter().any(|r| !r.text.is_empty()))
        .unwrap_or_else(|| vec![RichRun::plain(row.title.clone())])
}
fn card_lines(data: &DatabaseData, row: &DatabaseRow, width: u16) -> Vec<RichLine> {
    let mut runs = title_runs(data, row);
    for c in data.columns() {
        if crate::database::property(row, &c.id, &c.name)["type"] == "title" {
            continue;
        }
        let cell = data.cell(row, &c);
        if !cell.iter().any(|r| !r.text.is_empty()) {
            continue;
        }
        runs.push(RichRun::plain(format!("\n{}: ", c.name)));
        runs.extend(cell);
    }
    rich_text::wrap(&runs, width.saturating_sub(2))
}
fn cell_lines(data: &DatabaseData, row: &DatabaseRow, c: &Column, width: u16) -> Vec<RichLine> {
    let lines = rich_text::wrap(&data.cell(row, c), width);
    if c.wrap {
        lines
    } else {
        lines.into_iter().take(1).collect()
    }
}

#[cfg(test)]
mod tests;
