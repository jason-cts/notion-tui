# Notion TUI using your rusticnotion repository

## Summary

Build a ratatui app for searching Notion pages, browsing nested content, creating child pages, and editing individual blocks. Prefer `NOTION_API_TOKEN`; otherwise authenticate through OAuth.

Use your clean local checkout at `../rusticnotion`, whose remote is `jason-cts/rusticnotion`. Make necessary library fixes there; do not vendor another copy.

## Dependencies and Library Changes

- Configure `notion-tui` with `rusticnotion = { path = "../rusticnotion" }`. Document the sibling checkout requirement.
- Patch rusticnotion’s API version from `2022-02-22` to `2022-06-28` to match its `rich_text` models.
- Default omitted block-child collections to empty during deserialization. Preserve existing public field types and constructor compatibility.
- Use rusticnotion for search, page metadata, page creation, and paginated block reads. Traverse children explicitly instead of using its recursive helper.
- Retain the previously selected local reqwest adapter in `notion-tui` for block updates and appending children. Use the same API version and send only required fields.
- Keep library fixes and their regression tests in `../rusticnotion`; keep application behavior in `notion-tui`. No commits or pushes are included.

## Application Behavior

- Separate application state, rendering/input, and a `NotionService` interface covering search, page retrieval/creation, child listing, block updates, and appending.
- Use Tokio tasks for network requests and discard stale read responses. Keep rendering responsive and restore the terminal after exit, errors, or panics.
- Display page results, page content, and status/key hints. Narrow terminals show the focused pane.
- Support arrows/`j/k`, `Tab`, `Enter`, `Backspace`, `/` search, `r` refresh, `?` help, and `q` quit.
- Render common text blocks, code, and nested content; display unsupported blocks read-only.
- Number consecutive ordered siblings correctly, restart after other block types, and cycle decimal, alphabetic, and Roman markers by nesting depth. Keep wrapped lines aligned with their text.
- Provide `e` to edit paragraphs, headings, lists, and to-dos; `a` to append these types; `Space` to toggle a to-do; and `n` to create a child page.
- Use a multiline editor with explicit save and cancel. Confirm formatting loss before replacing styled text; keep mentions and equations read-only.
- Preserve failed-save drafts, check modification timestamps before overwriting, and show actionable loading/error states. Do not automatically retry uncertain creates or appends.

## OAuth Fallback

- When `NOTION_API_TOKEN` is absent or blank, reuse a saved OAuth login or open browser authorization before entering terminal mode. Demo mode requires no authentication.
- Use local `NOTION_OAUTH_CLIENT_ID` and `NOTION_OAUTH_CLIENT_SECRET` configuration. Default the registered redirect to `http://127.0.0.1:8765/oauth/callback`; allow an explicitly configured HTTP loopback redirect.
- Bind the callback listener before opening the browser, print a fallback URL, generate an unpredictable state, validate callbacks, and handle denial, a five-minute timeout, and Ctrl-C cancellation.
- Exchange the authorization code using Basic client authentication and the exact registered redirect URI. Keep secrets and provider response bodies out of diagnostics.
- Save one workspace's access token, refresh token, client ID, and workspace metadata in the platform configuration directory at `notion-tui/auth.json`. Never save the client secret. Use atomic replacement and a cross-process refresh lock; require private Unix directory/file permissions and reject symlinks or foreign ownership.
- Validate restored credentials. On an explicit 401, refresh and persist both rotated tokens. Invalid refresh credentials at startup start a new browser login; transient failures preserve saved credentials and report the problem.
- Share the OAuth session across both HTTP clients. Retry rejected reads once after refreshing. Never replay writes automatically; retain failed drafts and ask for an explicit save. If runtime refresh credentials are invalid, direct the user to restart with `--login`.
- Add mutually exclusive `--login`, `--logout`, and `--demo` modes. Forced login ignores the API token for that run. Logout removes only the local OAuth login and works without a terminal; help also works without a terminal.

## Validation and Defaults

- Add rusticnotion regression tests for API-version headers, omitted children, and representative block responses.
- Test pagination, navigation, stale responses, editing, conflicts, failed-save recovery, and write payloads using fixtures and mocks.
- Verify rendering at normal and narrow widths, including Unicode.
- Mock OAuth code exchange, state validation, denial, timeout, cancellation, saved-login reuse, refresh rotation, concurrent sessions, token precedence, logout, private storage, and expired saves retaining drafts.
- Run formatting checks, Clippy, and credential-free tests in both repositories. Run existing live integration tests only when credentials are available.
- Keep one connection and one saved OAuth workspace. Exclude database property workflows, deletion, reordering, offline sync, and whole-page Markdown editing.
