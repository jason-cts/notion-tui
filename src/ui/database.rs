use super::*;
use crate::database::{Column, DatabaseData, DatabaseRow, DatabaseState};
use rusticnotion::chrono::Local;

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
        if data
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
        if columns.is_empty() {
            self.database_list(frame, data, db.selected, area, key);
            return;
        }
        let requested_frozen = data
            .view
            .configuration
            .as_ref()
            .and_then(|c| c.frozen_column_index)
            .unwrap_or(0)
            .min(columns.len().saturating_sub(1));
        // Release frozen columns on small terminals so every property remains
        // reachable. Keep room for at least one scrollable property.
        let mut frozen = 0;
        let mut frozen_width = 0u16;
        for column in columns.iter().take(requested_frozen) {
            if frozen_width
                .saturating_add(column.width + 1)
                .saturating_add(8)
                > area.width
            {
                break;
            }
            frozen_width += column.width + 1;
            frozen += 1;
        }
        let mut visible: Vec<(&Column, Rect)> = vec![];
        let mut x = area.x;
        let start = (frozen + db.horizontal).min(columns.len() - 1);
        for index in (0..frozen).chain(start..columns.len()) {
            if x >= area.right() {
                break;
            }
            let c = &columns[index];
            let reserve = if index < frozen {
                8.min(area.width / 2)
            } else {
                0
            };
            let width = c
                .width
                .min(area.right().saturating_sub(x).saturating_sub(reserve))
                .max(1);
            visible.push((c, Rect::new(x, area.y, width, 1)));
            x = x.saturating_add(width + 1);
        }
        for (c, rect) in &visible {
            frame.render_widget(
                Paragraph::new(clean(&c.name))
                    .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                *rect,
            );
        }
        let content = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        let groups = data.groups(Local::now().date_naive());
        let grouped = data
            .view
            .configuration
            .as_ref()
            .is_some_and(|c| c.group_by.is_some());
        let rows: Vec<_> = groups
            .iter()
            .flat_map(|g| {
                g.cards
                    .iter()
                    .map(move |card| (card.row, if grouped { g.label.as_str() } else { "" }))
            })
            .collect();
        let target = rows
            .iter()
            .position(|(i, _)| *i == db.selected)
            .unwrap_or(0);
        let heights: Vec<_> = rows
            .iter()
            .map(|(i, _)| {
                visible
                    .iter()
                    .map(|(c, r)| cell_lines(data, &data.rows[*i], c, r.width).len())
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
                            !rows[*i].1.is_empty()
                                && (*i == *offset || rows[*i].1 != rows[*i - 1].1)
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
        let mut last_group = "";
        for (position, (row_index, group)) in rows.iter().enumerate().skip(offset) {
            if y >= content.bottom() {
                break;
            }
            if *group != last_group && content.height > 1 {
                frame.render_widget(
                    Paragraph::new(clean(group)).style(Style::default().fg(MUTED)),
                    Rect::new(area.x, y, area.width, 1),
                );
                last_group = group;
                y += 1;
            }
            let style = Style::default()
                .bg(if *row_index == db.selected {
                    Color::Rgb(43, 53, 66)
                } else {
                    PANEL
                })
                .fg(INK);
            let height =
                heights[position].min(usize::from(content.bottom().saturating_sub(y))) as u16;
            frame.render_widget(
                Block::default().style(style),
                Rect::new(area.x, y, area.width, height),
            );
            for (c, rect) in &visible {
                for (line_index, line) in cell_lines(data, &data.rows[*row_index], c, rect.width)
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
                if rect.right() < area.right()
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
