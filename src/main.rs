mod app;
mod auth;
mod cli;
mod editor;
mod hyperlinks;
mod model;
mod rich_text;
mod service;
mod ui;

use anyhow::{Result, bail};
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind},
    execute,
};
use futures_util::StreamExt;
use std::{
    io::{self, IsTerminal, Write},
    sync::Arc,
};
use tokio::sync::mpsc;

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = io::stdout().write_all(hyperlinks::CLOSE);
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
    // Keep one input stream across setup and the workspace: restarting its
    // background reader during a screen transition can lose keyboard input.
    let mut events = EventStream::new();
    let demo = mode == cli::Mode::Demo;
    let service: service::Service = if demo {
        Arc::new(service::DemoService)
    } else {
        let Some(choice) = auth::setup::choose(mode == cli::Mode::Login, &mut events).await? else {
            return Ok(());
        };
        match choice {
            auth::setup::Choice::OAuth(config) => {
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
            }
            auth::setup::Choice::ApiToken(token) => Arc::new(service::ApiService::new(token)?),
        }
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = app::App::new(service, tx.clone(), demo);
    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableBracketedPaste)?;
    // Some terminal environments coalesce resize notifications. Poll dimensions
    // on a modest cadence as well; ratatui only writes cells that changed.
    let mut redraw = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut ui = ui::Ui::default();
    let mut hyperlink_output = hyperlinks::Output::default();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    app.start_search();
    while !app.quit {
        if let Some(url) = app.pending_link.take() {
            let sender = tx.clone();
            app.status = "Opening link in your browser…".into();
            app.error = false;
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || open::that_detached(url)).await;
                let result = if matches!(result, Ok(Ok(()))) {
                    Ok(())
                } else {
                    Err(
                        "Could not open your browser; check the default browser configuration"
                            .into(),
                    )
                };
                let _ = sender.send(app::Message::Browser(result));
            });
        }
        let completed = terminal.draw(|frame| {
            ui.draw(frame, &app);
            if no_color {
                // Crossterm 0.29 emits empty color parameters under NO_COLOR,
                // which reset SGR modifiers. Reset colors in the buffer first
                // so bold/italic/underline/strike remain usable in monochrome.
                for cell in &mut frame.buffer_mut().content {
                    cell.fg = ratatui::style::Color::Reset;
                    cell.bg = ratatui::style::Color::Reset;
                }
            }
        })?;
        hyperlink_output.emit(&mut io::stdout(), completed.buffer, &ui.links)?;
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
