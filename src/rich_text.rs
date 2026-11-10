use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichRun {
    pub text: String,
    pub style: Style,
    pub url: Option<String>,
}

impl RichRun {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
            url: None,
        }
    }

    pub fn from_value(value: &Value) -> Self {
        let text = value["plain_text"]
            .as_str()
            .or_else(|| value["text"]["content"].as_str())
            .or_else(|| value["equation"]["expression"].as_str())
            .unwrap_or("[mention]");
        let mut run = Self::plain(text);
        for (name, modifier) in [
            ("bold", Modifier::BOLD),
            ("italic", Modifier::ITALIC),
            ("underline", Modifier::UNDERLINED),
            ("strikethrough", Modifier::CROSSED_OUT),
        ] {
            if value["annotations"][name].as_bool() == Some(true) {
                run.style = run.style.add_modifier(modifier);
            }
        }
        run.url = value["href"]
            .as_str()
            .or_else(|| value["text"]["link"]["url"].as_str())
            .and_then(external_url);
        run
    }

    pub fn display_style(&self) -> Style {
        if self.url.is_some() {
            self.style
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED)
        } else {
            self.style
        }
    }
}

pub fn external_url(value: &str) -> Option<String> {
    // OSC delimiters and other controls must never come from page content.
    if value.chars().any(char::is_control) {
        return None;
    }
    let url = reqwest::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| url.into())
}

pub fn external_links(runs: &[RichRun]) -> Vec<(String, String)> {
    let mut links: Vec<(String, String)> = Vec::new();
    for run in runs {
        if let Some(url) = &run.url
            && !links.iter().any(|(_, existing)| existing == url)
        {
            links.push((run.text.clone(), url.clone()));
        }
    }
    links
}

#[derive(Clone, Debug, Default)]
pub struct RichLine {
    pub runs: Vec<RichRun>,
}

impl RichLine {
    pub fn line(&self) -> Line<'static> {
        Line::from(
            self.runs
                .iter()
                .map(|run| Span::styled(run.text.clone(), run.display_style()))
                .collect::<Vec<_>>(),
        )
    }

    fn push(&mut self, text: &str, source: &RichRun) {
        if let Some(last) = self.runs.last_mut()
            && last.style == source.style
            && last.url == source.url
        {
            last.text.push_str(text);
        } else {
            self.runs.push(RichRun {
                text: text.into(),
                style: source.style,
                url: source.url.clone(),
            });
        }
    }
}

impl std::fmt::Display for RichLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for run in &self.runs {
            f.write_str(&run.text)?;
        }
        Ok(())
    }
}

pub fn wrap(runs: &[RichRun], width: u16) -> Vec<RichLine> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    let mut line = RichLine::default();
    let mut used = 0;
    for run in runs {
        let clean: String = run
            .text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        for grapheme in clean.graphemes(true) {
            if grapheme == "\n" {
                lines.push(std::mem::take(&mut line));
                used = 0;
                continue;
            }
            let grapheme = if UnicodeWidthStr::width(grapheme) > width {
                "�"
            } else {
                grapheme
            };
            let size = UnicodeWidthStr::width(grapheme);
            if used + size > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(grapheme, run);
            used += size;
        }
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn annotations_combine_and_link_targets_are_retained() {
        let run = RichRun::from_value(&json!({"type":"text", "plain_text":"Google",
            "text":{"content":"Google", "link":{"url":"https://www.google.com/"}},
            "annotations":{"bold":true,"italic":true,"underline":true,"strikethrough":true}}));
        assert_eq!(run.text, "Google");
        assert_eq!(run.url.as_deref(), Some("https://www.google.com/"));
        assert!(run.style.add_modifier.contains(
            Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::CROSSED_OUT
        ));
        let href =
            RichRun::from_value(&json!({"plain_text":"Link", "href":"https://example.com/"}));
        assert_eq!(href.url.as_deref(), Some("https://example.com/"));
        assert!(
            href.display_style()
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn only_http_targets_without_terminal_controls_are_opened_or_emitted() {
        for invalid in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "mailto:a@example.com",
            "https://example.com/\x1b]8;;evil",
            "https://example.com/\n",
            "https://example.com/\u{9c}",
        ] {
            assert!(external_url(invalid).is_none(), "{invalid:?}");
        }
        assert_eq!(
            external_url("https://example.com/a path").as_deref(),
            Some("https://example.com/a%20path")
        );
    }

    #[test]
    fn wrapping_preserves_spans_links_newlines_and_grapheme_widths() {
        let mut run = RichRun::plain("你好👩‍💻ab\nnext");
        run.style = Style::default().add_modifier(Modifier::ITALIC | Modifier::CROSSED_OUT);
        run.url = external_url("https://example.com/");
        let lines = wrap(&[run.clone()], 4);
        assert_eq!(
            lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["你好", "👩‍💻ab", "next"]
        );
        for line in lines {
            assert!(UnicodeWidthStr::width(line.to_string().as_str()) <= 4);
            assert!(
                line.runs
                    .iter()
                    .all(|r| r.style == run.style && r.url == run.url)
            );
        }
        assert_eq!(
            wrap(&[RichRun::plain("a\x1b\x07\n")], 4)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["a", ""]
        );
        assert!(
            wrap(&[run], 1)
                .iter()
                .all(|line| UnicodeWidthStr::width(line.to_string().as_str()) <= 1)
        );
    }
}
