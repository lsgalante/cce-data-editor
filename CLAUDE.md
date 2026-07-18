# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-data-editor` is the KDL config editor for the CCE Wayland desktop environment: a
GUI app for browsing and editing KDL files (primarily `~/.config/cce/config.kdl` and
`input.kdl`). It is one crate of the multi-repo `cce` workspace — this repo is cloned
side-by-side with its siblings and depends on `cce-ui = { path = "../cce-ui" }`, so a
checkout of `../cce-ui` must exist to build. The workspace-level `../CLAUDE.md` (when
present) covers the whole desktop environment; commit in THIS repo, never at the
workspace root.

The entire app is **one file: `src/main.rs`** (~2200 lines) — a `DataEditorApp` struct
implementing `cce_ui::engine::Application`, run by `cce_ui::engine::run::<DataEditorApp>()`.
There are no modules to navigate; use the section landmarks below.

## Build, run, test

```sh
cargo build --release          # binary lands in ../target/release/ (shared workspace target)
cargo run -- <file.kdl>        # optional file argument auto-loads on startup
make install                   # installs ../target/release/cce-data-editor to ~/.local/bin
cargo test                     # see caveat below
```

Running requires a live Wayland session (it is a raw Wayland client, not X11/toolkit).

**Test caveat**: both `#[cfg(test)]` tests read the user's real
`~/.config/cce/config.kdl` (via `cce_ui::config::get_config_path()`) and
`test_kdl_roundtrip` additionally asserts a specific key
(`style.status.background_color`) exists in it. They are environment-dependent
integration checks of the flatten/unflatten/span machinery, not hermetic unit tests —
failures may mean the local config changed, not that code broke.

## Architecture

### Data model: KDL ⇄ JSON ⇄ flat keys

The core state is `flat_keys: Vec<(String, serde_json::Value)>` — the KDL document
parsed to JSON (`cce_ui::config::parse_kdl_to_json`) then flattened to dotted paths
with bracket indices (`style.status.background_color`, `outputs[0].mode`). Three
representations are kept in sync:

- **Raw KDL text** — the right-pane multiline `TextBox` (`raw_json_editor`; the name
  is historical, it holds KDL). Ground truth for save/format.
- **`flat_keys`** — drives the left-pane `TreeList` and all value edits.
- **On-screen selection** — `selected_key_idx` into `flat_keys`.

Edits flow one of two ways:
- Tree/editor side: mutate `flat_keys[idx].1` → `update_raw_from_flat()` →
  `unflatten_json` → `json_to_kdl_string_with_annotations` regenerates the raw text
  (preserving KDL type annotations harvested from the current text).
- Raw side: `raw_json_editor.take_change()` in `tick()` reparses the text and rebuilds
  `flat_keys` (only if the KDL parses), re-resolving the selection by key name.

`parse_path` / `find_kdl_span` map a flat path to a byte span in the KDL source. This
powers bidirectional selection sync: clicking a tree row highlights its span in the raw
editor (`sync_preview_selection`), and clicking in the raw editor selects the tree row
whose span most tightly contains the cursor (smallest-span-wins search in
`handle_mouse_input`).

Save/format refuse to run when the text is not valid KDL; an invalid document still
loads (best-effort parse) with an error in the status bar.

### Type-driven inline value editors

Eight editor widgets exist permanently as fields (`selected_*_editor`); exactly one is
positioned inline over the selected tree row, the rest are parked off-screen at
`(-1000, -1000)` (the "hide" convention — there is no visibility flag). Which editor
appears is decided in two places that must stay in agreement: the layout block in
`display_list` and the click handler in `handle_mouse_input`. The decision keys off:

- **KDL type annotations** in the document (read via
  `cce_ui::config::get_kdl_type_annotation`): `menu:a,b,c` → `Dropdown`,
  `button` / `button:<shell-cmd>` → `Button` (clicking spawns the command),
  `keybind` → `KeybindRecorder`, `f64:min-max` → clamps applied values.
- **Key-name heuristics**: `font` / `*_font` / `*.font` → `FontSelector`; keybind-ish
  names (`key`, `shortcut`, `brightness_up`, …) → `KeybindRecorder`.
- **Value shape**: `#`-prefixed string → `ColorSelector`, bool → `Checkbox`,
  integer → `Spinbox`, everything else → plain `TextBox` (values are entered as JSON;
  bare words fall back to strings).

Editor commits are polled in `tick()` via each widget's `take_change()`, all funneling
into the same mutate-`flat_keys` → `update_raw_from_flat()` → `sync_preview_selection()`
sequence.

### cce-ui "Phase 6" conventions (post-Backplate)

This app tracks the current cce-ui architecture; mirror these patterns when touching UI
code, and don't reintroduce the retired ones:

- **No root container.** The root `Backplate` and the `SplitBox` are dissolved.
  Top-level widgets are parentless, registered once with
  `ui_context.register_widget(id, ptr)` in the first `display_list` call, and painted
  as separate roots via `cce_ui::scene::painter::paint_root_into` (shared borrows).
- **Single paint path.** Everything renders through `display_list` into one `PaintCtx`
  — window plate quad, widget walk, splitter divider quad, the toolbar "File:" label,
  then popovers and the context menu drawn last directly into the list (the engine's
  xdg-popup path is gone; `display_list_text()` returns `true`).
- **`SplitPane`** (app-owned struct at the top of the file) replaces `SplitBox`: it
  keeps only the divider drag/hover state and fraction; pane rects come from the scene
  solver.
- **Layout via the scene solver**: `display_list` builds a small
  `cce_ui::scene::layout` arena (column: menubar 42px / content row with the two panes
  growing by `split.frac` / statusbar 30px) and assigns solved rects with `set_rect`.
  The inline value editors are hand-positioned at `row_x + 245.0` over the tree row.
- **Routed events**: input handlers build a `cce_ui::widget::Event` and call
  `ui_context.propagate_event(&ev, root_id)` per root. Note the ordering contracts
  documented inline: the key-input chain short-circuits on first handled;
  Enter-applies-value is gated on `value_was_editing` captured *before* dispatch;
  mouse events check `editor_handled` before letting the tree list see the click.
- **TextBox editing model**: while `editing`, live content is `edit_buffer`, not
  `text` — hence the recurring
  `let content = if editing { &edit_buffer } else { &text }` idiom. Keep it when
  reading editor content.
- **Text shaping**: `refresh_widget_text()` calls `prepare_text(&mut self.font_system)`
  on every text-bearing widget each rebuild — this is load-bearing for cursor↔pixel
  mapping, not just rendering.

### Shortcuts and app config

Keyboard shortcuts resolve once at startup (`DataEditorKeys::load`) from
`~/.config/cce/input.kdl` through the `cce-data-editor` domain (falling back to
`cce-ui`, then the hardcoded defaults: ctrl+o / ctrl+s / ctrl+q / ctrl+shift+f) via
`cce_ui::input::app_chord`. The font-size chords (ctrl +/-) are deliberately hardcoded.
The recent-files list (File dropdown) is shared toolkit state via
`cce_ui::config::load_recent_files` / `save_recent_files`, capped at 10 entries.
