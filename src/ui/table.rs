use super::*;

pub(super) const SELECTED_CELL: Color = Color::Rgb(43, 53, 66);

pub(super) struct GridLayout {
    pub icon_width: u16,
    pub columns: Vec<(usize, Rect)>,
    pub left: Option<u16>,
    pub right: Option<u16>,
    pub hidden_left: bool,
    pub hidden_right: bool,
}

pub(super) fn grid_layout(
    widths: &[u16],
    focus: Option<usize>,
    leading_width: u16,
    requested_frozen: usize,
    area: Rect,
) -> GridLayout {
    let icon_width = area.width.min(leading_width);
    let remaining = area.width - icon_width;
    let mut layout = GridLayout {
        icon_width,
        columns: vec![],
        left: None,
        right: None,
        hidden_left: false,
        hidden_right: false,
    };
    if remaining < 3 {
        if leading_width == 0
            && remaining > 0
            && let Some(index) = focus.filter(|&i| i < widths.len())
        {
            layout
                .columns
                .push((index, Rect::new(area.x, area.y, remaining, 1)));
        }
        return layout;
    }
    layout.left = Some(area.x + icon_width);
    layout.right = Some(area.right() - 1);
    let width = remaining - 2;
    let mut frozen = 0;
    let mut frozen_width = 0u16;
    for &column_width in widths.iter().take(requested_frozen) {
        if frozen_width
            .saturating_add(column_width + 1)
            .saturating_add(8)
            > width
        {
            break;
        }
        frozen += 1;
        frozen_width += column_width + 1;
    }
    let mut start = frozen;
    if let Some(focus) = focus.filter(|&i| i >= frozen && i < widths.len()) {
        let mut needed = widths[start..=focus]
            .iter()
            .map(|&w| u64::from(w) + 1)
            .sum::<u64>()
            - 1;
        while start < focus && needed > u64::from(width - frozen_width) {
            needed -= u64::from(widths[start]) + 1;
            start += 1;
        }
    }
    layout.hidden_left = start > frozen;
    let mut x = area.x + icon_width + 1;
    let right = area.right() - 1;
    for index in (0..frozen).chain(start..widths.len()) {
        if x >= right {
            break;
        }
        let width = widths[index].min(right - x);
        layout.columns.push((index, Rect::new(x, area.y, width, 1)));
        x = x.saturating_add(width + 1);
    }
    layout.hidden_right = layout
        .columns
        .last()
        .is_some_and(|(i, r)| *i + 1 < widths.len() || r.width < widths[*i]);
    layout
}

pub(super) fn column_indicators(frame: &mut Frame, layout: &GridLayout, y: u16, style: Style) {
    for (x, hidden, arrow) in [
        (layout.left, layout.hidden_left, "<"),
        (layout.right, layout.hidden_right, ">"),
    ] {
        if let Some(x) = x
            && hidden
        {
            frame.render_widget(Paragraph::new(arrow).style(style), Rect::new(x, y, 1, 1));
        }
    }
}

impl Ui {
    pub(super) fn draw_table_cell(
        &mut self,
        frame: &mut Frame,
        lines: &[RichLine],
        area: Rect,
        style: Style,
        separator: bool,
    ) {
        frame.render_widget(Block::default().style(style), area);
        for (dy, line) in lines.iter().take(usize::from(area.height)).enumerate() {
            let row = Rect {
                y: area.y + dy as u16,
                height: 1,
                ..area
            };
            self.rich_row(frame, line, row, style);
        }
        if separator {
            for y in area.y..area.bottom() {
                frame.render_widget(
                    Paragraph::new("│").style(Style::default().fg(MUTED)),
                    Rect::new(area.right(), y, 1, 1),
                );
            }
        }
    }

    pub(super) fn simple_table(&mut self, frame: &mut Frame, app: &App, area: Rect) {
        let info = app.table_info().unwrap();
        let columns = app.table_columns();
        let title = if app.blocks.is_empty() || columns == 0 {
            "TABLE".into()
        } else {
            format!(
                "TABLE · Row {}/{} · Column {}/{}",
                app.block_selected + 1,
                app.blocks.len(),
                app.table_column + 1,
                columns
            )
        };
        let panel = panel(title, app.focus == Focus::Content);
        let inner = panel.inner(area);
        frame.render_widget(panel, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let id = &app.location().unwrap().id;
        if app.loading_content || app.blocks.is_empty() {
            if !app.loading_content {
                self.block_offsets.remove(id);
            }
            let text = if app.loading_content {
                "Loading table…"
            } else if app.error {
                "Table unavailable · r refresh"
            } else {
                "No rows · a insert a row"
            };
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            return;
        }
        let balloon = unicode_width::UnicodeWidthStr::width(comments::BALLOON) as u16;
        let has_comments = inner.width > balloon + 8
            && app.blocks.iter().any(|row| app.block_has_comments(&row.id));
        let grid = Rect {
            width: inner.width - if has_comments { balloon } else { 0 },
            ..inner
        };
        let count = columns.max(1);
        let width = ((usize::from(grid.width.saturating_sub(2)) + 1) / count)
            .saturating_sub(1)
            .clamp(8, 20) as u16;
        let layout = grid_layout(&vec![width; columns], Some(app.table_column), 0, 0, grid);
        let row_cells = |row: &crate::model::BlockItem| {
            layout
                .columns
                .iter()
                .map(|(index, rect)| {
                    let runs = row
                        .table_cells
                        .as_ref()
                        .and_then(|cells| cells.get(*index))
                        .map_or(&[][..], |cell| &cell.runs);
                    rich_text::wrap(runs, rect.width)
                })
                .collect::<Vec<_>>()
        };
        let height = |cells: &[Vec<RichLine>]| cells.iter().map(Vec::len).max().unwrap_or(1);
        let selected = app.block_selected.min(app.blocks.len() - 1);
        let offset = self.block_offsets.entry(id.clone()).or_default();
        *offset = (*offset).min(selected);
        let mut height_to_selection = 0;
        for index in (*offset..=selected).rev() {
            height_to_selection += height(&row_cells(&app.blocks[index]));
            if height_to_selection > usize::from(grid.height) {
                *offset = (index + 1).min(selected);
                break;
            }
        }
        let mut y = grid.y;
        for (row_index, row) in app.blocks.iter().enumerate().skip(*offset) {
            if y >= grid.bottom() {
                break;
            }
            self.visible_blocks.push(row.id.clone());
            let cells = row_cells(row);
            let height = height(&cells).min(usize::from(grid.bottom() - y)) as u16;
            for ((column, rect), lines) in layout.columns.iter().zip(&cells) {
                let header = (row_index == 0 && info.has_column_header)
                    || (*column == 0 && info.has_row_header);
                let active = row_index == selected && *column == app.table_column;
                let mut style = Style::default()
                    .bg(if active { SELECTED_CELL } else { PANEL })
                    .fg(INK);
                if header {
                    style = style.fg(ACCENT).add_modifier(Modifier::BOLD);
                }
                self.draw_table_cell(
                    frame,
                    lines,
                    Rect { y, height, ..*rect },
                    style,
                    rect.right() < grid.right().saturating_sub(1),
                );
            }
            if has_comments && app.block_has_comments(&row.id) {
                frame.render_widget(
                    Paragraph::new(comments::BALLOON).style(Style::default().fg(ACCENT)),
                    Rect::new(grid.right(), y, balloon, 1),
                );
            }
            y += height;
        }
        column_indicators(
            frame,
            &layout,
            grid.y,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        );
    }
}
