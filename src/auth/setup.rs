use super::{DEFAULT_REDIRECT, OAuthConfig, env_value};
use crate::{TerminalGuard, editor::Buffer};
use anyhow::{Result, bail};
use crossterm::{
    event::{
        EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
};
use futures_util::StreamExt;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::io;
use unicode_width::UnicodeWidthStr;

// No Debug implementations: these values contain secrets.
pub enum Choice {
    ApiToken(String),
    OAuth(OAuthConfig),
}

struct Form {
    fields: [Buffer; 4], // API token, client ID, client secret, redirect URI
    oauth: bool,
    force_oauth: bool,
    focus: usize,
    error: String,
}

impl Form {
    fn from_env(force_oauth: bool) -> Result<Self> {
        let id = env_value("NOTION_OAUTH_CLIENT_ID")?.unwrap_or_default();
        let secret = env_value("NOTION_OAUTH_CLIENT_SECRET")?.unwrap_or_default();
        let oauth = force_oauth || !id.is_empty() || !secret.is_empty();
        Ok(Self {
            fields: [
                Buffer::default(),
                Buffer::new(id),
                Buffer::new(secret),
                Buffer::new(
                    env_value("NOTION_OAUTH_REDIRECT_URI")?
                        .unwrap_or_else(|| DEFAULT_REDIRECT.into()),
                ),
            ],
            oauth,
            force_oauth,
            focus: 0,
            error: String::new(),
        })
    }

    fn indices(&self) -> &[usize] {
        if self.oauth { &[1, 2, 3] } else { &[0] }
    }

    fn oauth_config(&self) -> Result<OAuthConfig> {
        if self.fields[1].text.trim().is_empty() {
            bail!("OAuth client ID is required");
        }
        if self.fields[2].text.trim().is_empty() {
            bail!("OAuth client secret is required");
        }
        OAuthConfig::from_lookup(|name| {
            Ok(Some(match name {
                "NOTION_OAUTH_CLIENT_ID" => self.fields[1].text.trim().to_owned(),
                "NOTION_OAUTH_CLIENT_SECRET" => self.fields[2].text.trim().to_owned(),
                _ => self.fields[3].text.trim().to_owned(),
            }))
        }).map_err(|_| anyhow::anyhow!("Redirect URI must be an HTTP loopback URL with a fixed nonzero port and no credentials, query, or fragment"))
    }

    fn submit(&self) -> Result<Choice> {
        if self.oauth {
            Ok(Choice::OAuth(self.oauth_config()?))
        } else {
            let token = self.fields[0].text.trim();
            if token.is_empty() {
                bail!("API token is required");
            }
            Ok(Choice::ApiToken(token.into()))
        }
    }

    fn paste(&mut self, text: &str) {
        let index = self.indices()[self.focus];
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        if self.fields[index].text.len() + clean.len() <= 8192 {
            self.fields[index].insert(&clean);
            self.error.clear();
        } else {
            self.error = "Credentials are limited to 8192 bytes per field".into();
        }
    }

    fn key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::F(2) if !self.force_oauth => {
                self.oauth = !self.oauth;
                self.focus = 0;
                self.error.clear();
            }
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % self.indices().len(),
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + self.indices().len() - 1) % self.indices().len();
            }
            KeyCode::Enter if self.focus + 1 < self.indices().len() => self.focus += 1,
            KeyCode::Enter => return true,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let index = self.indices()[self.focus];
                self.fields[index] = Buffer::default();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.paste(&c.to_string());
            }
            _ => {
                let index = self.indices()[self.focus];
                self.fields[index].key(key, false);
            }
        }
        false
    }

    fn draw(&self, frame: &mut Frame) {
        let border = Block::default()
            .borders(Borders::ALL)
            .title(" CONNECT TO NOTION ");
        let area = border.inner(frame.area());
        frame.render_widget(border, frame.area());
        let parts = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(3),
        ])
        .split(area);
        let method = if self.oauth {
            "API token   [ OAuth ]"
        } else {
            "[ API token ]   OAuth"
        };
        frame.render_widget(
            Paragraph::new(method).style(Style::default().fg(Color::Cyan)),
            parts[0],
        );
        let description = if self.oauth {
            "Register the exact redirect URI in your Notion integration.\nClient credentials are used for this run; OAuth tokens are saved."
        } else {
            "Enter your Notion integration token.\nThe token is used for this run and is not saved."
        };
        frame.render_widget(Paragraph::new(description), parts[1]);
        let visible = usize::from(parts[2].height / 3);
        if visible == 0 || parts[2].width < 4 {
            frame.render_widget(
                Paragraph::new("Resize terminal to enter credentials"),
                parts[2],
            );
        } else {
            let start = self.focus.saturating_sub(visible - 1);
            let rows = Layout::vertical(
                self.indices()
                    .iter()
                    .skip(start)
                    .take(visible)
                    .map(|_| Constraint::Length(3)),
            )
            .split(parts[2]);
            for (offset, &index) in self.indices().iter().skip(start).take(visible).enumerate() {
                let field = &self.fields[index];
                let secret = index == 0 || index == 2;
                let text = if secret {
                    "*".repeat(field.text.chars().count())
                } else {
                    field.text.clone()
                };
                let col = if secret {
                    field.text[..field.cursor].chars().count()
                } else {
                    UnicodeWidthStr::width(&field.text[..field.cursor])
                };
                let active = start + offset == self.focus;
                let width = usize::from(rows[offset].width.saturating_sub(2));
                let scroll = if active {
                    col.saturating_sub(width.saturating_sub(1))
                } else {
                    0
                };
                let label = [
                    "API token",
                    "OAuth client ID",
                    "OAuth client secret",
                    "Redirect URI",
                ][index];
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(label)
                    .border_style(Style::default().fg(if active {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    }));
                frame.render_widget(
                    Paragraph::new(text).block(block).scroll((0, scroll as u16)),
                    rows[offset],
                );
                if active {
                    frame.set_cursor_position((
                        rows[offset].x + 1 + (col - scroll) as u16,
                        rows[offset].y + 1,
                    ));
                }
            }
        }
        frame.render_widget(
            Paragraph::new(self.error.as_str())
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: true }),
            parts[3],
        );
        let switch = if self.force_oauth {
            "OAuth login required by --login"
        } else {
            "F2 switch API token / OAuth"
        };
        frame.render_widget(Paragraph::new(format!("Tab / arrows: fields • Enter: next / connect\n{switch} • Ctrl-S connect\nEsc / Ctrl-C cancel • Ctrl-U clear field")), parts[4]);
    }
}

pub async fn choose(force_oauth: bool, events: &mut EventStream) -> Result<Option<Choice>> {
    if !force_oauth && let Some(token) = env_value("NOTION_API_TOKEN")? {
        return Ok(Some(Choice::ApiToken(token)));
    }
    let mut form = Form::from_env(force_oauth)?;
    if let Ok(config) = form.oauth_config() {
        return Ok(Some(Choice::OAuth(config)));
    }
    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableBracketedPaste)?;
    while let Some(event) = {
        terminal.draw(|frame| form.draw(frame))?;
        events.next().await
    } {
        match event? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    return Ok(None);
                }
                if form.key(key) {
                    match form.submit() {
                        Ok(choice) => return Ok(Some(choice)),
                        Err(error) => form.error = error.to_string(),
                    }
                }
            }
            Event::Paste(text) => form.paste(&text),
            _ => {}
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn form() -> Form {
        Form {
            fields: [
                Buffer::default(),
                Buffer::default(),
                Buffer::default(),
                Buffer::new(DEFAULT_REDIRECT.into()),
            ],
            oauth: false,
            force_oauth: false,
            focus: 0,
            error: String::new(),
        }
    }

    #[test]
    fn form_validates_both_methods_and_preserves_edits_when_switching() {
        let mut form = form();
        assert!(form.submit().is_err());
        form.paste("  api-token  ");
        assert!(matches!(form.submit().unwrap(), Choice::ApiToken(token) if token == "api-token"));
        form.key(KeyCode::F(2).into());
        assert!(
            form.submit()
                .err()
                .unwrap()
                .to_string()
                .contains("client ID")
        );
        form.paste("client-id");
        assert!(!form.key(KeyCode::Enter.into()));
        assert!(
            form.submit()
                .err()
                .unwrap()
                .to_string()
                .contains("client secret")
        );
        form.paste("client-secret");
        assert!(matches!(form.submit().unwrap(), Choice::OAuth(_)));
        form.fields[3] = Buffer::new("https://remote.example/callback?secret=hidden".into());
        let error = form.submit().err().unwrap().to_string();
        assert!(error.contains("loopback"));
        assert!(!error.contains("hidden"));
        form.key(KeyCode::F(2).into());
        assert!(matches!(form.submit().unwrap(), Choice::ApiToken(token) if token == "api-token"));
        form.force_oauth = true;
        form.oauth = true;
        form.key(KeyCode::F(2).into());
        assert!(form.oauth);
    }

    #[test]
    fn single_line_paste_unicode_editing_and_input_limits() {
        let mut form = form();
        form.paste("密碼\r\n\t\x1b");
        assert_eq!(form.fields[0].text, "密碼");
        form.key(KeyCode::Left.into());
        form.key(KeyCode::Backspace.into());
        assert_eq!(form.fields[0].text, "碼");
        form.paste(&"x".repeat(8193));
        assert_eq!(form.fields[0].text, "碼");
        assert!(!form.error.is_empty());
        form.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(form.fields[0].text.is_empty());
    }

    #[test]
    fn renderer_masks_secrets_and_handles_small_terminals() {
        let mut form = form();
        form.fields[0] = Buffer::new("api-token-should-be-hidden".into());
        form.fields[1] = Buffer::new("client-id".into());
        form.fields[2] = Buffer::new("client-secret-should-be-hidden".into());
        for oauth in [false, true] {
            form.oauth = oauth;
            form.focus = if oauth { 1 } else { 0 };
            for (width, height) in [(110, 24), (45, 20), (20, 12), (5, 5), (1, 1)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| form.draw(frame)).unwrap();
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(!screen.contains("api-token-should-be-hidden"));
                assert!(!screen.contains("client-secret-should-be-hidden"));
                if width >= 45 {
                    assert!(screen.contains("********"));
                }
            }
        }
    }
}
