# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

lazyjj is a Rust TUI for [Jujutsu/jj](https://github.com/martinvonz/jj) built on [Ratatui](https://ratatui.rs/) + crossterm. It does not link against jj as a library — every operation shells out to the `jj` binary and parses the output. Minimum supported jj version is declared in `JJ_MIN_VERSION` (`src/commander/mod.rs`), enforced at startup unless `--ignore-jj-version` is passed.

## Common commands

- Run against current dir: `cargo run`
- Run against another repo: `cargo run -- --path ~/other-repo`
- Run against a specific revset: `cargo run -- -r '::@'`
- Release build: `cargo build --release` (LTO + size-optimized; output in `target/release/lazyjj`)
- Tests: `cargo test --all-targets` — note tests shell out to a real `jj` binary, so `jj` must be installed and on `PATH`. Each test creates a colocated git+jj repo in a `tempdir`.
- Single test: `cargo test --all-targets <substring>` (e.g. `cargo test run_describe`)
- Lint (matches CI): `cargo clippy --workspace --all-targets -- -D warnings`
- Format (matches CI): `cargo fmt --all -- --check`
- Snapshot tests use [`insta`](https://insta.rs/). Review with `cargo insta review` after changing output formatting; snapshots live in `src/commander/snapshots/`.

## Debugging the running TUI

The TUI takes over the terminal, so `println!` is useless. Use:
- `LAZYJJ_LOG=1 cargo run` — writes a `lazyjj.log` in CWD via `tracing_subscriber`. Most jj-call sites are wrapped with `#[instrument(level = "trace", skip(self))]` and span events log on close, giving you per-command timings for free.
- `LAZYJJ_TRACE=1 cargo run` — emits a Chrome trace JSON viewable at chrome://tracing or ui.perfetto.dev. Useful when something feels laggy.

## Architecture

### Layered structure

The codebase has three deliberately separated layers; respect the boundaries when adding features.

1. **`commander/`** — the only place that calls `jj`. `Commander::execute_command` is the single chokepoint for every subprocess; it sets the working dir to the repo root, applies queued env vars, records every invocation into `command_history` (which feeds the Command Log tab), and surfaces non-zero exits as `CommandError::Status`. `Commander` is split across files by area (`log.rs`, `bookmarks.rs`, `files.rs`, `jj.rs`, `ids.rs`, `github.rs`) — there are multiple `impl Commander` blocks, one per file. Add new jj operations as methods on `Commander`, never call `Command::new("jj")` from anywhere else.
2. **`ui/`** — Ratatui rendering, organized as `Component`s (trait defined in `src/ui/mod.rs`). Each tab and popup implements `Component { focus, update, draw, input }`. Components return a `ComponentAction` (e.g. `ViewFiles(Head)`, `SetPopup(...)`, `RefreshTab()`) instead of mutating other tabs directly; the central `App::handle_action` dispatches.
3. **`app.rs`** — owns one optional instance of each tab (`LogTab`, `FilesTab`, `BookmarksTab`, `CommandLogTab`) plus an optional `popup: Box<dyn Component>`. Tabs are lazily initialized via `get_or_init_tab` so startup doesn't pay for tabs the user never opens. The popup, if present, gets first crack at input; otherwise the current tab does.

### Main loop (`main.rs::run_app`)

1. Each iteration: `update` the popup (if any) and current tab, then `draw` everything via `ui::ui`.
2. Input model: when a popup is showing a loader, `event::poll` with a 100ms timeout so the spinner can animate; otherwise block on `event::read`. Mouse-move events are filtered out before reaching components.
3. Two pseudo-events: `Event::FocusGained` triggers `Component::focus` on the current tab (used to refresh state when the terminal regains focus). The app intentionally does NOT treat bare `Esc` as quit — some terminals emit stray `Esc` events during fast scrolls and that used to drop people out of the app.
4. `setup_terminal` opts into `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` when the terminal supports it, so handlers can distinguish e.g. ctrl+shift+p from ctrl+P. The panic hook restores the terminal before unwinding so a crash doesn't leave the user in a wrecked state.

### Parsing jj output

jj log output is parsed via a custom template `HEAD_TEMPLATE` in `src/commander/log.rs` that emits `[change_id|commit_id|divergent|immutable|signed|bookmarks]`, then matched with a regex. `get_log` runs jj twice — once for the human-readable graph, once with the head template — so each line of the displayed graph maps to an `Option<Head>` for selection logic. When adding fields, update both `HEAD_TEMPLATE` and `HEAD_TEMPLATE_REGEX` together and bump capture indices.

`Head` intentionally implements `PartialEq`/`Hash` ignoring `local_bookmarks` so a bookmark moving doesn't invalidate cached head references held by tabs — preserve that behavior.

### Config

`Env::new` loads jj config by shelling out to `jj config list --template ...` and parsing as TOML. Two struct shapes are supported: a flat dotted-key form (`Config`) and a nested form (`JjConfig`) — older jj versions didn't TOML-escape keys, so we try the flat parse first and fall back to the nested one. Both `lazyjj.*` keys and corresponding upstream jj keys (`ui.diff.format`, `ui.diff.tool`, `git.push-bookmark-prefix`) are honored, with `lazyjj.*` taking precedence.

### Keybindings

Defaults are defined per-context (currently `LogTabKeybinds`, `RebasePopupKeybinds`) in `src/keybinds/`, and overridable via `lazyjj.keybinds.<context>` in jj config. The `set_keybinds!` / `update_keybinds!` / `make_keybinds_help!` macros in `src/keybinds/mod.rs` are the canonical way to register bindings: `set_keybinds!` has a `debug_assert` that two events don't share a shortcut, so duplicates fail loud in dev builds. `Shortcut::from_event` lowercases char keycodes so shift-modified bindings don't double-count. When adding a new bound action: add the variant to the relevant `*Event` enum, register a default in `set_keybinds!`, add a `Option<Keybind>` field to the matching `*KeybindsConfig` struct, wire it in `extend_from_config` via `update_keybinds!`, and add a help string via `make_keybinds_help!`. Document the binding in `docs/keybindings.md`.

### GitHub integration

`commander/github.rs` shells out to `git remote get-url origin` to discover the slug, then to `gh pr list` to fetch open PRs. Both are best-effort — failures (no remote, `gh` not installed, not authed) silently disable the feature. Results are cached process-wide for `PR_CACHE_TTL` (60s) so we don't fire `gh` on every log refresh.

### Clipboard

`src/clipboard.rs` writes to the clipboard via OSC 52 (works over SSH, no system clipboard API needed) with a hand-rolled base64 encoder and tmux passthrough wrapping when `$TMUX` is set. Don't pull in a dependency for this.
