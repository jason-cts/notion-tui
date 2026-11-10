mod app;
mod auth;
mod cli;
mod editor;
mod model;
mod service;
mod ui;

use anyhow::{Context, Result, bail};
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind},
    execute,
};
use futures_util::StreamExt;
use std::{
    io::{self, IsTerminal},
    sync::Arc,
};
use tokio::sync::mpsc;

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        ratatui::restore();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mode = cli::parse(std::env::args().skip(1))?;
    if mode == cli::Mode::Help {
        println!("{}", cli::HELP);
        return Ok(());
    }
    if mode == cli::Mode::Logout {
        auth::TokenStore::default_location()?.logout().await?;
        println!("Saved local OAuth login removed. Notion integration access was not revoked.");
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("Notion TUI requires an interactive terminal");
    }
    let demo = mode == cli::Mode::Demo;
    let service: service::Service = if demo {
        Arc::new(service::DemoService)
    } else {
        let token = if mode == cli::Mode::Normal {
            auth::env_value("NOTION_API_TOKEN")?
        } else {
            None
        };
        if cli::uses_oauth(mode, token.as_deref()) {
            let config = auth::OAuthConfig::from_env()?;
            let store = auth::TokenStore::default_location()?;
            let session = tokio::select! {
                result = auth::OAuthSession::connect(config, store, mode == cli::Mode::Login) => result?,
                result = tokio::signal::ctrl_c() => {
                    result?;
                    println!("OAuth login cancelled.");
                    return Ok(());
                }
            };
            Arc::new(service::OAuthService::new(session).await?)
        } else {
            Arc::new(service::ApiService::new(
                token.context("Missing API token")?,
            )?)
        }
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = app::App::new(service, tx, demo);
    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableBracketedPaste)?;
    let mut events = EventStream::new();
    // Some terminal environments coalesce resize notifications. Poll dimensions
    // on a modest cadence as well; ratatui only writes cells that changed.
    let mut redraw = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut ui = ui::Ui::default();
    app.start_search();
    while !app.quit {
        terminal.draw(|frame| ui.draw(frame, &app))?;
        tokio::select! {
            _ = redraw.tick() => {},
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => app.key(key),
                Some(Ok(Event::Paste(text))) => app.paste(&text),
                Some(Ok(_)) => {},
                Some(Err(error)) => return Err(error.into()),
                None => break,
            },
            Some(message) = rx.recv() => app.receive(message),
        }
    }
    Ok(())
}
