use std::{io::Cursor, time::Duration};

use anyhow::{Context, Result, bail};
use crossterm::event::KeyCode;
use image::{DynamicImage, Rgb, RgbImage, imageops::sample_bilinear, io::Reader};

const MAX_DOWNLOAD: usize = 10 * 1024 * 1024;

pub struct PreviewImage {
    width: u32,
    height: u32,
    thumbnail: RgbImage,
    contrast_reference: Rgb<u8>,
}

impl PreviewImage {
    fn new(image: DynamicImage) -> Self {
        let (width, height) = (image.width(), image.height());
        let thumbnail = if width.max(height) > 648 {
            image.thumbnail(648, 648).into_rgb8()
        } else {
            image.into_rgb8()
        };
        let brightest = thumbnail.pixels().map(luminance).fold(0.0, f64::max);
        // Bilinear sampling can round each RGB channel upward by half a unit.
        let contrast_reference = Rgb([((brightest + 0.5).ceil().min(255.0)) as u8; 3]);
        Self {
            width,
            height,
            thumbnail,
            contrast_reference,
        }
    }

    pub fn pixels_per_character(&self, view: View) -> f64 {
        f64::from(self.width) / f64::from(view.columns)
    }
}

fn luminance(pixel: &Rgb<u8>) -> f64 {
    let [r, g, b] = pixel.0.map(f64::from);
    r * 0.299 + g * 0.587 + b * 0.114
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub columns: u32,
    pub rows: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
pub struct Viewport {
    width: u32,
    height: u32,
    zoom: i32,
    x: u32,
    y: u32,
}

impl Viewport {
    pub fn resize(&mut self, width: u16, height: u16, image: Option<&PreviewImage>) {
        self.width = u32::from(width);
        self.height = u32::from(height);
        if let Some(image) = image {
            self.clamp(image);
        }
    }

    pub fn key(&mut self, key: KeyCode, image: &PreviewImage) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        match key {
            KeyCode::Char('+' | '=') => self.zoom += 1,
            KeyCode::Char('-') => self.zoom -= 1,
            KeyCode::Left => self.x = self.x.saturating_sub(1),
            KeyCode::Right => self.x += 1,
            KeyCode::Up => self.y = self.y.saturating_sub(1),
            KeyCode::Down => self.y += 1,
            _ => return,
        }
        self.clamp(image);
    }

    pub fn view(&self, image: &PreviewImage) -> View {
        let columns = self.columns();
        // Terminal characters are approximately twice as tall as they are wide.
        let rows = (columns * image.height / (2 * image.width)).max(1);
        let width = self.width.min(columns);
        let height = self.height.min(rows);
        View {
            columns,
            rows,
            x: self.x.min(columns.saturating_sub(width.max(1))),
            y: self.y.min(rows.saturating_sub(height.max(1))),
            width,
            height,
        }
    }

    fn columns(&self) -> u32 {
        let fit = self.width.max(1);
        (fit as i32 + self.zoom).clamp(fit.div_ceil(4) as i32, (fit * 4) as i32) as u32
    }

    fn clamp(&mut self, image: &PreviewImage) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let view = self.view(image);
        self.zoom = view.columns as i32 - self.width as i32;
        self.x = view.x;
        self.y = view.y;
    }
}

pub async fn load(url: &str) -> Result<PreviewImage> {
    let mut response = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url())?
        .error_for_status()
        .map_err(|e| e.without_url())?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD as u64)
    {
        bail!("Image exceeds the 10 MiB preview limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.without_url())? {
        if bytes.len() + chunk.len() > MAX_DOWNLOAD {
            bail!("Image exceeds the 10 MiB preview limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || -> Result<_> {
        let mut reader = Reader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = image::io::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        reader.limits(limits);
        let image = reader
            .decode()
            .context("Could not decode image for preview")?;
        anyhow::ensure!(
            image.width() > 0 && image.height() > 0,
            "Image has no pixels"
        );
        Ok(PreviewImage::new(image))
    })
    .await?
}

pub struct Art {
    pub glyphs: String,
    pub pixels: RgbImage,
}

pub fn render(image: &PreviewImage, view: View) -> Result<Art> {
    // RASCII normalizes by the brightest pixel. Include a reference column so
    // moving the viewport never changes the glyph for a pixel already in view.
    let pixels = DynamicImage::ImageRgb8(RgbImage::from_fn(view.width + 1, view.height, |x, y| {
        if x == view.width {
            return image.contrast_reference;
        }
        let u = ((view.x + x) as f32 + 0.5) / view.columns as f32;
        let v = ((view.y + y) as f32 + 0.5) / view.rows as f32;
        sample_bilinear(&image.thumbnail, u, v).unwrap_or(image.contrast_reference)
    }));
    let mut glyphs = String::new();
    rascii_art::render_image_to(
        &pixels,
        &mut glyphs,
        &rascii_art::RenderOptions::new()
            .width(view.width + 1)
            .height(view.height)
            .charset(&[" ", ".", ":", "-", "=", "+", "*", "#", "%", "@"]),
    )?;
    Ok(Art {
        glyphs,
        pixels: pixels.into_rgb8(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_pans_by_one_and_clamps_after_zoom_and_resize() {
        let image = PreviewImage::new(DynamicImage::new_rgb8(24, 48));
        let mut viewport = Viewport::default();
        viewport.resize(8, 3, Some(&image));
        assert_eq!(
            viewport.view(&image),
            View {
                columns: 8,
                rows: 8,
                x: 0,
                y: 0,
                width: 8,
                height: 3
            }
        );
        viewport.key(KeyCode::Right, &image);
        assert_eq!(viewport.view(&image).x, 0);

        viewport.key(KeyCode::Char('='), &image);
        viewport.key(KeyCode::Right, &image);
        viewport.key(KeyCode::Down, &image);
        let view = viewport.view(&image);
        assert_eq!((view.columns, view.x, view.y), (9, 1, 1));
        viewport.key(KeyCode::Left, &image);
        viewport.key(KeyCode::Up, &image);
        assert_eq!((viewport.view(&image).x, viewport.view(&image).y), (0, 0));
        for _ in 0..10 {
            viewport.key(KeyCode::Down, &image);
        }
        assert_eq!(viewport.view(&image).y, 6);

        viewport.resize(10, 12, Some(&image));
        assert_eq!(
            (viewport.view(&image).columns, viewport.view(&image).y),
            (11, 0)
        );
        viewport.key(KeyCode::Char('-'), &image);
        assert_eq!(viewport.view(&image).columns, 10);
        for _ in 0..100 {
            viewport.key(KeyCode::Char('-'), &image);
        }
        assert_eq!(viewport.view(&image).columns, 3);
        for _ in 0..100 {
            viewport.key(KeyCode::Char('+'), &image);
        }
        assert_eq!(viewport.view(&image).columns, 40);
        viewport.resize(3, 3, Some(&image));
        assert_eq!(viewport.view(&image).columns, 12);
        viewport.key(KeyCode::Char('-'), &image);
        assert_eq!(viewport.view(&image).columns, 11);
    }

    #[test]
    fn panning_shifts_glyphs_and_colors_without_rerendering_offscreen_cells() {
        let image = PreviewImage::new(DynamicImage::ImageRgb8(RgbImage::from_fn(
            32,
            24,
            |x, y| Rgb([(x * 8) as u8, (y * 10) as u8, 0]),
        )));
        let mut viewport = Viewport::default();
        viewport.resize(8, 2, Some(&image));
        viewport.key(KeyCode::Char('+'), &image);
        let first = render(&image, viewport.view(&image)).unwrap();
        viewport.key(KeyCode::Right, &image);
        viewport.key(KeyCode::Down, &image);
        let moved = render(&image, viewport.view(&image)).unwrap();
        assert_eq!(moved.pixels.dimensions(), (9, 2)); // Includes the reference column.
        assert_eq!(
            &first.glyphs.lines().nth(1).unwrap()[1..8],
            &moved.glyphs.lines().next().unwrap()[..7]
        );
        for x in 0..7 {
            assert_eq!(
                first.pixels.get_pixel(x + 1, 1),
                moved.pixels.get_pixel(x, 0)
            );
        }
    }
}
