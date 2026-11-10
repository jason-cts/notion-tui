# Notion TUI

A keyboard-driven Notion client built with ratatui and your local rusticnotion checkout. Search shared pages, explore nested blocks, edit text, toggle tasks, append blocks, and create child pages.

## Run

Keep the repositories side by side:

```text
cts/
├── notion-tui/
└── rusticnotion/
```

The dependency is `rusticnotion = { path = "../rusticnotion" }`. This app requires the library patches in that checkout; copying this repository alone is insufficient.

Create a Notion integration with read, insert, and update content permissions. Share the pages you want to access with the integration, then set its token in your shell:

```sh
export NOTION_API_TOKEN='your-integration-token'
cargo run --release
```

To explore the interface without credentials:

```sh
cargo run -- --demo
```

Demo content is read-only. `cargo run -- --help` shows command-line usage. A terminal with true-color support gives the best appearance. Below 80 columns, the focused pane fills the screen.

## OAuth login

If `NOTION_API_TOKEN` is unset or blank, the app restores a saved OAuth session or opens your browser to log in. A nonempty API token takes precedence and does not require OAuth configuration.

Create your own OAuth integration in the [Notion developer portal](https://www.notion.so/profile/integrations), enable read/insert/update content capabilities, and register this exact redirect URI in its OAuth configuration:

```text
http://127.0.0.1:8765/oauth/callback
```

Set the integration's client credentials in your shell, then start the app:

```sh
unset NOTION_API_TOKEN
export NOTION_OAUTH_CLIENT_ID='your-client-id'
export NOTION_OAUTH_CLIENT_SECRET='your-client-secret'
cargo run --release
```

The browser prompts you to choose a workspace and share pages. Return to the terminal after allowing access. The terminal also prints the login URL if the browser does not open. Login times out after five minutes; Ctrl-C cancels it. This uses Notion's [authorization-code flow](https://developers.notion.com/guides/authorization/overview). The client secret is supplied locally and is never bundled with the application or saved to disk. Keep the client variables available on subsequent runs so the app can refresh expired credentials. Environment files are not loaded automatically.

To use another callback port, set `NOTION_OAUTH_REDIRECT_URI` to an HTTP loopback URI with an explicit port and register that exact URI with Notion. The listener binds only to a loopback address. If running over SSH, forward the callback port, for example `ssh -L 8765:127.0.0.1:8765 your-host`, and open the printed URL in your local browser. Choose another registered port if 8765 is occupied.

```sh
cargo run -- --login    # Force browser login, even when NOTION_API_TOKEN is set
cargo run -- --logout   # Forget saved local OAuth login and exit
```

`--demo`, `--login`, and `--logout` are mutually exclusive. Logout does not revoke the integration in Notion or unset an API token in your shell. Use Notion's connection settings to revoke access. A forced login applies to that run; a set API token takes precedence again on the next normal run.

Tokens and workspace metadata are saved as JSON in the platform configuration directory: on Linux, `$XDG_CONFIG_HOME/notion-tui/auth.json`, defaulting to `~/.config/notion-tui/auth.json`; on macOS, `~/Library/Application Support/notion-tui/auth.json`; on Windows, the roaming application-data directory under `notion-tui`. The file contains plaintext tokens. Unix directories and files are created with `0700` and `0600` permissions; symlinks, foreign-owned files, and group/world-accessible permissions are rejected. Fix such permissions before retrying. Credential replacements are atomic and locked between processes. The OAuth client secret is not included in this file.

Saved tokens are validated at startup. Expired tokens refresh once, and both rotated tokens are saved. Revoked or invalid refresh credentials start browser login at startup; network failures preserve credentials and report an error. During a TUI session, reads can retry once after refresh. Writes are never replayed automatically: the draft remains open and you explicitly save again. If renewal requires browser login during an active session, copy any draft you need before exiting and restarting with `--login`.

## Keyboard

| Key | Action |
| --- | --- |
| ↑/↓ or `j`/`k` | Select a page or block |
| `g`/`G`, Home/End | First/last item |
| Tab | Switch pages/content panes |
| Enter | Open a page or a block's children |
| Backspace or ← | Return to parent |
| `/` | Search page titles; Enter submits, Esc cancels |
| `r` | Refresh the focused pane |
| `v` | Open the full block in a scrollable reader |
| `e` | Edit a paragraph, heading, list item, or to-do |
| Space | Toggle a to-do without changing its rich text |
| `a` | Append a block to the current location |
| `n` | Create a child of the current page, including when viewing nested blocks |
| `?` | Keyboard help |
| `q` / Ctrl-C | Quit |

The editor supports multiline text, Unicode, bracketed paste, arrows, and Home/End. Tab cycles the type of an appended block: paragraph, three heading levels, bullet, numbered item, or to-do. Ctrl-S saves; Esc asks before discarding the draft. In the full-block reader and help, ↑/↓ or `j`/`k` scroll, PageUp/PageDown move ten lines, and another key closes the reader.

Consecutive numbered-list siblings count upward and restart after another block type. Nested list items indent and cycle through decimal, lowercase letters, and lowercase Roman numerals. Opening a child page resets the nesting context. The full-block reader uses the same numbering as the content pane.

## Saving

Editing replaces a block's rich text with plain text. Styled text and links require confirmation before replacement. Block colors, child blocks, and a to-do's checked state are preserved when editing text. Mentions, inline equations, and unsupported block types are read-only.

Before updating a block, the app retrieves its current modification timestamp and rejects a conflicting edit. This is a best-effort check: Notion does not provide an atomic compare-and-update operation, so a change between the check and the save can still race. Failed saves retain the draft and show the error. There are no automatic write retries. If a response is lost, inspect Notion before retrying an append or page creation; the first request may have succeeded. Rate-limit errors show the requested waiting time.

Drafts and navigation state live in memory. Quitting discards them. During a save, wait for the response before quitting. The app uses one API token or one saved OAuth workspace authorization and excludes database property editing, deletion, reordering, offline sync, and whole-page Markdown editing. Both clients use API version `2022-06-28`; modern multi-source database workflows are outside this version's scope.

## Structure and library patches

- `src/app.rs`: navigation, modal state, asynchronous results, and stale-read rejection.
- `src/editor.rs`: text buffer, draft validation, and write intent.
- `src/service.rs`: `NotionService`, rusticnotion reads/page creation, and a small reqwest block-write adapter.
- `src/auth.rs` and `src/auth/`: OAuth authorization, callback handling, rotating sessions, and private credential storage.
- `src/model.rs`: display models and minimal block-write payloads.
- `src/ui.rs`: responsive ratatui rendering.

The sibling rusticnotion patches update the API header, default omitted child arrays, encode opaque cursors, add paginated page searches and shallow block reads, expose a test/proxy endpoint, and report rate-limit waiting times. They also fix an existing recursive-helper compile error and propagate child-read failures. Table rows now decode arrays of rich-text arrays, and page/database mentions decode ID references. The corrected table-cell and mention field types affect callers constructing those models directly; child collection field types remain unchanged.

## Checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
python3 tests/terminal_smoke.py
python3 tests/auth_smoke.py

cargo fmt --manifest-path ../rusticnotion/Cargo.toml --check
CARGO_HUSKY_DONT_INSTALL_HOOKS=1 cargo test --manifest-path ../rusticnotion/Cargo.toml --lib
CARGO_HUSKY_DONT_INSTALL_HOOKS=1 cargo clippy --manifest-path ../rusticnotion/Cargo.toml --lib
```

Unit and HTTP mock tests need no credentials; mocks bind local ports. The terminal smoke tests use Linux pseudo-terminals to exercise search, nested navigation, draft recovery, resizing, terminal restoration, and OAuth cancellation/denial before terminal mode. The OAuth smoke test disables desktop launchers and never contacts Notion. rusticnotion's existing integration tests contact Notion and require credentials; the `--lib` command intentionally runs only credential-free library tests.
