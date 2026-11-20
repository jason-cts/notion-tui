use super::*;
use crate::{
    app::ImagePreview,
    image_preview::{self, Art, View},
};

#[derive(Clone, Copy, Default)]
pub(super) enum ImageColor {
    #[default]
    None,
    Ansi,
    Indexed,
    Rgb,
}

impl ImageColor {
    fn detect(no_color: bool) -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        if no_color || term == "dumb" {
            return Self::None;
        }
        match crossterm::style::available_color_count() {
            0..=7 => Self::None,
            8..=255 if term.is_empty() || matches!(term.as_str(), "vt100" | "vt102" | "vt220") => {
                Self::None
            }
            8..=255 => Self::Ansi,
            256 => Self::Indexed,
            _ => Self::Rgb,
        }
    }

    pub(super) fn foreground(self, [r, g, b]: [u8; 3]) -> Option<Color> {
        Some(match self {
            Self::None => return None,
            Self::Rgb => Color::Rgb(r, g, b),
            Self::Indexed => {
                let level = |value| match value {
                    0..=47 => 0,
                    48..=114 => 1,
                    115..=154 => 2,
                    155..=194 => 3,
                    195..=234 => 4,
                    _ => 5,
                };
                Color::Indexed(16 + 36 * level(r) + 6 * level(g) + level(b))
            }
            Self::Ansi => {
                let colors = [
                    Color::Black,
                    Color::Red,
                    Color::Green,
                    Color::Yellow,
                    Color::Blue,
                    Color::Magenta,
                    Color::Cyan,
                    Color::Gray,
                ];
                let index = usize::from(r >= 128)
                    | (usize::from(g >= 128) << 1)
                    | (usize::from(b >= 128) << 2);
                colors[index]
            }
        })
    }
}

pub(super) struct CachedArt {
    key: (u64, View),
    output: Result<Art, String>,
}

fn popup_areas(screen: Rect) -> (Rect, Rect) {
    let area = popup(screen, 164, screen.height.saturating_sub(2).max(5));
    let inner = if area.width < 4 || area.height < 4 {
        area
    } else {
        area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        })
    };
    (area, inner)
}

fn art_area(inner: Rect) -> Rect {
    let header = inner.height.min(2);
    Rect {
        y: inner.y + header,
        height: inner.height - header,
        ..inner
    }
}

impl Ui {
    pub fn with_image_color(no_color: bool) -> Self {
        Self {
            image_color: ImageColor::detect(no_color),
            ..Self::default()
        }
    }

    pub fn resize_image_preview(app: &mut App, screen: Rect) {
        if let Some(preview) = &mut app.image_preview {
            let (_, inner) = popup_areas(screen);
            let art = art_area(inner);
            let image = preview
                .result
                .as_ref()
                .and_then(|result| result.as_ref().ok());
            preview.viewport.resize(art.width, art.height, image);
        }
    }

    pub(super) fn image_modal(&mut self, frame: &mut Frame, preview: &ImagePreview) {
        let (area, inner) = popup_areas(frame.area());
        frame.render_widget(Clear, area);
        if inner != area {
            frame.render_widget(
                panel(
                    "IMAGE · +/= zoom in · - out · arrows pan · o open · Esc close".into(),
                    true,
                ),
                area,
            );
        }
        if inner.is_empty() {
            return;
        }

        let mut link = RichRun::plain(&preview.url);
        link.url = Some(preview.url.clone());
        if let Some(line) = rich_text::wrap(&[link], inner.width).first() {
            self.rich_row(frame, line, Rect { height: 1, ..inner }, Style::default());
        }
        if inner.height < 2 {
            return;
        }

        let status = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        let image = match &preview.result {
            Some(Ok(image)) => image,
            result => {
                let text = match result {
                    Some(Err(error)) => clean(error),
                    _ => "Loading image…".into(),
                };
                frame.render_widget(
                    Paragraph::new(text).wrap(Wrap { trim: false }),
                    Rect {
                        height: inner.height - 1,
                        ..status
                    },
                );
                return;
            }
        };
        let view = preview.viewport.view(image);
        let scale = image.pixels_per_character(view);
        frame.render_widget(
            Paragraph::new(format!("{scale:.2} pixels per character"))
                .style(Style::default().fg(MUTED)),
            status,
        );
        let art = art_area(inner);
        if art.is_empty() || view.width == 0 || view.height == 0 {
            return;
        }

        let key = (preview.request, view);
        if self
            .image_art
            .as_ref()
            .is_none_or(|cached| cached.key != key)
        {
            let output = image_preview::render(image, view)
                .map_err(|e| format!("Could not render image to ASCII ({e})"));
            self.image_art = Some(CachedArt { key, output });
        }
        match &self.image_art.as_ref().unwrap().output {
            Ok(output) => draw_art(frame, art, output, self.image_color),
            Err(error) => frame.render_widget(Paragraph::new(clean(error)), art),
        }
    }
}

fn draw_art(frame: &mut Frame, area: Rect, art: &Art, color: ImageColor) {
    let width = usize::from(area.width).min(art.pixels.width().saturating_sub(1) as usize);
    let buffer = frame.buffer_mut();
    for (y, line) in art
        .glyphs
        .lines()
        .take(usize::from(area.height))
        .enumerate()
    {
        for (x, glyph) in line.bytes().take(width).enumerate() {
            let cell = &mut buffer[(area.x + x as u16, area.y + y as u16)];
            cell.set_char(char::from(glyph));
            if let Some(fg) = color.foreground(art.pixels.get_pixel(x as u32, y as u32).0) {
                cell.set_fg(fg);
            }
        }
    }
}
