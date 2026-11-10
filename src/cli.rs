use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Demo,
    Login,
    Logout,
    Help,
}

pub const HELP: &str = "Notion TUI

Usage: notion-tui [--demo | --login | --logout]

A nonempty NOTION_API_TOKEN takes precedence over saved OAuth login.
Without a token, use saved OAuth credentials or log in through your browser.

OAuth setup:
  NOTION_OAUTH_CLIENT_ID       Your Notion OAuth integration client ID
  NOTION_OAUTH_CLIENT_SECRET   Your integration's client secret
  NOTION_OAUTH_REDIRECT_URI    Default: http://127.0.0.1:8765/oauth/callback
Register the exact redirect URI in the integration's OAuth settings.

--demo    Browse a read-only sample workspace without authentication
--login   Force OAuth login, ignoring NOTION_API_TOKEN, then open the TUI
--logout  Forget saved local OAuth login and exit (does not revoke access)
--help    Show this help

Press ? inside the app for keyboard shortcuts.";

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Mode> {
    let args: Vec<_> = args.into_iter().collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Mode::Help);
    }
    let mut mode = Mode::Normal;
    for arg in args {
        let next = match arg.as_str() {
            "--demo" => Mode::Demo,
            "--login" => Mode::Login,
            "--logout" => Mode::Logout,
            _ => bail!("Unknown argument. Use --help for usage"),
        };
        if mode != Mode::Normal {
            bail!("Use only one of --demo, --login, or --logout");
        }
        mode = next;
    }
    Ok(mode)
}

pub fn uses_oauth(mode: Mode, token: Option<&str>) -> bool {
    mode == Mode::Login
        || (mode == Mode::Normal && token.is_none_or(|token| token.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_and_authentication_precedence() {
        assert_eq!(parse(vec![]).unwrap(), Mode::Normal);
        for (flag, mode) in [
            ("--demo", Mode::Demo),
            ("--login", Mode::Login),
            ("--logout", Mode::Logout),
        ] {
            assert_eq!(parse(vec![flag.into()]).unwrap(), mode);
        }
        assert!(parse(vec!["--demo".into(), "--login".into()]).is_err());
        assert!(parse(vec!["--login".into(), "--logout".into()]).is_err());
        assert!(parse(vec!["--bad".into()]).is_err());
        assert!(uses_oauth(Mode::Normal, None));
        assert!(uses_oauth(Mode::Normal, Some("  ")));
        assert!(!uses_oauth(Mode::Normal, Some("token")));
        assert!(uses_oauth(Mode::Login, Some("token")));
        assert!(!uses_oauth(Mode::Demo, None));
    }
}
