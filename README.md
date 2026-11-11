# Notion TUI

A keyboard-driven Notion client built with ratatui and [rusticnotion](https://github.com/jason-cts/rusticnotion). Search shared pages, browse databases, explore nested blocks, edit text, toggle tasks, append blocks, and create child pages.

## Run

Cargo fetches rusticnotion directly from your GitHub repository over SSH:

```toml
rusticnotion = { git = "ssh://git@github.com/jason-cts/rusticnotion.git" }
```

Configure GitHub SSH access before building. `Cargo.lock` pins the resolved Git commit; use `cargo update -p rusticnotion` to adopt a newer revision.

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

If `NOTION_API_TOKEN` is unset or blank and OAuth client credentials are missing, the app opens a **Connect to Notion** screen. Enter an API token, or press F2 to select OAuth and enter a client ID, client secret, and registered redirect URI. Existing OAuth environment values prefill the form. Tab or the arrow keys move between fields; Enter advances or connects, Ctrl-S connects from any field, and Esc or Ctrl-C cancels. Tokens and client secrets are masked, and bracketed paste is supported.

API tokens entered interactively are used for that run and are not saved. OAuth client credentials also stay in memory; access and refresh tokens from successful OAuth login are saved as described below. You can enter the client credentials again on later runs or set the environment variables to skip setup. `--login` restricts setup to OAuth.

When OAuth client credentials are already configured in the environment, the app restores a saved session or opens your browser directly. A nonempty API token takes precedence and does not require OAuth configuration.

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

The browser prompts you to choose a workspace and share pages. Return to the terminal after allowing access. The terminal also prints the login URL if the browser does not open. Login times out after five minutes; Ctrl-C cancels it. This uses Notion's [authorization-code flow](https://developers.notion.com/guides/authorization/overview). The client secret is supplied locally and is never bundled with the application or saved to disk. Supply the client credentials through the form or environment on subsequent runs so the app can refresh expired credentials. Environment files are not loaded automatically.

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
| Backspace | Return to parent |
| ← | Return to parent, including from a database |
| `[` / `]` | Previous/next saved database view |
| `h`/`l` | Move between board columns or scroll table columns; → also moves right |
| `/` | Search page titles; Enter submits, Esc cancels |
| `r` | Refresh the focused pane |
| `v` | Open the full block in a scrollable reader |
| `o` | Open the selected block's external link in your browser; choose when it contains multiple links |
| `e` | Edit a paragraph, heading, list item, or to-do |
| Space | Toggle a to-do without changing its rich text |
| `a` | Insert a block immediately after the selected block; create the first block in an empty location |
| `n` | Create a child of the current page, including when viewing nested blocks |
| `?` | Keyboard help |
| `q` / Ctrl-C | Quit |

The editor supports multiline text, Unicode, bracketed paste, arrows, and Home/End. Tab cycles the type of an inserted block: paragraph, three heading levels, bullet, numbered item, or to-do. Ctrl-S saves; Esc asks before discarding the draft. In the full-block reader and help, ↑/↓ or `j`/`k` scroll and PageUp/PageDown move ten lines. In the reader, `o` opens its external links; another key closes it.

Insertion stays at the current nesting level, directly after the block selected when you pressed `a`. Saving refreshes the list and selects the block just after that anchor. Failed saves retain the draft and its insertion point.

Notion bold, italic, underline, and strikethrough annotations render as terminal styles, including combined styles within a block. Wrapping and the full-block reader preserve these spans. Exact appearance depends on your terminal and font. HTTP/HTTPS links appear cyan and underlined and emit OSC 8 hyperlinks for terminals that support clicking them. Use your terminal's usual link-opening gesture, or press `o` to launch the default browser. For a block with several targets, use ↑/↓ and Enter in the link chooser; Esc cancels. Browsing content alone never launches a browser.

Consecutive numbered-list siblings count upward and restart after another block type. Nested list items indent and cycle through decimal, lowercase letters, and lowercase Roman numerals. Opening a child page resets the nesting context. The full-block reader uses the same numbering as the content pane.

## Child databases

Selecting a child database loads its saved view metadata and shows the layout in the content title, for example `child_database (list)`. Enter opens the database. The first supported saved view is selected initially; `[` and `]` switch views and the choice is remembered for this session. Each view uses its own data source, saved filters, and saved sorts through Notion's Views API.

List views show page titles and visible properties. Tables preserve saved column order, visibility, wrapping, and frozen columns, with widths adapted to terminal cells. Boards show grouped cards, counts, visible properties, and subgroup sections. Use Up/Down or `j`/`k` to select rows/cards, and `h`/`l` to navigate board columns or scroll a table. Enter opens the selected page; Left or Backspace returns to the parent, including from the database itself. Returning from a row page restores its database selection. `v` opens row details, `o` opens external links, and `r` refreshes metadata and rows. Database screens are read-only; existing content-editing commands work after opening a page.

Grouping supports status groups/options, select, multi-select, checkbox, people, creator/editor, relations, text/prefixes, numbers/ranges, dates, and formula results. Multi-valued properties can show the same page in several groups. Missing values appear under **No value**. Date grouping uses the terminal's local timezone and the saved week start. Manual group ordering uses schema order when available and first-seen query order otherwise; a notice explains where ordering can differ from Notion's web UI. Sub-items are displayed as flat rows, and image covers are omitted. Other view layouts remain selectable with an unsupported-layout message. Unknown future grouping types fall back to a list with a notice.

Property matching accepts both raw IDs from saved views and percent-encoded IDs from schemas and pages. Status columns follow schema workflow order, and empty card properties are omitted. Column names and row order come from the API. When a view has no saved sort, a notice explains that API row order may differ from the web's manual order.

Queries load all result pages and hydrate properties used by the view. Temporary query caches are deleted after loading; cleanup failure does not discard rows. Explicit rate-limit responses are retried up to three times using Notion's waiting interval. An expired query restarts once. These operations do not edit saved views, database schemas, or pages.

The table-view compatibility fix for `frozen_column_index: -1` is implemented in the sibling `../rusticnotion` working tree. Until that revision is published to GitHub and adopted in `Cargo.lock`, use a temporary local override for development:

```sh
cargo --config 'patch."https://github.com/jason-cts/rusticnotion.git".rusticnotion.path="../rusticnotion"' run -- --demo
```

Cargo may temporarily resolve that checkout into the lockfile. The declared dependency stays on Git; after publishing rusticnotion, run `cargo update -p rusticnotion` without the override and repeat the locked checks below.

## Saving

Editing replaces a block's rich text with plain text. Styled text and links require confirmation before replacement. Block colors, child blocks, and a to-do's checked state are preserved when editing text. Mentions, inline equations, and unsupported block types are read-only.

Before updating a block, the app retrieves its current modification timestamp and rejects a conflicting edit. This is a best-effort check: Notion does not provide an atomic compare-and-update operation, so a change between the check and the save can still race. Failed saves retain the draft and show the error. There are no automatic write retries. If a response is lost, inspect Notion before retrying an append or page creation; the first request may have succeeded. Rate-limit errors show the requested waiting time.

Drafts and navigation state live in memory. Quitting discards them. During a save, wait for the response before quitting. The app uses one API token or one saved OAuth workspace authorization and excludes database property editing, deletion, reordering, offline sync, and whole-page Markdown editing. Both clients use API version `2026-03-11` from rusticnotion's `NOTION_API_VERSION` constant. Insertions use `position.after_block` to retain their selected sibling anchor.

## Structure and dependency

- `src/app.rs`: navigation, modal state, asynchronous results, and stale-read rejection.
- `src/editor.rs`: text buffer, draft validation, and write intent.
- `src/service.rs`: `NotionService`, rusticnotion reads/page creation, and a small reqwest block-write adapter.
- `src/auth.rs` and `src/auth/`: OAuth authorization, callback handling, rotating sessions, and private credential storage.
- `src/model.rs`: display models and minimal block-write payloads.
- `src/database.rs` and the database submodules under app/service/ui: saved views, property formatting, grouping, navigation, and rendering.
- `src/rich_text.rs` and `src/hyperlinks.rs`: styled spans, Unicode wrapping, and OSC 8 output kept separate from Ratatui cell symbols.
- `src/ui.rs`: responsive ratatui rendering.

The Git dependency supplies the Notion API models, paginated search and child reads, shallow block retrieval, page creation, and rate-limit errors. The TUI accepts pages with data-source parents and uses the current `in_trash` response fields. The reqwest adapter shares the library's API version for block writes.

## Checks

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/terminal_smoke.py
python3 tests/auth_smoke.py
python3 tests/rich_text_smoke.py
python3 tests/database_smoke.py
```

Unit and HTTP mock tests need no credentials; mocks bind local ports. The terminal smoke tests use Linux pseudo-terminals to exercise search, nested navigation, draft recovery, resizing, terminal restoration, interactive authentication, OAuth cancellation/denial, text styles, hyperlinks, and database list/board/table navigation. The authentication smoke test disables desktop launchers; the rich-text smoke test uses a fake browser to verify URL dispatch. None contacts Notion. The library's own tests are maintained in the rusticnotion repository.
