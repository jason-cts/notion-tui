use super::*;
use crate::app::CommentsModal;

pub(super) const BALLOON: &str = "💬";

pub(super) fn render_comments(frame: &mut Frame, modal: &CommentsModal, saving: bool) {
    let area = popup(
        frame.area(),
        100,
        frame.area().height.saturating_sub(2).max(5),
    );
    frame.render_widget(Clear, area);
    let border = panel("COMMENTS · 💬".into(), true);
    let inner = border.inner(area);
    frame.render_widget(border, area);
    if inner.is_empty() {
        return;
    }
    let controls = if inner.width < 32 || inner.height < 6 {
        "Esc close · r retry"
    } else {
        "↑/↓ choose · PgUp/PgDn scroll · a new · Enter reply · e edit · d delete · r refresh · Esc close"
    };
    let footer = wrapped(controls, inner.width);
    let footer_height = (footer.len() as u16).min(inner.height);
    let footer_area = Rect {
        y: inner.bottom() - footer_height,
        height: footer_height,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(ACCENT)),
        footer_area,
    );
    let available = inner.height.saturating_sub(footer_height);
    if available == 0 {
        return;
    }
    let preview = wrapped_block(&modal.label, inner.width);
    let preview_height = (preview.len() as u16)
        .min(2)
        .min(available.saturating_sub(1));
    for (i, line) in preview.iter().take(usize::from(preview_height)).enumerate() {
        frame.render_widget(
            Paragraph::new(line.line()).style(Style::default().fg(MUTED)),
            Rect {
                y: inner.y + i as u16,
                height: 1,
                ..inner
            },
        );
    }
    let mut body = Rect {
        y: inner.y + preview_height,
        height: available - preview_height,
        ..inner
    };
    let message = if saving {
        Some("Saving comment… please wait")
    } else if modal.loading {
        Some("Loading comments…")
    } else if let Some(error) = &modal.error {
        Some(error.as_str())
    } else if modal.items.is_empty() {
        Some("No comments yet · a adds a comment")
    } else {
        None
    };
    if let Some(message) = message {
        let text = if let Some(notice) = &modal.notice
            && modal.error.is_none()
        {
            format!("{notice}\n{message}")
        } else {
            message.into()
        };
        frame.render_widget(
            Paragraph::new(clean(&text))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(if modal.error.is_some() { ERROR } else { MUTED })),
            body,
        );
        return;
    }
    if let Some(notice) = &modal.notice
        && body.height > 1
    {
        frame.render_widget(
            Paragraph::new(clean(notice)).style(Style::default().fg(ACCENT)),
            Rect { height: 1, ..body },
        );
        body.y += 1;
        body.height -= 1;
    }
    let mut rows: Vec<(usize, RichLine)> = vec![];
    let mut previous_thread = None;
    let mut thread = 0;
    let mut selected_start = 0;
    let mut selected_len = 0;
    for (i, comment) in modal.items.iter().enumerate() {
        let start = rows.len();
        if previous_thread != Some(comment.discussion.as_str()) {
            thread += 1;
            rows.push((
                i,
                RichLine {
                    runs: vec![RichRun::plain(format!("Discussion {thread}"))],
                },
            ));
            previous_thread = Some(comment.discussion.as_str());
        }
        let edited = if comment.modified != comment.created {
            " · edited"
        } else {
            ""
        };
        for line in rich_text::wrap(
            &[RichRun::plain(format!(
                "{} · {}{edited}",
                clean(&comment.author),
                clean(&comment.created)
            ))],
            body.width.saturating_sub(2),
        ) {
            rows.push((i, line));
        }
        for line in rich_text::wrap(&comment.runs, body.width.saturating_sub(2)) {
            rows.push((i, line));
        }
        if comment.attachments > 0 || comment.pending_attachments {
            rows.push((
                i,
                RichLine {
                    runs: vec![RichRun::plain(format!(
                        "[{} attachments{}]",
                        comment.attachments,
                        if comment.pending_attachments {
                            ", uploading"
                        } else {
                            ""
                        }
                    ))],
                },
            ));
        }
        rows.push((i, RichLine::default()));
        if i == modal.selected {
            selected_start = start;
            selected_len = rows.len() - start;
        }
    }
    let offset = selected_start
        + modal
            .scroll
            .min(selected_len.saturating_sub(usize::from(body.height)));
    for (row_index, (i, line)) in rows
        .iter()
        .skip(offset)
        .take(usize::from(body.height))
        .enumerate()
    {
        let selected = *i == modal.selected;
        let style = Style::default().fg(INK).bg(if selected {
            Color::Rgb(43, 53, 66)
        } else {
            PANEL
        });
        let row = Rect {
            y: body.y + row_index as u16,
            height: 1,
            ..body
        };
        frame.render_widget(
            Paragraph::new(if selected { "▌" } else { "" }).style(style),
            row,
        );
        frame.render_widget(
            Paragraph::new(line.line()).style(style),
            Rect {
                x: row.x + 2.min(row.width),
                width: row.width.saturating_sub(2),
                ..row
            },
        );
    }
}

pub(super) fn render_comment_confirmation(frame: &mut Frame, app: &App) {
    let Some(comment) = app.comments.as_ref().and_then(|m| m.items.get(m.selected)) else {
        return;
    };
    let area = popup(frame.area(), 72, 12);
    frame.render_widget(Clear, area);
    let border = panel("DELETE COMMENT".into(), true);
    let inner = border.inner(area);
    frame.render_widget(border, area);
    let controls = wrapped("y delete · n/Esc cancel", inner.width);
    let height = (controls.len() as u16).min(inner.height);
    let body = Rect {
        height: inner.height.saturating_sub(height),
        ..inner
    };
    let text = comment
        .runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>();
    frame.render_widget(
        Paragraph::new(clean(&format!(
            "Delete {}'s comment?\n{text}",
            comment.author
        )))
        .wrap(Wrap { trim: false }),
        body,
    );
    frame.render_widget(
        Paragraph::new(controls).style(Style::default().fg(ACCENT)),
        Rect {
            y: body.bottom(),
            height,
            ..inner
        },
    );
}

#[cfg(test)]
mod tests;
