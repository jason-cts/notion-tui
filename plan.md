# Notion TUI using your rusticnotion repository

## Summary

Build a ratatui app for searching Notion pages, browsing nested content, creating child pages, and editing individual blocks. Prefer `NOTION_API_TOKEN`; otherwise authenticate through OAuth.

Fetch rusticnotion directly from `git@github.com:jason-cts/rusticnotion.git`, using Cargo's SSH URL form. Pin the resolved commit in `Cargo.lock`; do not vendor another copy.

## Dependencies and Library Changes

- Configure `notion-tui` with `rusticnotion = { git = "ssh://git@github.com/jason-cts/rusticnotion.git" }`. Document GitHub SSH access and the locked Git revision.
- Use rusticnotion 0.7's Notion API version `2026-03-11` for both clients. Update response fixtures to `in_trash`, cover data-source page parents, and send insertion anchors with `position.after_block`.
- Use the library's current response models and compatibility fixes, including omitted child collections and nested file captions.
- Use rusticnotion for search, page metadata, page creation, and paginated block reads. Traverse children explicitly instead of using its recursive helper.
- Retain the previously selected local reqwest adapter in `notion-tui` for block updates and appending children. Use the same API version and send only required fields.
- Keep library fixes and their regression tests in the rusticnotion repository; keep application behavior in `notion-tui`. No commits or pushes are included.

## Application Behavior

- Separate application state, rendering/input, and a `NotionService` interface covering search, page retrieval/creation, child listing, block updates, and appending.
- Use Tokio tasks for network requests and discard stale read responses. Keep rendering responsive and restore the terminal after exit, errors, or panics.
- Display page results, page content, and status/key hints. Narrow terminals show the focused pane.
- Support arrows/`j/k`, `Tab`, `Enter`, `Backspace`, `/` search, `r` refresh, `?` help, and `q` quit.
- Render common text blocks, code, and nested content; display unsupported blocks read-only.
- Preserve bold, italic, underline, and strikethrough spans through Unicode wrapping and the full-block reader. Keep HTTP/HTTPS targets, emit OSC 8 hyperlinks outside Ratatui's cell symbols, clear stale targets after redraws, and provide `o` to open a link in the default browser with a chooser for multiple targets.
- Number consecutive ordered siblings correctly, restart after other block types, and cycle decimal, alphabetic, and Roman markers by nesting depth. Keep wrapped lines aligned with their text.
- Provide `e` to edit paragraphs, headings, lists, and to-dos; `a` to insert these types immediately after the selected sibling (or create the first block in an empty container); `Space` to toggle a to-do; and `n` to create a child page. Preserve the insertion anchor in failed drafts and select the next block after a successful refresh.
- Use a multiline editor with explicit save and cancel. Confirm formatting loss before replacing styled text; keep mentions and equations read-only.
- Preserve failed-save drafts, check modification timestamps before overwriting, and show actionable loading/error states. Do not automatically retry uncertain creates or appends.

## OAuth Fallback

- If neither an API token nor complete OAuth client configuration is supplied, show a TUI authentication form with API-token and OAuth methods. Prefill partial OAuth environment values, mask secrets, validate required fields and loopback redirects, support paste and small terminals, and allow cancellation. Keep entered API tokens and OAuth client secrets in memory only. Fully configured environments bypass the form; `--login` restricts it to OAuth.
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
- Run formatting checks, Clippy, and credential-free tests against the locked Git dependency. Keep the library's own test suite in its repository. Run existing live integration tests only when credentials are available.
- Keep one connection and one saved OAuth workspace. Exclude database property workflows, deletion, reordering, offline sync, and whole-page Markdown editing.

## Child Database Views

- Add typed view IDs, saved view/configuration/grouping models, and list/retrieve/query/paginate/query-cleanup methods in rusticnotion. Preserve unknown view types, additional fields, partial page references, and property truncation metadata. Add user retrieval for people-group labels.
- Keep notion-tui's SSH Git dependency. Validate the unpublished library patch through a temporary local Cargo override; update the Git lockfile only after the library revision is published. No commits or pushes are included.
- Distinguish pages, block containers, and databases in navigation. Fetch/cache selected child-database metadata and show `child_database (list)`, `(board)`, or `(table)` in the content title. Enter opens a saved view; `[`/`]` switches views, and Enter on a row opens its page. Restore the database view, selected page, and viewport on return.
- Query each view's own data source through the saved-view query API, retaining server filters/sorts. Follow all result and required-property pages, resolve labels with cached lookups, reject repeated/missing cursors, clean up temporary query caches, retry explicit rate limits up to three times, and restart an expired snapshot once.
- Render list titles/properties, board columns/cards/subgroups, and table headers/cells. Apply saved property visibility/order, terminal-adapted widths, wrapping, frozen columns, Unicode styles, and hyperlinks. Adapt to narrow terminals and keep selected entries visible.
- Support every documented grouping family, including formula result grouping and multi-valued membership. Use local dates, configured week starts, half-open numeric buckets, schema/first-seen manual order, and an explicit No value group. Preserve query order within groups. Display notices for API presentation gaps and unsupported future grouping/layouts.
- Keep database screens read-only; open a row page to use existing content editing. Document keyboard behavior and add demo content plus database PTY coverage alongside existing smoke tests.
- Validate library HTTP contracts, TUI service pagination/expiry/cleanup, all grouping families, subgroups/empty groups, stale loads, view switching, page navigation, styles/links, narrow rendering, and terminal restoration. Run formatting, Clippy, builds, and credential-free tests in both repositories.
