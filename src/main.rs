use wayland_client::QueueHandle;
use glyphon::FontSystem;
use cce_ui::engine::{Application, EngineState, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{
    MouseButton, ElementState, MouseScrollDelta, KeyEvent, WidgetHost,
    TextBox, Button, Key, TreeList, TreeElement, ColorSelector, Spinbox, FontSelector, Dropdown,
    KeybindRecorder, MenuBar, StatusBar, Checkbox
};

#[derive(Debug, Clone)]
enum AppMessage {
    Exit,
    OpenDocument,
    OpenRecent(std::path::PathBuf),
    SaveDocument,
    SaveDocumentAs,
    FormatJson,
    RefreshDocument,
    ApplyValue,
    DeleteKey,
    /// File-picker results, sent from the picker thread over the engine's
    /// message channel — the portal dialog must not block the event loop (the
    /// dropdown's contraction has to animate while the dialog is up).
    OpenPicked(Option<std::path::PathBuf>),
    SaveAsPicked(Option<std::path::PathBuf>),
}

#[derive(Debug, Clone)]
enum PathToken {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Vec<PathToken> {
    let mut tokens = Vec::new();
    for part in path.split('.') {
        if part.is_empty() { continue; }
        if let Some(bracket_idx) = part.find('[') {
            let name = &part[..bracket_idx];
            if !name.is_empty() {
                tokens.push(PathToken::Key(name.to_string()));
            }
            let mut rest = &part[bracket_idx..];
            while let Some(start) = rest.find('[') {
                if let Some(end) = rest.find(']') {
                    let idx_str = &rest[start + 1..end];
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        tokens.push(PathToken::Index(idx));
                    }
                    rest = &rest[end + 1..];
                } else {
                    break;
                }
            }
        } else {
            tokens.push(PathToken::Key(part.to_string()));
        }
    }
    tokens
}

fn find_kdl_span(content: &str, tokens: &[PathToken]) -> Option<(usize, usize)> {
    let doc = content.parse::<kdl::KdlDocument>().ok()?;
    find_kdl_span_in_doc(&doc, tokens)
}

fn find_kdl_span_in_doc(doc: &kdl::KdlDocument, tokens: &[PathToken]) -> Option<(usize, usize)> {
    if tokens.is_empty() {
        return None;
    }
    let next_token = &tokens[0];
    match next_token {
        PathToken::Key(target_key) => {
            let target_idx = if tokens.len() > 1 {
                if let PathToken::Index(idx) = &tokens[1] {
                    Some(*idx)
                } else {
                    None
                }
            } else {
                None
            };

            let nodes: Vec<&kdl::KdlNode> = doc.nodes().iter().filter(|n| n.name().value() == target_key).collect();
            if let Some(idx) = target_idx {
                let node = nodes.first()?;
                if let Some(children) = node.children() {
                    let child_node = children.nodes().get(idx)?;
                    if tokens.len() == 2 {
                        let span = child_node.span();
                        return Some((span.offset(), span.offset() + span.len()));
                    } else if tokens.len() == 3 {
                        if let PathToken::Key(prop_key) = &tokens[2] {
                            if let Some(entry) = child_node.entries().iter().find(|e| e.name().map(|id| id.value()) == Some(prop_key)) {
                                let span = entry.span();
                                return Some((span.offset(), span.offset() + span.len()));
                            }
                        }
                    }
                } else {
                    let node = nodes.get(idx)?;
                    if tokens.len() == 2 {
                        let span = node.span();
                        return Some((span.offset(), span.offset() + span.len()));
                    } else {
                        if let PathToken::Key(prop_key) = &tokens[2] {
                            if let Some(entry) = node.entries().iter().find(|e| e.name().map(|id| id.value()) == Some(prop_key)) {
                                let span = entry.span();
                                return Some((span.offset(), span.offset() + span.len()));
                            }
                        }
                    }
                }
            } else {
                let node = nodes.first()?;
                if tokens.len() == 1 {
                    if let Some(entry) = node.entries().first() {
                        let span = entry.span();
                        return Some((span.offset(), span.offset() + span.len()));
                    } else {
                        let span = node.span();
                        return Some((span.offset(), span.offset() + span.len()));
                    }
                } else if tokens.len() == 2 {
                    if let PathToken::Key(prop_key) = &tokens[1] {
                        if let Some(entry) = node.entries().iter().find(|e| e.name().map(|id| id.value()) == Some(prop_key)) {
                            let span = entry.span();
                            return Some((span.offset(), span.offset() + span.len()));
                        }
                    }
                    if let Some(children) = node.children() {
                        if let Some(span) = find_kdl_span_in_doc(children, &tokens[1..]) {
                            return Some(span);
                        }
                    }
                } else if let Some(children) = node.children() {
                    return find_kdl_span_in_doc(children, &tokens[1..]);
                }
            }
        }
        PathToken::Index(_) => {}
    }
    None
}


fn insert_value(target: &mut serde_json::Value, tokens: &[PathToken], value: serde_json::Value) {
    if tokens.is_empty() {
        *target = value;
        return;
    }
    
    match &tokens[0] {
        PathToken::Key(key) => {
            if !target.is_object() {
                *target = serde_json::Value::Object(serde_json::Map::new());
            }
            let map = target.as_object_mut().unwrap();
            
            let next_default = if tokens.len() > 1 {
                match &tokens[1] {
                    PathToken::Key(_) => serde_json::Value::Object(serde_json::Map::new()),
                    PathToken::Index(_) => serde_json::Value::Array(Vec::new()),
                }
            } else {
                serde_json::Value::Null
            };
            
            let entry = map.entry(key.clone()).or_insert(next_default);
            insert_value(entry, &tokens[1..], value);
        }
        PathToken::Index(idx) => {
            if !target.is_array() {
                *target = serde_json::Value::Array(Vec::new());
            }
            let arr = target.as_array_mut().unwrap();
            
            let next_default = if tokens.len() > 1 {
                match &tokens[1] {
                    PathToken::Key(_) => serde_json::Value::Object(serde_json::Map::new()),
                    PathToken::Index(_) => serde_json::Value::Array(Vec::new()),
                }
            } else {
                serde_json::Value::Null
            };
            
            while arr.len() <= *idx {
                arr.push(next_default.clone());
            }
            insert_value(&mut arr[*idx], &tokens[1..], value);
        }
    }
}

fn flatten_json(value: &serde_json::Value, prefix: &str, out: &mut Vec<(String, serde_json::Value)>) {
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                out.push((prefix.to_string(), value.clone()));
            } else {
                for (k, v) in map {
                    let new_prefix = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{}.{}", prefix, k)
                    };
                    flatten_json(v, &new_prefix, out);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                out.push((prefix.to_string(), value.clone()));
            } else {
                for (i, v) in arr.iter().enumerate() {
                    let new_prefix = if prefix.is_empty() {
                        format!("[{}]", i)
                    } else {
                        format!("{}[{}]", prefix, i)
                    };
                    flatten_json(v, &new_prefix, out);
                }
            }
        }
        _ => {
            out.push((prefix.to_string(), value.clone()));
        }
    }
}

fn unflatten_json(flat: &[(String, serde_json::Value)]) -> serde_json::Value {
    let mut root = serde_json::Value::Null;
    for (path, val) in flat {
        let tokens = parse_path(path);
        insert_value(&mut root, &tokens, val.clone());
    }
    root
}


/// App-owned two-pane horizontal split replacing the dissolved `SplitBox`
/// (Phase 6aa; the 6y recipe): divider quad, hover tint, and the proportion drag —
/// `SplitBox`'s two-child horizontal math verbatim. The panes (tree list, raw editor)
/// are positioned directly from the pane rects and walked as separate roots.
struct SplitPane {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    frac: f32,
    min_left: f32,
    min_right: f32,
    gap: f32,
    dragging: bool,
    hovered: bool,
}

impl SplitPane {
    fn new(frac: f32, min_left: f32, min_right: f32, gap: f32) -> Self {
        Self { x: 0.0, y: 0.0, w: 0.0, h: 0.0, frac, min_left, min_right, gap, dragging: false, hovered: false }
    }

    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.x = x;
        self.y = y;
        self.w = w;
        self.h = h;
    }

    fn left_w(&self) -> f32 {
        self.frac * (self.w - self.gap).max(0.0)
    }

    fn divider_rect(&self) -> (f32, f32, f32, f32) {
        (self.x + self.left_w(), self.y, self.gap, self.h)
    }

    fn hit_divider(&self, px: f32, py: f32) -> bool {
        let (sx, sy, sw, sh) = self.divider_rect();
        px >= sx && px <= sx + sw && py >= sy && py <= sy + sh
    }

    fn cursor_moved(&mut self, px: f32, py: f32) -> bool {
        let mut handled = false;
        if self.dragging {
            let combined = (self.w - self.gap).max(0.0);
            if combined > 0.1 {
                let new_left = (px - self.x - self.gap / 2.0)
                    .clamp(self.min_left, (combined - self.min_right).max(self.min_left));
                let new_frac = new_left / combined;
                if (new_frac - self.frac).abs() > 0.0001 {
                    self.frac = new_frac;
                    handled = true;
                }
            }
        }
        let new_hovered = !self.dragging && self.hit_divider(px, py);
        if new_hovered != self.hovered {
            self.hovered = new_hovered;
            handled = true;
        }
        handled
    }

    fn press(&mut self, px: f32, py: f32) -> bool {
        if self.hit_divider(px, py) {
            self.dragging = true;
            return true;
        }
        false
    }

    fn release(&mut self) -> bool {
        std::mem::take(&mut self.dragging)
    }

    /// `SplitBox::extra_quads`: accent while dragging, tint on hover, hairline otherwise.
    fn divider_quad(&self) -> (f32, f32, f32, f32, [f32; 4]) {
        let (sx, sy, sw, sh) = self.divider_rect();
        if self.dragging {
            (sx + sw / 2.0 - 1.0, sy, 2.0, sh, [0.36, 0.56, 0.38, 0.8])
        } else if self.hovered {
            (sx + sw / 2.0 - 1.0, sy, 2.0, sh, [0.25, 0.25, 0.32, 0.6])
        } else {
            (sx + sw / 2.0 - 0.5, sy, 1.0, sh, [0.15, 0.15, 0.18, 0.4])
        }
    }
}

/// App shortcuts, resolved once at startup from input.kdl
/// (`cce-data-editor` domain → `cce-ui` domain).
struct DataEditorKeys {
    open_document: String,
    save_document: String,
    quit: String,
    format_json: String,
}

impl DataEditorKeys {
    fn load() -> Self {
        let get = cce_ui::input::app_chord;
        Self {
            open_document: get("open_document", "ctrl+o"),
            save_document: get("save_document", "ctrl+s"),
            quit: get("quit", "ctrl+q"),
            format_json: get("format_json", "ctrl+shift+f"),
        }
    }
}

struct DataEditorApp {
    keys: DataEditorKeys,

    // Toolbar Buttons
    btn_open: cce_ui::widget::Adapted<Dropdown>,

    // Left Panel Form Edit
    flat_keys: Vec<(String, serde_json::Value)>,
    selected_key_idx: Option<usize>,
    /// A `--select <flat.path>` from the CLI, waiting for the first laid-out
    /// frame (the tree's viewport height) before it can select and scroll.
    pending_select: Option<String>,
    tree_list: cce_ui::widget::Adapted<TreeList>,

    // Edit Value input
    selected_value_editor: cce_ui::widget::Adapted<TextBox>,
    selected_color_editor: cce_ui::widget::Adapted<ColorSelector>,
    selected_spinbox_editor: cce_ui::widget::Adapted<cce_ui::widget::Spinbox>,
    selected_font_editor: cce_ui::widget::Adapted<FontSelector>,
    selected_choice_editor: cce_ui::widget::Adapted<Dropdown>,
    selected_keybind_editor: cce_ui::widget::Adapted<KeybindRecorder>,
    selected_bool_editor: cce_ui::widget::Adapted<Checkbox>,
    selected_button_editor: cce_ui::widget::Adapted<cce_ui::widget::Button>,
    selected_bevel_editor: cce_ui::widget::Adapted<cce_ui::widget::BevelPreview>,
    /// A cce-bevel child spawned from the (bevel) preview: kept so a second
    /// click refocuses it (try_wait reaps an exited one) instead of piling
    /// up editors.
    bevel_child: Option<std::process::Child>,

    // Right Panel Raw Json
    raw_json_editor: cce_ui::widget::Adapted<TextBox>,

    // App state
    current_file_path: Option<std::path::PathBuf>,
    status_message: Option<(String, bool)>,
    // Disk-sync watch: the open file's (mtime, len) and a hash of its content
    // as last loaded/saved. tick() polls once a second; a mismatch means
    // another writer touched the file — auto-reload when the in-memory
    // document still matches `disk_hash` (no local edits to lose), else flag
    // `file_outdated` (toolbar + status indicator, cleared by Refresh/Save).
    disk_state: Option<(std::time::SystemTime, u64)>,
    disk_hash: u64,
    file_outdated: bool,
    disk_poll: f32,
    // File pickers run on a thread and report back over the engine's message
    // channel (OpenPicked / SaveAsPicked) so the event loop keeps animating.
    msg_sender: calloop::channel::Sender<AppMessage>,
    file_dialog_open: bool,

    // UI state
    menubar: cce_ui::widget::Adapted<MenuBar>,
    statusbar: cce_ui::widget::Adapted<StatusBar>,
    width: u32,
    height: u32,
    scale_factor: f64,
    // Shapes glyph advances (prepare_text) for the editors — load-bearing for cursor↔pixel
    // mapping; all rendered text is display-list prims shaped by the engine.
    font_system: FontSystem,
    needs_rebuild: bool,
    ui_context: cce_ui::context::UiContext,
    ctrl_pressed: bool,
    initial_focus: bool,
    widgets_registered: bool,
    cached_content: String,
    cached_flat_keys: Vec<(String, serde_json::Value)>,
    cached_annotations: Vec<Option<String>>,
    split: SplitPane,
}


impl DataEditorApp {

    fn load_recent_files(&self) -> Vec<String> {
        cce_ui::config::load_recent_files()
    }

    fn save_recent_files(&self, files: &[String]) {
        cce_ui::config::save_recent_files(files)
    }

    fn add_recent_file(&mut self, file_path: &std::path::Path) {
        if let Ok(abs_path) = std::fs::canonicalize(file_path) {
            let abs_str = abs_path.to_string_lossy().to_string();
            let mut recent = self.load_recent_files();
            recent.retain(|p| p != &abs_str);
            recent.insert(0, abs_str);
            if recent.len() > 10 {
                recent.truncate(10);
            }
            self.save_recent_files(&recent);
            self.update_recent_files_dropdown(recent);
        }
    }

    fn update_recent_files_dropdown(&mut self, recent: Vec<String>) {
        let mut options = vec![
            "Save".to_string(),
            "Save As".to_string(),
            "Refresh".to_string(),
            "Format".to_string(),
            "-".to_string(),
            "Open...".to_string(),
        ];
        if !recent.is_empty() {
            options.push("-".to_string());
            options.extend(recent);
        }
        options.push("-".to_string());
        options.push("Exit".to_string());
        self.btn_open.options = options;
        self.btn_open.selected = 0;
        self.needs_rebuild = true;
    }

    fn open_file_by_path(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                self.raw_json_editor.text = content;
                self.raw_json_editor.edit_buffer = self.raw_json_editor.text.clone();
                self.raw_json_editor.cursor_idx = 0;
                self.raw_json_editor.select_anchor = None;
                self.raw_json_editor.editing = false;
                
                self.flat_keys.clear();
                if self.raw_json_editor.text.parse::<kdl::KdlDocument>().is_ok() {
                    let val = cce_ui::config::parse_kdl_to_json(&self.raw_json_editor.text);
                    flatten_json(&val, "", &mut self.flat_keys);
                    self.status_message = Some((format!("Opened {}", path.file_name().unwrap_or_default().to_string_lossy()), false));
                } else {
                    let val = cce_ui::config::parse_kdl_to_json(&self.raw_json_editor.text);
                    flatten_json(&val, "", &mut self.flat_keys);
                    self.status_message = Some(("Loaded, but KDL is syntactically invalid".to_string(), true));
                }
                
                self.selected_key_idx = None;
                self.tree_list.scroll_box.scroll_y = 0.0;
                self.current_file_path = Some(path.clone());
                self.sync_preview_selection();
                self.add_recent_file(&path);
                self.note_disk_sync();
                Ok(())
            }
            Err(e) => {
                Err(format!("Error opening: {}", e))
            }
        }
    }

    fn update_raw_from_flat(&mut self) {
        let root = unflatten_json(&self.flat_keys);
        let mut anno_map = std::collections::HashMap::new();
        let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
        for (key_path, _) in &self.flat_keys {
            if let Some(anno) = cce_ui::config::get_kdl_type_annotation(content, key_path) {
                anno_map.insert(key_path.clone(), anno);
            }
        }
        let pretty = cce_ui::config::json_to_kdl_string_with_annotations(&root, &anno_map);
        self.raw_json_editor.text = pretty;
        self.raw_json_editor.edit_buffer = self.raw_json_editor.text.clone();
        self.raw_json_editor.sync_editor_state();
    }

    fn rename_key_path(&mut self, old_path: &str, new_path: &str) {
        let old_prefix = format!("{}.", old_path);
        let new_prefix = format!("{}.", new_path);

        for (k, _) in &mut self.flat_keys {
            if k == old_path {
                *k = new_path.to_string();
            } else if k.starts_with(&old_prefix) {
                *k = k.replacen(&old_prefix, &new_prefix, 1);
            }
        }
        
        self.update_raw_from_flat();
        self.rebuild_tree();
    }

    /// Record that memory and disk agree right now — call immediately after
    /// reading the open file or writing it. Stores the file's (mtime, len) and
    /// the content hash the dirty check compares against.
    fn note_disk_sync(&mut self) {
        let content = if self.raw_json_editor.editing {
            &self.raw_json_editor.edit_buffer
        } else {
            &self.raw_json_editor.text
        };
        self.disk_hash = hash_content(content);
        self.disk_state = self.current_file_path.as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
        self.file_outdated = false;
    }

    /// Focus-within for the tree pane's rim: the inline value editors float over
    /// tree rows and take ctx focus from the TreeList when a leaf is clicked, but
    /// they are visually part of the tree pane — the rim stays lit for any of
    /// them (and the tree's own search/rename boxes). It goes out only when focus
    /// genuinely leaves the pane: the raw editor, the File menu, or nothing.
    /// Called after every input dispatch (the only places focus moves).
    fn sync_tree_focus_rim(&mut self) {
        let ui = &self.ui_context;
        let outside = !ui.has_focus()
            || ui.is_focused_id(self.raw_json_editor.id())
            || ui.is_focused_id(self.btn_open.id());
        self.tree_list.focused = !outside;
    }

    fn sync_preview_selection(&mut self) {
        if let Some(idx) = self.selected_key_idx {
            if idx < self.flat_keys.len() {
                let path = &self.flat_keys[idx].0;
                let tokens = parse_path(path);
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                if let Some((start, end)) = find_kdl_span(content, &tokens) {
                    self.raw_json_editor.select_anchor = Some(start);
                    self.raw_json_editor.cursor_idx = end;
                    self.raw_json_editor.sync_editor_state();
                    self.raw_json_editor.scroll_to_cursor();
                    return;
                }
            }
        }
        self.raw_json_editor.select_anchor = None;
        self.raw_json_editor.sync_editor_state();
    }

    /// Adopt the tree's selection and seed the inline value editors from the
    /// selected key's value — the shared tail of every selection path that
    /// goes through `TreeList::select_and_show_key` (raw-pane click sync,
    /// `--select` deep link).
    fn adopt_tree_selection(&mut self) {
        self.selected_key_idx = self.tree_list.selected_key_idx;
        if let Some(idx) = self.selected_key_idx {
            self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
            self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
            self.selected_value_editor.editing = false;

            let val = &self.flat_keys[idx].1;
            if let serde_json::Value::String(s) = val {
                if let Some(c) = parse_hex_color(s) {
                    self.selected_color_editor.color = c;
                } else {
                    self.selected_font_editor.font_family = s.clone();
                }
            } else if let Some(num) = val.as_i64() {
                self.selected_spinbox_editor.value = num as i32;
            } else if let serde_json::Value::Bool(b) = val {
                self.selected_bool_editor.set_checked(*b);
            }
        }
        self.sync_preview_selection();
    }

    /// The (bevel) preview's click action: open the cce-bevel material
    /// editor, or refocus the one this session already spawned. cce-bevel
    /// saves to config.kdl itself; the disk-sync watch reloads the document
    /// here when it does.
    fn open_bevel_editor(&mut self) {
        let home = std::env::var("HOME").unwrap_or_default();
        if let Some(child) = self.bevel_child.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                let ccectl = format!("{home}/.local/bin/ccectl");
                let ccectl = if std::path::Path::new(&ccectl).exists() { ccectl } else { "ccectl".to_string() };
                let _ = std::process::Command::new(ccectl).args(["focus-window", "cce-bevel"]).spawn();
                return;
            }
            self.bevel_child = None;
        }
        let local = format!("{home}/.local/bin/cce-bevel");
        let cmd = if std::path::Path::new(&local).exists() { local } else { "cce-bevel".to_string() };
        let mut command = std::process::Command::new(&cmd);
        // Target the file being edited: per-app configs get their own
        // material instead of cce-bevel's default shared-config.kdl save.
        if let Some(ref path) = self.current_file_path {
            command.args(["--config", &path.to_string_lossy()]);
        }
        match command.spawn() {
            Ok(child) => self.bevel_child = Some(child),
            Err(e) => self.status_message = Some((format!("cce-bevel launch failed: {e}"), true)),
        }
    }

    fn rebuild_tree(&mut self) {
        self.tree_list.selected_key_idx = self.selected_key_idx;
        let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
        
        let needs_anno_rebuild = content != &self.cached_content || self.flat_keys != self.cached_flat_keys;
        if needs_anno_rebuild {
            self.cached_content = content.clone();
            self.cached_flat_keys = self.flat_keys.clone();
            
            let keys: Vec<String> = self.flat_keys.iter().map(|(k, _)| k.clone()).collect();
            self.cached_annotations = cce_ui::config::get_kdl_type_annotations(content, &keys);
            
            self.tree_list.annotations = self.cached_annotations.clone();
            self.tree_list.set_flat_keys(self.flat_keys.clone());
        }
    }

    /// The pre-frame widget-state refresh — everything `rebuild_text_items` did EXCEPT
    /// building TextItems (all rendered text is display-list prims now: widget text from the
    /// paint walk, the toolbar file label from `display_list` directly).
    fn refresh_widget_text(&mut self) {
        self.rebuild_tree();
        // Glyph-advance shaping — load-bearing for cursor↔pixel mapping in the editors.
        self.raw_json_editor.prepare_text(&mut self.font_system);
        self.selected_value_editor.prepare_text(&mut self.font_system);
        self.selected_keybind_editor.prepare_text(&mut self.font_system);
        self.tree_list.prepare_text(&mut self.font_system);

        // Status Bar indicators
        if let Some((msg, is_error)) = &self.status_message {
            let color = if *is_error { [0.98, 0.32, 0.32, 1.0] } else { [0.25, 0.75, 0.34, 1.0] };
            self.statusbar.set_text(msg);
            self.statusbar.set_text_color(color);
        } else {
            let msg = format!("Keys: {} | Selected Index: {:?}", self.flat_keys.len(), self.selected_key_idx);
            self.statusbar.set_text(&msg);
            self.statusbar.set_text_color([0.51, 0.51, 0.54, 1.0]);
        }
        self.statusbar.prepare_text(&mut self.font_system);
        cce_ui::widget::WidgetHost::prepare_text(&mut self.menubar, &mut self.font_system);
    }
}

impl Application for DataEditorApp {
    type Message = AppMessage;

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
    }

    fn ui_context_mut(&mut self) -> Option<&mut cce_ui::context::UiContext> {
        Some(&mut self.ui_context)
    }

    fn is_movable_backplate_at(&self, px: f32, py: f32) -> bool {
        if self.split.dragging || self.split.hovered {
            return false;
        }
        // Root Backplate dissolved: the surface itself is the movable plate; drag anywhere a
        // drag-blocking widget isn't.
        self.ui_context.drag_allowed_at(px, py)
    }

    fn new(_qh: &QueueHandle<EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        println!("RUNNING DATA EDITOR DROPDOWN COLOR: {:?}", cce_ui::colors::dropdown_background_color());



        let mut selected_value_editor = TextBox::new(String::new()).with_multiline(false).with_draw_bg_border(true);
        selected_value_editor.set_placeholder("value (e.g. 42, true, \"hello\")");
        let selected_color_editor = ColorSelector::new([255, 255, 255]);
        let selected_spinbox_editor = Spinbox::new(0, -1000000, 1000000, 1);
        let selected_font_editor = FontSelector::new(String::new());
        let selected_choice_editor = Dropdown::new(Vec::new(), 0);
        let selected_keybind_editor = KeybindRecorder::new(String::new());
        let selected_bool_editor = Checkbox::new();
        let selected_button_editor = Button::new(0.0, 0.0, 125.0, 26.0).with_label("Send Test");
        let selected_bevel_editor = cce_ui::widget::BevelPreview::new();

        let mut raw_json_editor = TextBox::new(String::new())
            .with_multiline(true)
            .with_draw_bg_border(true)
            .with_max_width(None);
        raw_json_editor.font_family = "monospace".to_string();
        raw_json_editor.font_size = 13.0;

        // Auto-load argument path if passed. `--select <flat.path>` deep-links
        // to a key (or section prefix) once the first frame has laid out.
        let args: Vec<String> = std::env::args().collect();
        let mut current_file_path = None;
        let mut flat_keys = Vec::new();
        let mut pending_select = None;
        let mut file_arg = None;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--select" {
                if i + 1 < args.len() {
                    pending_select = Some(args[i + 1].clone());
                    i += 1;
                }
            } else if file_arg.is_none() {
                file_arg = Some(args[i].clone());
            }
            i += 1;
        }
        if let Some(arg) = file_arg {
            let path = std::path::PathBuf::from(&arg);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    raw_json_editor.text = content;
                    raw_json_editor.edit_buffer = raw_json_editor.text.clone();
                    current_file_path = Some(path);
                    let val = cce_ui::config::parse_kdl_to_json(&raw_json_editor.text);
                    flatten_json(&val, "", &mut flat_keys);
                }
            }
        }

        // Load recent files list
        let mut recent = cce_ui::config::load_recent_files();

        if let Some(ref path) = current_file_path {
            if let Ok(abs_path) = std::fs::canonicalize(path) {
                let abs_str = abs_path.to_string_lossy().to_string();
                recent.retain(|p| p != &abs_str);
                recent.insert(0, abs_str);
                if recent.len() > 10 {
                    recent.truncate(10);
                }
                cce_ui::config::save_recent_files(&recent);
            }
        }

        let mut dropdown_options = vec![
            "Save".to_string(),
            "Save As".to_string(),
            "Refresh".to_string(),
            "Format".to_string(),
            "-".to_string(),
            "Open...".to_string(),
        ];
        if !recent.is_empty() {
            dropdown_options.push("-".to_string());
            dropdown_options.extend(recent);
        }
        dropdown_options.push("-".to_string());
        dropdown_options.push("Exit".to_string());
        let mut btn_open = Dropdown::new(dropdown_options, 0).with_custom_display_text("File");
        btn_open.set_rect(10.0, 8.0, 70.0, 26.0);

        let mut tree_list = TreeList::new();
        let keys_to_anno: Vec<String> = flat_keys.iter().map(|(k, _)| k.clone()).collect();
        let cached_annotations = cce_ui::config::get_kdl_type_annotations(&raw_json_editor.text, &keys_to_anno);
        tree_list.annotations = cached_annotations.clone();
        tree_list.set_flat_keys(flat_keys.clone());

        let cached_content = raw_json_editor.text.clone();
        let cached_flat_keys = flat_keys.clone();

        // Disk-sync baseline for the startup file (later loads/saves go
        // through note_disk_sync).
        let disk_hash = hash_content(&raw_json_editor.text);
        let disk_state = current_file_path.as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok().map(|t| (t, m.len())));

        // Recessed: no bar background, the window backplate shows through and is shaded to
        // read as carved into it. Supersedes the old opaque .with_color([0.08,0.08,0.12,1]).
        let menubar = MenuBar::new(0.0, 0.0, 800.0, 42.0).with_recess(true);
        let statusbar = StatusBar::new()
            .with_recess(true)
            .with_text_offset_x(15.0);

            let split = SplitPane::new(0.49, 100.0, 100.0, cce_ui::layout::backplate_gap());

            Self {
                keys: DataEditorKeys::load(),
                menubar,
                statusbar,
                btn_open,
                flat_keys,
                selected_key_idx: None,
                pending_select,
                tree_list,
                selected_value_editor,
                selected_color_editor,
                selected_spinbox_editor,
                selected_font_editor,
                selected_choice_editor,
                selected_keybind_editor,
                selected_bool_editor,
                selected_button_editor,
                selected_bevel_editor,
                bevel_child: None,
                raw_json_editor,
                current_file_path,
                status_message: None,
                disk_state,
                disk_hash,
                file_outdated: false,
                disk_poll: 0.0,
                msg_sender: sender,
                file_dialog_open: false,
                width: 800,
                height: 600,
                scale_factor: 1.0,
                font_system: cce_ui::create_font_system(),
                needs_rebuild: true,
                ui_context: cce_ui::context::UiContext::new(),
                ctrl_pressed: false,
                initial_focus: true,
                widgets_registered: false,
                cached_content,
                cached_flat_keys,
                cached_annotations,
                split,
            }
    }

    fn settings(&self) -> WindowSettings {
        WindowSettings {
            title: "Clear KDL Data Editor".to_string(),
            app_id: "cce-data-editor".to_string(),
            width: 800,
            height: 600,
            fullscreen: false,
            min_size: Some((600, 450)),
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, exit: &mut bool) {
        match msg {
            AppMessage::Exit => {
                *exit = true;
            }
            AppMessage::OpenDocument => {
                // The portal dialog runs on a thread; the result arrives as
                // OpenPicked. Blocking here would freeze the loop with the
                // dropdown still expanded — its contraction animates while the
                // dialog is up.
                if self.file_dialog_open {
                    return;
                }
                self.file_dialog_open = true;
                let sender = self.msg_sender.clone();
                std::thread::spawn(move || {
                    let picked = cce_ui::file_dialog::pick_file(
                        "Open KDL Document",
                        &[("KDL Documents", &["kdl"]), ("All Files", &["*"])],
                    );
                    let _ = sender.send(AppMessage::OpenPicked(picked));
                });
            }
            AppMessage::OpenPicked(picked) => {
                self.file_dialog_open = false;
                if let Some(path) = picked {
                    if let Err(e) = self.open_file_by_path(path) {
                        self.status_message = Some((e, true));
                    }
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            AppMessage::OpenRecent(path) => {
                println!("[DEBUG] update: AppMessage::OpenRecent received, path = {:?}", path);
                if let Err(e) = self.open_file_by_path(path) {
                    self.status_message = Some((e, true));
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            AppMessage::SaveDocument => {
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                if let Err(e) = content.parse::<kdl::KdlDocument>() {
                    self.status_message = Some((format!("Cannot save: invalid KDL ({})", e), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return;
                }
                
                if let Some(path) = self.current_file_path.clone() {
                    match std::fs::write(&path, content) {
                        Ok(_) => {
                            if self.raw_json_editor.editing {
                                self.raw_json_editor.text = self.raw_json_editor.edit_buffer.clone();
                            }
                            self.status_message = Some((format!("Saved to {}", path.file_name().unwrap_or_default().to_string_lossy()), false));
                            self.add_recent_file(&path);
                            self.note_disk_sync();
                        }
                        Err(e) => {
                            self.status_message = Some((format!("Save failed: {}", e), true));
                        }
                    }
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                } else {
                    self.update(AppMessage::SaveDocumentAs, needs_rebuild, exit);
                }
            }
            AppMessage::SaveDocumentAs => {
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                if let Err(e) = content.parse::<kdl::KdlDocument>() {
                    self.status_message = Some((format!("Cannot save: invalid KDL ({})", e), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return;
                }
                // Same threaded-picker shape as OpenDocument; the write happens
                // in SaveAsPicked (content re-read and re-validated there —
                // the document can change while the dialog is up).
                if self.file_dialog_open {
                    return;
                }
                self.file_dialog_open = true;
                let sender = self.msg_sender.clone();
                std::thread::spawn(move || {
                    let picked = cce_ui::file_dialog::save_file(
                        "Save KDL Document",
                        &[("KDL Documents", &["kdl"]), ("All Files", &["*"])],
                    );
                    let _ = sender.send(AppMessage::SaveAsPicked(picked));
                });
            }
            AppMessage::SaveAsPicked(picked) => {
                self.file_dialog_open = false;
                let Some(path) = picked else {
                    return;
                };
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                if let Err(e) = content.parse::<kdl::KdlDocument>() {
                    self.status_message = Some((format!("Cannot save: invalid KDL ({})", e), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return;
                }
                match std::fs::write(&path, content) {
                    Ok(_) => {
                        if self.raw_json_editor.editing {
                            self.raw_json_editor.text = self.raw_json_editor.edit_buffer.clone();
                        }
                        self.current_file_path = Some(path.clone());
                        self.status_message = Some((format!("Saved to {}", path.file_name().unwrap_or_default().to_string_lossy()), false));
                        self.add_recent_file(&path);
                        self.note_disk_sync();
                    }
                    Err(e) => {
                        self.status_message = Some((format!("Save failed: {}", e), true));
                    }
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            AppMessage::FormatJson => {
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                match content.parse::<kdl::KdlDocument>() {
                    Ok(doc) => {
                        let pretty = doc.to_string();
                        self.raw_json_editor.text = pretty;
                        self.raw_json_editor.edit_buffer = self.raw_json_editor.text.clone();
                        self.raw_json_editor.editing = false;
                        self.raw_json_editor.sync_editor_state();
                        
                        let val = cce_ui::config::parse_kdl_to_json(&self.raw_json_editor.text);
                        self.flat_keys.clear();
                        flatten_json(&val, "", &mut self.flat_keys);
                        self.status_message = Some(("Formatted successfully".to_string(), false));
                        self.sync_preview_selection();
                    }
                    Err(e) => {
                        self.status_message = Some((format!("KDL error: {}", e), true));
                    }
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            AppMessage::RefreshDocument => {
                if let Some(path) = &self.current_file_path {
                    match std::fs::read_to_string(path) {
                        Ok(content) => {
                            // Selection survives the reload by key name (the
                            // raw-reparse convention): captured before the
                            // rebuild, re-resolved after.
                            let selected_key = self.selected_key_idx
                                .and_then(|idx| self.flat_keys.get(idx).map(|(k, _)| k.clone()));

                            self.raw_json_editor.text = content;
                            self.raw_json_editor.edit_buffer = self.raw_json_editor.text.clone();
                            self.raw_json_editor.cursor_idx = 0;
                            self.raw_json_editor.select_anchor = None;
                            self.raw_json_editor.editing = false;

                            self.flat_keys.clear();
                            if self.raw_json_editor.text.parse::<kdl::KdlDocument>().is_ok() {
                                let val = cce_ui::config::parse_kdl_to_json(&self.raw_json_editor.text);
                                flatten_json(&val, "", &mut self.flat_keys);
                                self.status_message = Some(("Refreshed from disk".to_string(), false));
                            } else {
                                let val = cce_ui::config::parse_kdl_to_json(&self.raw_json_editor.text);
                                flatten_json(&val, "", &mut self.flat_keys);
                                self.status_message = Some(("Refreshed, but KDL is syntactically invalid".to_string(), true));
                            }

                            self.selected_key_idx = selected_key
                                .as_deref()
                                .and_then(|k| self.flat_keys.iter().position(|(key, _)| key == k));
                            if let Some(idx) = self.selected_key_idx {
                                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                                self.selected_value_editor.editing = false;

                                // Sync controls (the raw-reparse pattern).
                                let val = &self.flat_keys[idx].1;
                                if let serde_json::Value::String(s) = val {
                                    if let Some(c) = parse_hex_color(s) {
                                        self.selected_color_editor.color = c;
                                    } else {
                                        self.selected_font_editor.font_family = s.clone();
                                    }
                                } else if let Some(num) = val.as_i64() {
                                    self.selected_spinbox_editor.value = num as i32;
                                } else if let serde_json::Value::Bool(b) = val {
                                    self.selected_bool_editor.set_checked(*b);
                                }
                            } else {
                                self.selected_value_editor.text.clear();
                                self.selected_value_editor.edit_buffer.clear();
                                self.selected_value_editor.editing = false;
                            }
                            self.sync_preview_selection();
                            self.note_disk_sync();
                        }
                        Err(e) => {
                            self.status_message = Some((format!("Refresh failed: {}", e), true));
                        }
                    }
                } else {
                    self.status_message = Some(("No file is currently open".to_string(), true));
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }

            AppMessage::ApplyValue => {
                if let Some(idx) = self.selected_key_idx {
                    let val_str = if self.selected_value_editor.editing { &self.selected_value_editor.edit_buffer } else { &self.selected_value_editor.text };
                    let val_trimmed = val_str.trim();
                    match serde_json::from_str::<serde_json::Value>(val_trimmed) {
                        Ok(mut parsed) => {
                            if let Some(num_f) = parsed.as_f64() {
                                if let Some(annotation) = cce_ui::config::get_kdl_type_annotation(&self.raw_json_editor.text, &self.flat_keys[idx].0) {
                                    if annotation.starts_with("f64:") {
                                        let range_str = annotation.trim_start_matches("f64:");
                                        if let Some(dash_idx) = range_str.find('-') {
                                            let min_str = &range_str[..dash_idx].trim();
                                            let max_str = &range_str[dash_idx + 1..].trim();
                                            if let (Ok(min_f), Ok(max_f)) = (min_str.parse::<f64>(), max_str.parse::<f64>()) {
                                                let clamped = num_f.clamp(min_f, max_f);
                                                if let Some(num) = serde_json::Number::from_f64(clamped) {
                                                    parsed = serde_json::Value::Number(num);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            self.flat_keys[idx].1 = parsed;
                            self.update_raw_from_flat();
                            self.status_message = Some(("Value applied".to_string(), false));
                            self.sync_preview_selection();

                            // Sync controls:
                            let val = &self.flat_keys[idx].1;
                            if let serde_json::Value::String(s) = val {
                                if let Some(c) = parse_hex_color(s) {
                                    self.selected_color_editor.color = c;
                                } else {
                                    self.selected_font_editor.font_family = s.clone();
                                }
                            } else if let Some(num) = val.as_i64() {
                                self.selected_spinbox_editor.value = num as i32;
                            }
                        }
                        Err(e) => {
                            if !val_trimmed.starts_with('"') && !val_trimmed.starts_with('[') && !val_trimmed.starts_with('{') {
                                let parsed = serde_json::Value::String(val_trimmed.to_string());
                                self.flat_keys[idx].1 = parsed;
                                self.update_raw_from_flat();
                                
                                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                                self.selected_value_editor.editing = false;
                                
                                // Sync controls:
                                let val = &self.flat_keys[idx].1;
                                if let serde_json::Value::String(s) = val {
                                    if let Some(c) = parse_hex_color(s) {
                                        self.selected_color_editor.color = c;
                                    } else {
                                        self.selected_font_editor.font_family = s.clone();
                                    }
                                } else if let Some(num) = val.as_i64() {
                                    self.selected_spinbox_editor.value = num as i32;
                                }

                                self.status_message = Some(("Value applied as string".to_string(), false));
                                self.sync_preview_selection();
                            } else {
                                self.status_message = Some((format!("Parse Error: must be valid JSON value ({})", e), true));
                            }
                        }
                    }
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            AppMessage::DeleteKey => {
                if let Some(idx) = self.selected_key_idx {
                    let name = self.flat_keys[idx].0.clone();
                    self.flat_keys.remove(idx);
                    self.selected_key_idx = None;
                    self.selected_value_editor.text.clear();
                    self.selected_value_editor.edit_buffer.clear();
                    self.selected_value_editor.editing = false;
                    self.update_raw_from_flat();
                    self.sync_preview_selection();
                    self.status_message = Some((format!("Deleted key: {}", name), false));
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
    }

    fn tick(&mut self, _dt: f32, needs_rebuild: &mut bool) {
        if self.ui_context.tick(_dt) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        // One-shot `--select` deep link. Deferred to here (not `new`) because
        // scroll_to_selected_key needs the tree's solved viewport height, which
        // exists only after the first frame has laid out.
        if self.pending_select.is_some() && self.tree_list.scroll_box.viewport_h > 0.0 {
            let sel = self.pending_select.take().unwrap();
            // Exact key first; else treat it as a section prefix and land on
            // the section's first key ("style.status" → its first child).
            let key_path = if self.flat_keys.iter().any(|(k, _)| k == &sel) {
                Some(sel.clone())
            } else {
                let prefix = format!("{}.", sel);
                self.flat_keys.iter().map(|(k, _)| k).find(|k| k.starts_with(&prefix)).cloned()
            };
            match key_path {
                Some(kp) if self.tree_list.select_and_show_key(&kp) => {
                    self.adopt_tree_selection();
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
                _ => {
                    self.status_message = Some((format!("--select: key '{}' not found", sel), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
            }
        }
        // Disk-sync watch: once a second, compare the open file's (mtime, len)
        // against the recorded loaded/saved state. Another writer touched it:
        // reload in place when the in-memory document is clean, else raise the
        // outdated indicator and leave the local edits alone.
        self.disk_poll += _dt;
        if self.disk_poll >= 1.0 {
            self.disk_poll = 0.0;
            if let (Some(path), Some(recorded)) = (self.current_file_path.clone(), self.disk_state) {
                let on_disk = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
                if let Some(on_disk) = on_disk {
                    if on_disk != recorded && !self.file_outdated {
                        let content = if self.raw_json_editor.editing {
                            &self.raw_json_editor.edit_buffer
                        } else {
                            &self.raw_json_editor.text
                        };
                        if hash_content(content) == self.disk_hash {
                            let mut exit = false;
                            self.update(AppMessage::RefreshDocument, needs_rebuild, &mut exit);
                            self.status_message = Some(("Reloaded: file changed on disk".to_string(), false));
                        } else {
                            self.file_outdated = true;
                            self.status_message = Some((
                                "File changed on disk — unsaved edits here; Refresh discards them, Save overwrites the disk version".to_string(),
                                true,
                            ));
                        }
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                    }
                }
            }
        }
        if let Some((old_path, new_path)) = self.tree_list.take_rename_request() {
            self.rename_key_path(&old_path, &new_path);
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if let Some(new_key) = self.tree_list.take_new_key_path_request() {
            let trimmed = new_key.trim().to_string();
            if trimmed.is_empty() {
                self.status_message = Some(("Key path cannot be empty".to_string(), true));
            } else if self.flat_keys.iter().any(|(k, _)| k == &trimmed) {
                self.status_message = Some(("Key path already exists".to_string(), true));
            } else {
                self.flat_keys.push((trimmed.clone(), serde_json::Value::String(String::new())));
                self.update_raw_from_flat();
                if let Some(idx) = self.flat_keys.iter().position(|(k, _)| k == &trimmed) {
                    self.selected_key_idx = Some(idx);
                    self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                    self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                    self.selected_value_editor.editing = false;
                    self.sync_preview_selection();
                }
                self.status_message = Some((format!("Added key: {}", trimmed), false));
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_color_editor.tick(_dt, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_font_editor.tick(_dt, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_choice_editor.tick(_dt, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        if self.selected_value_editor.take_change() {
            let mut exit = false;
            self.update(AppMessage::ApplyValue, needs_rebuild, &mut exit);
        }
        if self.selected_bool_editor.take_change() {
            if let Some(idx) = self.selected_key_idx {
                let new_val = serde_json::Value::Bool(self.selected_bool_editor.checked());
                self.flat_keys[idx].1 = new_val;
                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                self.selected_value_editor.editing = false;
                self.update_raw_from_flat();
                self.sync_preview_selection();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        if self.selected_color_editor.take_change() {
            if let Some(idx) = self.selected_key_idx {
                let hex_str = self.selected_color_editor.get_value_string().unwrap_or_else(|| format_hex_color(self.selected_color_editor.color));
                let new_val = serde_json::Value::String(hex_str.clone());
                self.flat_keys[idx].1 = new_val;
                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                self.selected_value_editor.editing = false;
                self.update_raw_from_flat();
                self.sync_preview_selection();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        if self.selected_spinbox_editor.take_change() {
            if let Some(idx) = self.selected_key_idx {
                let new_val = serde_json::Value::Number(self.selected_spinbox_editor.value.into());
                self.flat_keys[idx].1 = new_val;
                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                self.selected_value_editor.editing = false;
                self.update_raw_from_flat();
                self.sync_preview_selection();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        if self.selected_font_editor.take_change() {
            if let Some(idx) = self.selected_key_idx {
                let new_val = serde_json::Value::String(self.selected_font_editor.font_family.clone());
                self.flat_keys[idx].1 = new_val;
                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                self.selected_value_editor.editing = false;
                self.update_raw_from_flat();
                self.sync_preview_selection();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        if self.selected_choice_editor.take_change() {
            if let Some(idx) = self.selected_key_idx {
                let selected_opt = self.selected_choice_editor.options[self.selected_choice_editor.selected].clone();
                let new_val = match &self.flat_keys[idx].1 {
                    serde_json::Value::Bool(_) => {
                        serde_json::Value::Bool(selected_opt.parse::<bool>().unwrap_or(false))
                    }
                    serde_json::Value::Number(_) => {
                        if let Ok(i) = selected_opt.parse::<i64>() {
                            serde_json::Value::Number(i.into())
                        } else if let Ok(f) = selected_opt.parse::<f64>() {
                            serde_json::Value::Number(serde_json::Number::from_f64(f).unwrap())
                        } else {
                            serde_json::Value::String(selected_opt)
                        }
                    }
                    _ => serde_json::Value::String(selected_opt),
                };
                self.flat_keys[idx].1 = new_val;
                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                self.selected_value_editor.editing = false;
                self.update_raw_from_flat();
                self.sync_preview_selection();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        if self.selected_keybind_editor.tick(_dt, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_keybind_editor.take_change() {
            if let Some(idx) = self.selected_key_idx {
                let new_val = serde_json::Value::String(self.selected_keybind_editor.value.clone());
                self.flat_keys[idx].1 = new_val;
                self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                self.selected_value_editor.editing = false;
                self.update_raw_from_flat();
                self.sync_preview_selection();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        if self.raw_json_editor.take_change() {
            let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
            if content.parse::<kdl::KdlDocument>().is_ok() {
                let val = cce_ui::config::parse_kdl_to_json(content);
                let mut new_flat = Vec::new();
                flatten_json(&val, "", &mut new_flat);
                
                let selected_key = self.selected_key_idx.and_then(|idx| self.flat_keys.get(idx).map(|(k, _)| k.clone()));
                self.flat_keys = new_flat;
                
                if let Some(k) = selected_key {
                    self.selected_key_idx = self.flat_keys.iter().position(|(key, _)| key == &k);
                    if let Some(idx) = self.selected_key_idx {
                        self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                        if !self.selected_value_editor.editing {
                            self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                        }
                        
                        // Sync controls:
                        let val = &self.flat_keys[idx].1;
                        if let serde_json::Value::String(s) = val {
                            if let Some(c) = parse_hex_color(s) {
                                self.selected_color_editor.color = c;
                            } else {
                                self.selected_font_editor.font_family = s.clone();
                            }
                        } else if let Some(num) = val.as_i64() {
                            self.selected_spinbox_editor.value = num as i32;
                        } else if let serde_json::Value::Bool(b) = val {
                            self.selected_bool_editor.set_checked(*b);
                        }
                    } else {
                        self.selected_value_editor.text.clear();
                        self.selected_value_editor.edit_buffer.clear();
                        self.selected_value_editor.editing = false;
                    }
                }
                if !self.raw_json_editor.editing {
                    self.sync_preview_selection();
                }
                self.status_message = None;
            } else {
                self.status_message = Some(("KDL syntax error in text editor".to_string(), true));
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        // Phase 6 single paint path: setup/relayout (the old view() body), then the whole
        // frame — the Backplate tree walked into one list plus the toolbar file label — is
        // built here. Widget text comes from the paint walk; TreeList (a legacy subtree
        // painter) serves its rows' text through the walk's bounded-getter emission.
        if !self.widgets_registered {
            self.widgets_registered = true;
            // The root Backplate is DISSOLVED (Phase 6): top-level widgets register directly
            // (parentless) and the window plate is emitted below as prims. The splitter still
            // owns its two panes.
            let self_ptr = self as *mut Self;
            unsafe {
                self.ui_context.register_widget(self.btn_open.base().id(), (*self_ptr).btn_open.as_ptr_mut());
                self.ui_context.register_widget(self.tree_list.base().id(), (*self_ptr).tree_list.as_ptr_mut());
                self.ui_context.register_widget(self.selected_value_editor.base().id(), (*self_ptr).selected_value_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_color_editor.base().id(), (*self_ptr).selected_color_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_spinbox_editor.base().id(), (*self_ptr).selected_spinbox_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_font_editor.base().id(), (*self_ptr).selected_font_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_choice_editor.base().id(), (*self_ptr).selected_choice_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_keybind_editor.base().id(), (*self_ptr).selected_keybind_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_bool_editor.base().id(), (*self_ptr).selected_bool_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_button_editor.base().id(), (*self_ptr).selected_button_editor.as_ptr_mut());
                self.ui_context.register_widget(self.selected_bevel_editor.base().id(), (*self_ptr).selected_bevel_editor.as_ptr_mut());
                self.ui_context.register_widget(self.menubar.id(), (*self_ptr).menubar.as_ptr_mut());
                self.ui_context.register_widget(self.statusbar.base().id(), (*self_ptr).statusbar.as_ptr_mut());
                self.ui_context.register_widget(self.raw_json_editor.base().id(), (*self_ptr).raw_json_editor.as_ptr_mut());
            }
            self.ui_context.rebuild_spatial_grid();
        }

        if self.initial_focus {
            self.initial_focus = false;
            self.ui_context.set_focused(&mut self.raw_json_editor);
            WidgetHost::focus(&mut self.raw_json_editor);
            self.needs_rebuild = true;
        }
        
        let size_changed = self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale;
        if self.needs_rebuild || size_changed {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            
            // Layout via the scene solver (Phase 6ac): the frame is a stretched column
            // [menubar (fixed 42, padded 10/8, holding the File-menu leaf), content row
            // (grow, padded 10, gap = the divider width, panes growing by the SplitPane
            // fractions), statusbar (fixed 30)]. Solves to the exact 6aa hand rects; the
            // SplitPane keeps the divider drag/hover state and its rect derives from the
            // solved panes. The inline value editors stay hand-positioned below — they
            // float over tree rows, not a static tree.
            {
                use cce_ui::scene::arena::Arena;
                use cce_ui::scene::layout::{
                    compute_layout, CrossAlign, Edges, LayoutBox, Length, Size as LSize, Style,
                };
                let mut arena: Arena<LayoutBox> = Arena::new();
                let root = arena.insert(LayoutBox::container(
                    Style::column().cross_align(CrossAlign::Stretch),
                ));
                // DE-wide plate rim padding (style.surface.backplate.padding);
                // the pane gap is the SplitPane's, seeded from backplate_gap.
                let plate_pad = cce_ui::layout::backplate_padding();
                // The File dropdown nests into the window's top-left corner:
                // equal gap to the left and top edges, so its corner_frame
                // adjustment (below) rounds it concentric with the plate.
                let menu_gap = 8.0;
                let top_bar = arena.insert(LayoutBox::container({
                    let mut s = Style::row().height(Length::Fixed(42.0));
                    s.padding = Edges { left: menu_gap, right: plate_pad, top: menu_gap, bottom: 42.0 - menu_gap - 26.0 };
                    s
                }));
                let menu_leaf = arena.insert(LayoutBox::leaf(Style::row(), LSize::new(70.0, 26.0)));
                let content = arena.insert(LayoutBox::container({
                    let mut s = Style::row()
                        .grow(1.0)
                        .padding(plate_pad)
                        .gap(self.split.gap)
                        .cross_align(CrossAlign::Stretch);
                    s.min_height = 120.0;
                    s
                }));
                let tree_pane = arena.insert(LayoutBox::container(Style::column().grow(self.split.frac)));
                let editor_pane = arena.insert(LayoutBox::container(Style::column().grow(1.0 - self.split.frac)));
                let status = arena.insert(LayoutBox::container(
                    Style::column().height(Length::Fixed(30.0)),
                ));
                arena.append_child(root, top_bar);
                arena.append_child(top_bar, menu_leaf);
                arena.append_child(root, content);
                arena.append_child(content, tree_pane);
                arena.append_child(content, editor_pane);
                arena.append_child(root, status);
                compute_layout(&mut arena, root, LSize::new(self.width as f32, self.height as f32));

                let tb = arena.value(top_bar).unwrap().rect;
                self.menubar.set_rect(tb.x, tb.y, tb.width, tb.height);
                let mb = arena.value(menu_leaf).unwrap().rect;
                self.btn_open.set_rect(mb.x, mb.y, mb.width, mb.height);
                // Concentric with the window plate: equal left/top gaps make
                // the corner_frame adjustment round the dropdown's top-left
                // corner to (plate radius - gap), following the window curve.
                self.btn_open.set_corner_frame(Some((
                    (0.0, 0.0, self.width as f32, self.height as f32),
                    cce_ui::colors::backplate_corner_radius(),
                    (true, true, true, true),
                )));
                let tr = arena.value(tree_pane).unwrap().rect;
                self.tree_list.set_rect(tr.x, tr.y, tr.width, tr.height);
                let er = arena.value(editor_pane).unwrap().rect;
                self.raw_json_editor.set_rect(er.x, er.y, er.width, er.height);
                let st = arena.value(status).unwrap().rect;
                self.statusbar.set_rect(st.x, st.y, st.width, st.height);
                // The divider's hit/drag frame spans both panes (same fractions, same math).
                self.split.set_rect(tr.x, tr.y, (er.x + er.width) - tr.x, tr.height);
            }

            // Position the selected value editor inline inside the list if visible
            if let Some(selected_idx) = self.selected_key_idx {
                if let Some((row_x, row_y, _row_w, _row_h)) = self.tree_list.get_row_rect(selected_idx) {
                    let val = &self.flat_keys[selected_idx].1;
                    let key_name = &self.flat_keys[selected_idx].0;
                    let is_font_type = key_name == "font" || key_name.ends_with("_font") || key_name.ends_with(".font");
                    
                    let mut is_menu_type = false;
                    let mut menu_options = Vec::new();
                    let mut is_button_type = false;
                    let mut is_bevel_type = false;
                    let mut annotation_str = None;
                    if let Some(annotation) = cce_ui::config::get_kdl_type_annotation(&self.raw_json_editor.text, key_name) {
                        annotation_str = Some(annotation.clone());
                        if annotation.starts_with("menu:") {
                            is_menu_type = true;
                            let opts_str = annotation.trim_start_matches("menu:");
                            menu_options = opts_str.split(',').map(|s| s.trim().to_string()).collect();
                        } else if annotation == "button" || annotation.starts_with("button:") {
                            is_button_type = true;
                        } else if annotation == "bevel" {
                            is_bevel_type = true;
                        }
                    }
                    // Parked unless the bevel branch below places it (the other
                    // branches never show it, so one shared park suffices).
                    self.selected_bevel_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);

                    if is_bevel_type {
                        // The (bevel) preview: a mini lit cross-section of the
                        // knob triple; clicking it opens cce-bevel.
                        if let serde_json::Value::String(st) = val {
                            self.selected_bevel_editor.set_knobs_str(st);
                        }
                        self.selected_bevel_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                        self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_keybind_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_button_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    } else if is_menu_type {
                        self.selected_choice_editor.options = menu_options.clone();
                        let val_str = match val {
                            serde_json::Value::String(st) => st.clone(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Number(n) => n.to_string(),
                            _ => String::new(),
                        };
                        if let Some(pos) = menu_options.iter().position(|o| o == &val_str) {
                            self.selected_choice_editor.selected = pos;
                        } else {
                            self.selected_choice_editor.selected = 0;
                        }
                        self.selected_choice_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                        self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_keybind_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_button_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    } else if is_button_type {
                        let val_str = match val {
                            serde_json::Value::String(st) => st.clone(),
                            _ => "Trigger".to_string(),
                        };
                        self.selected_button_editor.base_mut().label = Some(val_str);
                        self.selected_button_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                        self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_keybind_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    } else {
                        self.selected_button_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        let is_keybind_type = key_name == "key" || key_name == "keybind" || key_name == "shortcut" || key_name == "delete" || key_name.ends_with("_key") || key_name.ends_with(".key") || key_name.ends_with(".keybind") || key_name.ends_with("_delete") || key_name.ends_with(".delete") || key_name == "brightness_up" || key_name == "brightness_down" || key_name.ends_with(".brightness_up") || key_name.ends_with(".brightness_down") || annotation_str.as_deref() == Some("keybind");
                        if is_keybind_type {
                            let val_str = match val {
                                serde_json::Value::String(st) => st.clone(),
                                _ => String::new(),
                            };
                            self.selected_keybind_editor.value = val_str;
                            self.selected_keybind_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                            self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        } else {
                            self.selected_keybind_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            if let serde_json::Value::String(s) = val {
                                self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                if s.starts_with('#') {
                                    self.selected_color_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                                    self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                    self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                    self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                } else if is_font_type {
                                    self.selected_font_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                                    self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                    self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                    self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                } else {
                                    self.selected_value_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                                    self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                    self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                    self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                }
                            } else if val.is_i64() {
                                self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_spinbox_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                                self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            } else if val.is_boolean() {
                                self.selected_bool_editor.set_rect(row_x + 245.0, row_y + 1.0, 26.0, 26.0);
                                self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            } else {
                                self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_value_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                                self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                                self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            }
                        }
                    }
                } else {
                    self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_keybind_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_button_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_bevel_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                }
            } else {
                self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_keybind_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_bool_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_button_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_bevel_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
            }
            

            
            self.refresh_widget_text();
            self.ui_context.rebuild_spatial_grid();
            self.needs_rebuild = false;
        }

        // Popover registration — ui_context ONLY (drives the engine's dl-text occlusion
        // clamp). The popovers and the context menu draw into this display list below;
        // the global registry fed the engine's render-only xdg popup, no longer used.
        self.ui_context.clear_popovers();
        if self.selected_choice_editor.popover_rect().is_some() {
            self.ui_context.register_popover(&mut self.selected_choice_editor);
        }
        if self.btn_open.popover_rect().is_some() {
            self.ui_context.register_popover(&mut self.btn_open);
        }

        let mut pc = cce_ui::scene::paint::PaintCtx::new();

        // The dissolved root Backplate's plate — its exact legacy paint: page-low background
        // at the active backplate opacity, config corner radius (Backplate::color /
        // corner_radius defaults; this window set no border/bevel).
        {
            use cce_ui::scene::layout::Rect;
            let mut plate_color = cce_ui::color::page_low_color();
            if plate_color[3] > 0.001 {
                plate_color[3] = cce_ui::color::active_backplate_opacity();
            }
            let rect = Rect { x: 0.0, y: 0.0, width: self.width as f32, height: self.height as f32 };
            let radius = cce_ui::colors::backplate_corner_radius();
            if radius > 0.1 {
                // One glass slab: the fill plus a rolled, lit perimeter. The menubar and
                // statusbar then sink into this surface as steps (see their with_recess),
                // so the whole window reads as a single piece with varying depth rather
                // than stacked opaque bars.
                pc.plate(rect, (radius, radius, radius, radius), plate_color, cce_ui::layout::bevel_width());
            } else if plate_color[3] > 0.001 {
                pc.quad(rect, plate_color);
            }
        }

        // Top-level widgets walked in the old child order (menubar, statusbar, top bar, then
        // the dissolved splitter's slot: its divider quad and the two panes walked as
        // separate roots), and the inline editors LAST.
        {
            // The walk takes shared borrows now — no self-alias, no pointers.
            let tops: [&dyn cce_ui::widget::WidgetHost; 3] =
                [&self.menubar, &self.statusbar, &self.btn_open];
            for top in tops {
                cce_ui::scene::painter::paint_root_into(&self.ui_context, top, &mut pc);
            }
            {
                use cce_ui::scene::layout::Rect;
                let (dx, dy, dw, dh, dc) = self.split.divider_quad();
                pc.quad(Rect { x: dx, y: dy, width: dw, height: dh }, dc);
            }
            {
                let panes: [&dyn cce_ui::widget::WidgetHost; 2] = [&self.tree_list, &self.raw_json_editor];
                for pane in panes {
                    cce_ui::scene::painter::paint_root_into(&self.ui_context, pane, &mut pc);
                }
            }
            // The inline value editors float OVER the tree's rows, so they have to be
            // painted after it: TreeList fills a background quad for every visible row,
            // including the selected one it leaves a value-cell hole in. Painted before the
            // tree, an editor's box, border and caret all landed under that quad — only its
            // text survived, because the engine draws every label after all geometry. Hence
            // "the value control has no caret".
            let editors: [&dyn cce_ui::widget::WidgetHost; 9] = [
                &self.selected_value_editor,
                &self.selected_color_editor,
                &self.selected_spinbox_editor,
                &self.selected_font_editor,
                &self.selected_choice_editor,
                &self.selected_keybind_editor,
                &self.selected_bool_editor,
                &self.selected_button_editor,
                &self.selected_bevel_editor,
            ];
            for editor in editors {
                cce_ui::scene::painter::paint_root_into(&self.ui_context, editor, &mut pc);
            }
        }

        // Toolbar file label — app chrome, not owned by any widget. Outdated
        // (changed on disk under local edits) renders in the highlight accent.
        let file_name_str = match &self.current_file_path {
            Some(path) => path.display().to_string(),
            None => "Untitled".to_string(),
        };
        let (file_label, file_label_color) = if self.file_outdated {
            let hc = cce_ui::color::highlight_primary_color();
            (
                format!("File: {} — changed on disk", file_name_str),
                [(hc[0] * 255.0) as u8, (hc[1] * 255.0) as u8, (hc[2] * 255.0) as u8],
            )
        } else {
            (format!("File: {}", file_name_str), [0xdd, 0xdd, 0xe2])
        };
        pc.text_with(
            file_label,
            self.raw_json_editor.rect().0 + 10.0,
            15.0,
            12.0,
            file_label_color,
            Some(cce_ui::layout::menubar_font()),
            None,
        );

        // Popovers + the global context menu — geometry and labels last, on top of
        // everything, exactly where they hit-test (the engine xdg popup is gone). Labels
        // carry bounds equal to their overlay rect: clips them to the plate and exempts
        // them from the dl-text occlusion clamp (the is-overlay-text convention).
        {
            use cce_ui::scene::layout::Rect;
            for &pop_id in &self.ui_context.active_popovers {
                let Some(pop_ptr) = self.ui_context.tree.get_ptr(pop_id) else { continue };
                let popover = unsafe { &*pop_ptr };
                if popover.popover_rect().is_none() {
                    continue;
                }
                // PaintCtx is a RenderTarget: the popover draws its REAL prims
                // (the dropdown's expanded inset-plate surface) with its own
                // per-label bounds — no flattening collector round-trip.
                popover.render_popover(&mut pc);
            }
            if cce_ui::widget::context_menu::is_visible() {
                let menu_bounds = Some([
                    cce_ui::widget::context_menu::x(),
                    cce_ui::widget::context_menu::y(),
                    cce_ui::widget::context_menu::x() + cce_ui::widget::context_menu::w(),
                    cce_ui::widget::context_menu::y() + cce_ui::widget::context_menu::h(),
                ]);
                for (qx, qy, qw, qh, qc) in cce_ui::widget::context_menu::extra_quads() {
                    pc.quad(Rect { x: qx, y: qy, width: qw, height: qh }, qc);
                }
                for label in cce_ui::widget::context_menu::text_labels() {
                    pc.text_with(
                        label.text.clone(),
                        label.x,
                        label.y,
                        label.font_size,
                        label.color,
                        None,
                        menu_bounds,
                    );
                }
            }
        }

        Some(pc.finish())
    }

    fn display_list_text(&self) -> bool {
        true
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let mut changed = false;
        let px = pos.x as f32;
        let py = pos.y as f32;

        if self.split.cursor_moved(px, py) {
            changed = true;
        }

        if cce_ui::widget::context_menu::is_visible() {
            if cce_ui::widget::context_menu::cursor_moved(px, py) {
                changed = true;
            }
        } else {
            // Routed dispatch (Phase 6ac): one PointerMove through the router per root
            // (hover bookkeeping, Enter/Leave synthesis, drag forwarding).
            let ev = cce_ui::widget::Event::PointerMove { x: px, y: py, local_x: px, local_y: py };
            if self.ui_context.propagate_event(&ev, self.btn_open.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_value_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_color_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_spinbox_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_font_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_choice_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_keybind_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_bool_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_button_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.selected_bevel_editor.id()) { changed = true; }
            if self.ui_context.propagate_event(&ev, self.raw_json_editor.id()) { changed = true; }

            if self.ui_context.propagate_event(&ev, self.tree_list.id()) { changed = true; }
        }

        if changed {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: LogicalPosition, needs_rebuild: &mut bool) -> Option<Self::Message> {
        println!("[DEBUG] handle_mouse_input: button={:?}, state={:?}, pos=({:.1}, {:.1})", button, state, pos.x, pos.y);
        let mut changed = false;
        let mut msg_out = None;
        let px = pos.x as f32;
        let py = pos.y as f32;

        // Routed dispatch (Phase 6ac): one MouseButton event through the router per
        // root — presses are hit-gated per widget, releases delivered everywhere, drag
        // targets recorded; the interleaved take_* plumbing below is unchanged.
        let mouse_ev = cce_ui::widget::Event::MouseButton { button, state, x: px, y: py, local_x: px, local_y: py };

        // The dissolved splitter's divider: a left press grabs it (stealing keyboard
        // focus like the legacy ctx.set_focused_ptr / clear_focus pair did), a release
        // ends the drag.
        if button == MouseButton::Left {
            if state == ElementState::Pressed {
                if self.split.press(px, py) {
                    self.ui_context.clear_focus();
                    self.sync_tree_focus_rim();
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            } else if self.split.release() {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        }

        if cce_ui::widget::context_menu::is_visible() {
            if cce_ui::widget::context_menu::mouse_input(button, state, px, py, Some(&mut self.ui_context)) {
                changed = true;
            }
            if let Some(_deleted_path) = self.tree_list.take_deleted_key_path() {
                if let Some(idx) = self.tree_list.selected_key_idx {
                    self.selected_key_idx = Some(idx);
                }
                msg_out = Some(AppMessage::DeleteKey);
            }
            if changed || msg_out.is_some() {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            if msg_out.is_some() {
                return msg_out;
            }
            return None;
        }

        if self.ui_context.propagate_event(&mouse_ev, self.btn_open.id()) {
            changed = true;
            if self.btn_open.take_change() {
                let selected_idx = self.btn_open.selected;
                if selected_idx < self.btn_open.options.len() {
                    let option_text = &self.btn_open.options[selected_idx];
                    match option_text.as_str() {
                        "Save" => {
                            println!("[DEBUG] File menu selected 'Save', Dispatching SaveDocument");
                            msg_out = Some(AppMessage::SaveDocument);
                        }
                        "Save As" => {
                            println!("[DEBUG] File menu selected 'Save As', Dispatching SaveDocumentAs");
                            msg_out = Some(AppMessage::SaveDocumentAs);
                        }
                        "Refresh" => {
                            println!("[DEBUG] File menu selected 'Refresh', Dispatching RefreshDocument");
                            msg_out = Some(AppMessage::RefreshDocument);
                        }
                        "Format" => {
                            println!("[DEBUG] File menu selected 'Format', Dispatching FormatJson");
                            msg_out = Some(AppMessage::FormatJson);
                        }
                        "Exit" => {
                            println!("[DEBUG] File menu selected 'Exit', Dispatching Exit");
                            msg_out = Some(AppMessage::Exit);
                        }
                        "Open..." | "Other" => {
                            println!("[DEBUG] File menu selected 'Open...', Dispatching OpenDocument");
                            msg_out = Some(AppMessage::OpenDocument);
                        }
                        _ => {
                            let path = std::path::PathBuf::from(option_text);
                            println!("[DEBUG] File menu selected recent file: {:?}", path);
                            msg_out = Some(AppMessage::OpenRecent(path));
                        }
                    }
                }
            }
        }
        let mut editor_handled = false;
        if self.ui_context.propagate_event(&mouse_ev, self.selected_value_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_color_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_spinbox_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_font_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_choice_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_keybind_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_bool_editor.id()) {
            changed = true;
            editor_handled = true;
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_bevel_editor.id()) {
            changed = true;
            editor_handled = true;
            if state == ElementState::Pressed && self.selected_bevel_editor.take_click() {
                self.open_bevel_editor();
            }
        }
        if self.ui_context.propagate_event(&mouse_ev, self.selected_button_editor.id()) {
            changed = true;
            editor_handled = true;
            if state == ElementState::Released && self.selected_button_editor.take_click() {
                if let Some(idx) = self.selected_key_idx {
                    let key_name = &self.flat_keys[idx].0;
                    if let Some(annotation) = cce_ui::config::get_kdl_type_annotation(&self.raw_json_editor.text, key_name) {
                        if annotation.starts_with("button:") {
                            let cmd = annotation.trim_start_matches("button:");
                            println!("[DEBUG] Visual button clicked, running command: {}", cmd);
                            std::process::Command::new("sh")
                                .args(["-c", cmd])
                                .spawn()
                                .ok();
                        } else if key_name == "notifications.test_notification" {
                            println!("[DEBUG] Test notification button clicked, sending D-Bus notification");
                            std::process::Command::new("busctl")
                                .args([
                                    "--user",
                                    "call",
                                    "org.freedesktop.Notifications",
                                    "/org/freedesktop/Notifications",
                                    "org.freedesktop.Notifications",
                                    "Notify",
                                    "susssasa{sv}i",
                                    "cce-client",
                                    "0",
                                    "",
                                    "CCE Test Notification",
                                    "This is a test notification from the Data Editor.",
                                    "0",
                                    "0",
                                    "-1",
                                ])
                                .spawn()
                                .ok();
                        }
                    }
                }
            }
        }
        if self.ui_context.propagate_event(&mouse_ev, self.raw_json_editor.id()) {
            changed = true;
            editor_handled = true;

            if button == MouseButton::Left && state == ElementState::Pressed {
                // TextBox handles the click without claiming ctx focus; claim it
                // here so the tree gets FocusOut and its focus rim clears.
                self.ui_context.set_focused(&mut self.raw_json_editor);
                let cursor_offset = self.raw_json_editor.cursor_idx;
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                
                let mut best_key = None;
                let mut best_span_len = usize::MAX;
                
                for (flat_key, _) in &self.flat_keys {
                    let tokens = parse_path(flat_key);
                    if let Some((start, end)) = find_kdl_span(content, &tokens) {
                        if cursor_offset >= start && cursor_offset <= end {
                            let span_len = end - start;
                            if span_len < best_span_len {
                                best_span_len = span_len;
                                best_key = Some(flat_key.clone());
                            }
                        }
                    }
                }
                
                if let Some(key_path) = best_key {
                    if self.tree_list.select_and_show_key(&key_path) {
                        self.adopt_tree_selection();
                        changed = true;
                    }
                }
            }
        }

        if !editor_handled && self.ui_context.propagate_event(&mouse_ev, self.tree_list.id()) {
            changed = true;
            if let Some((old_path, new_path)) = self.tree_list.take_rename_request() {
                self.rename_key_path(&old_path, &new_path);
            }
            if let Some(clicked_item) = self.tree_list.take_clicked_item() {
                match clicked_item {
                    TreeElement::Section { .. } => {
                        changed = true;
                    }
                    TreeElement::Leaf { original_idx, .. } => {
                        self.selected_key_idx = Some(original_idx);
                        
                        let value_str = serde_json::to_string(&self.flat_keys[original_idx].1).unwrap_or_default();
                        self.selected_value_editor.text = value_str;
                        self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                        self.selected_value_editor.editing = false;
                        
                        // Sync controls:
                        let val = &self.flat_keys[original_idx].1;
                        let key_name = &self.flat_keys[original_idx].0;
                        let is_font_type = key_name == "font" || key_name.ends_with("_font") || key_name.ends_with(".font");
                        
                        let mut is_menu_type = false;
                        let mut menu_options = Vec::new();
                        let mut annotation_str = None;
                        if let Some(annotation) = cce_ui::config::get_kdl_type_annotation(&self.raw_json_editor.text, key_name) {
                            annotation_str = Some(annotation.clone());
                            if annotation.starts_with("menu:") {
                                is_menu_type = true;
                                let opts_str = annotation.trim_start_matches("menu:");
                                menu_options = opts_str.split(',').map(|s| s.trim().to_string()).collect();
                            }
                        }

                        if is_menu_type {
                            self.selected_choice_editor.options = menu_options.clone();
                            let val_str = match val {
                                serde_json::Value::String(st) => st.clone(),
                                serde_json::Value::Bool(b) => b.to_string(),
                                serde_json::Value::Number(n) => n.to_string(),
                                _ => String::new(),
                            };
                            if let Some(pos) = menu_options.iter().position(|o| o == &val_str) {
                                self.selected_choice_editor.selected = pos;
                            } else {
                                self.selected_choice_editor.selected = 0;
                            }
                        } else if let serde_json::Value::String(s) = val {
                            if let Some(rgba) = parse_hex_color_rgba(s) {
                                self.selected_color_editor.color = [rgba[0], rgba[1], rgba[2]];
                                self.selected_color_editor.alpha = rgba[3];
                                let clean_s = s.trim_matches(|c| c == '"' || c == '\'' || c == ' ').trim_start_matches('#');
                                self.selected_color_editor.with_alpha = clean_s.len() == 8;
                            } else {
                                self.selected_font_editor.font_family = s.clone();
                            }
                        } else if let Some(num) = val.as_i64() {
                            self.selected_spinbox_editor.value = num as i32;
                        } else if let serde_json::Value::Bool(b) = val {
                            self.selected_bool_editor.set_checked(*b);
                        }
                        
                        let is_keybind_type = key_name == "key" || key_name == "keybind" || key_name == "shortcut" || key_name == "delete" || key_name.ends_with("_key") || key_name.ends_with(".key") || key_name.ends_with(".keybind") || key_name.ends_with("_delete") || key_name.ends_with(".delete") || key_name == "brightness_up" || key_name == "brightness_down" || key_name.ends_with(".brightness_up") || key_name.ends_with(".brightness_down") || annotation_str.as_deref() == Some("keybind");

                        if is_menu_type {
                            self.ui_context.set_focused(&mut self.selected_choice_editor);
                        } else if is_keybind_type {
                            if let serde_json::Value::String(s) = val {
                                self.selected_keybind_editor.value = s.clone();
                            }
                            self.ui_context.set_focused(&mut self.selected_keybind_editor);
                        } else if let serde_json::Value::String(s) = val {
                            if s.starts_with('#') {
                                self.ui_context.set_focused(&mut self.selected_color_editor);
                            } else if is_font_type {
                                self.ui_context.set_focused(&mut self.selected_font_editor);
                            } else {
                                self.ui_context.set_focused(&mut self.selected_value_editor);
                                WidgetHost::focus(&mut self.selected_value_editor);
                            }
                        } else if val.is_i64() {
                            self.ui_context.set_focused(&mut self.selected_spinbox_editor);
                        } else if val.is_boolean() {
                            self.ui_context.set_focused(&mut self.selected_bool_editor);
                        } else {
                            self.ui_context.set_focused(&mut self.selected_value_editor);
                            WidgetHost::focus(&mut self.selected_value_editor);
                        }
                        self.sync_preview_selection();
                        changed = true;
                    }
                }
            }
        } else if !editor_handled && button == MouseButton::Left && state == ElementState::Pressed {
            let (rx, ry, rw, rh) = self.raw_json_editor.rect();
            let in_raw = px >= rx && px <= rx + rw && py >= ry && py <= ry + rh;
            
            if !in_raw {
                self.raw_json_editor.unfocus();
                self.selected_value_editor.unfocus();
                self.selected_color_editor.unfocus();
                self.selected_spinbox_editor.unfocus();
                self.selected_font_editor.unfocus();
                self.selected_choice_editor.unfocus();
                self.selected_keybind_editor.unfocus();
                self.tree_list.unfocus();
                // The manual unfocus sweep above leaves ctx.focused_widget stale;
                // clear it so focus-derived state (the tree rim) sees reality.
                self.ui_context.clear_focus();
                changed = true;
            }
        }

        self.sync_tree_focus_rim();
        if changed {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        msg_out
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let px = pos.x as f32;
        let py = pos.y as f32;
        // Routed dispatch (Phase 6ac): hit-scoped per root, like the legacy direct calls.
        let wheel_ev = cce_ui::widget::Event::MouseWheel { delta: delta.clone(), x: px, y: py, local_x: px, local_y: py };
        if self.ui_context.propagate_event(&wheel_ev, self.tree_list.id()) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.ui_context.propagate_event(&wheel_ev, self.selected_color_editor.id()) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.ui_context.propagate_event(&wheel_ev, self.selected_spinbox_editor.id()) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.ui_context.propagate_event(&wheel_ev, self.selected_font_editor.id()) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.ui_context.propagate_event(&wheel_ev, self.selected_choice_editor.id()) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.ui_context.propagate_event(&wheel_ev, self.raw_json_editor.id()) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Self::Message> {
        self.ctrl_pressed = event.ctrl;
        self.ui_context.ctrl_pressed = event.ctrl;
        self.ui_context.shift_pressed = event.shift;

        if event.state == ElementState::Pressed && cce_ui::widget::context_menu::is_visible() {
            cce_ui::widget::context_menu::hide();
            *needs_rebuild = true;
            self.needs_rebuild = true;
            return None;
        }

        if event.state == ElementState::Pressed && self.status_message.is_some() {
            self.status_message = None;
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        let mut handled = false;
        let mut msg_out = None;

        if event.state == ElementState::Pressed {
            let open_search_shortcut = cce_ui::color::tree_open_search_key();
            if match_key_shortcut(event, &open_search_shortcut) {
                let has_ctrl = open_search_shortcut.to_lowercase().contains("ctrl") || open_search_shortcut.to_lowercase().contains("control");
                if has_ctrl || self.ui_context.focused_widget.is_none() {
                    self.tree_list.focus_search(&mut self.ui_context);
                    handled = true;
                }
            }
        }

        // Keyboard Shortcuts (input.kdl `cce-data-editor` domain); the
        // font-size chords stay hardcoded (+/= don't round-trip chords).
        if !handled && event.state == ElementState::Pressed {
            let m = |chord: &str| cce_ui::widget::match_key_shortcut(event, chord);
            if m(&self.keys.open_document) {
                msg_out = Some(AppMessage::OpenDocument);
                handled = true;
            } else if m(&self.keys.save_document) {
                msg_out = Some(AppMessage::SaveDocument);
                handled = true;
            } else if m(&self.keys.quit) {
                msg_out = Some(AppMessage::Exit);
                handled = true;
            } else if m(&self.keys.format_json) {
                msg_out = Some(AppMessage::FormatJson);
                handled = true;
            }
        }
        if !handled && event.ctrl && event.state == ElementState::Pressed {
            if let Key::Character(ref ch) = event.logical_key {
                match ch.to_lowercase().as_str() {
                    "=" | "+" => {
                        self.raw_json_editor.font_size = (self.raw_json_editor.font_size + 1.0).min(72.0);
                        self.selected_value_editor.font_size = (self.selected_value_editor.font_size + 1.0).min(72.0);
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        handled = true;
                    }
                    "-" | "_" => {
                        self.raw_json_editor.font_size = (self.raw_json_editor.font_size - 1.0).max(6.0);
                        self.selected_value_editor.font_size = (self.selected_value_editor.font_size - 1.0).max(6.0);
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        handled = true;
                    }
                    _ => {}
                }
            }
        }

        // Routed dispatch (Phase 6ac). The router delivers KeyInput to the ctx-focused
        // widget FIRST on every call, so the legacy anything-goes chain would deliver a
        // typed key to the focused widget once per call site — the chain short-circuits
        // on first handled now (at most one widget is focused/editing at a time; the
        // legacy non-else chain relied on exactly that). Plumbing that used to key off
        // WHICH call returned true is gated on widget state instead: Enter applies the
        // value when the value editor was editing when the key arrived, no matter which
        // call site's propagate consumed it.
        let value_was_editing = self.selected_value_editor.editing;
        if !handled {
            let key_ev = cce_ui::widget::Event::KeyInput(event.clone());
            let roots: [cce_ui::widget::WidgetId; 10] = [
                self.tree_list.id(),
                self.btn_open.id(),
                self.raw_json_editor.id(),
                self.selected_value_editor.id(),
                self.selected_color_editor.id(),
                self.selected_spinbox_editor.id(),
                self.selected_font_editor.id(),
                self.selected_choice_editor.id(),
                self.selected_keybind_editor.id(),
                self.selected_bool_editor.id(),
            ];
            for root in roots {
                if self.ui_context.propagate_event(&key_ev, root) {
                    handled = true;
                    break;
                }
            }
            if handled {
                if self.btn_open.take_change() {
                    let selected_idx = self.btn_open.selected;
                    if selected_idx < self.btn_open.options.len() {
                        let option_text = &self.btn_open.options[selected_idx];
                        match option_text.as_str() {
                            "Save" => msg_out = Some(AppMessage::SaveDocument),
                            "Save As" => msg_out = Some(AppMessage::SaveDocumentAs),
                            "Refresh" => msg_out = Some(AppMessage::RefreshDocument),
                            "Format" => msg_out = Some(AppMessage::FormatJson),
                            "Exit" => msg_out = Some(AppMessage::Exit),
                            "Open..." | "Other" => msg_out = Some(AppMessage::OpenDocument),
                            _ => {
                                msg_out = Some(AppMessage::OpenRecent(std::path::PathBuf::from(option_text)));
                            }
                        }
                    }
                }
                if value_was_editing
                    && event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(cce_ui::widget::NamedKey::Enter)
                {
                    msg_out = Some(AppMessage::ApplyValue);
                }
            }
        }

        self.sync_tree_focus_rim();
        if handled {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        msg_out
    }
}

fn parse_hex_color_rgba(s: &str) -> Option<[u8; 4]> {
    cce_ui::color::parse_hex_bytes(s)
}

fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    parse_hex_color_rgba(s).map(|rgba| [rgba[0], rgba[1], rgba[2]])
}

fn match_key_shortcut(event: &KeyEvent, shortcut_str: &str) -> bool {
    let shortcut_lower = shortcut_str.to_lowercase();
    let parts: Vec<&str> = shortcut_lower.split('+').collect();
    
    let mut req_ctrl = false;
    let mut req_shift = false;
    let mut req_key = "";
    
    for part in parts {
        match part {
            "ctrl" | "control" => req_ctrl = true,
            "shift" => req_shift = true,
            "super" | "win" | "logo" | "alt" | "meta" => {}
            k => req_key = k,
        }
    }
    
    if event.ctrl != req_ctrl { return false; }
    if event.shift != req_shift { return false; }
    
    if let Key::Character(ref ch) = event.logical_key {
        let ch_lower = ch.to_lowercase();
        if req_key.len() == 1 {
            return ch_lower == req_key;
        } else {
            let mapped_key = match req_key {
                "slash" => "/",
                "enter" => "enter",
                "escape" => "escape",
                "space" => " ",
                k => k,
            };
            return ch_lower == mapped_key;
        }
    } else if let Key::Named(named) = event.logical_key {
        let named_str = match named {
            cce_ui::widget::NamedKey::Enter => "enter",
            cce_ui::widget::NamedKey::Escape => "escape",
            cce_ui::widget::NamedKey::Space => "space",
            _ => "",
        };
        return named_str == req_key;
    }
    false
}

fn format_hex_color(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn hash_content(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();
    
    cce_ui::engine::run::<DataEditorApp>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kdl_roundtrip() {
        let content = std::fs::read_to_string(cce_ui::config::get_config_path()).unwrap();
        let val = cce_ui::config::parse_kdl_to_json(&content);
        let mut flat = Vec::new();
        flatten_json(&val, "", &mut flat);
        for (k, v) in &flat {
            println!("FLAT KEY: {:?} = {:?}", k, v);
        }
        
        let mut found = false;
        for (k, v) in &mut flat {
            if k == "style.status.background_color" {
                *v = serde_json::Value::String("#151520e6".to_string());
                found = true;
            }
        }
        assert!(found, "Should find style.status.background_color!");

        let root = unflatten_json(&flat);
        let new_kdl = cce_ui::config::json_to_kdl_string(&root);
        println!("Generated KDL:\n{}", new_kdl);
        match new_kdl.parse::<kdl::KdlDocument>() {
            Ok(_) => println!("Parsed OK!"),
            Err(e) => panic!("Failed to parse generated KDL: {}", e),
        }
    }

    #[test]
    fn test_kdl_span_lookup() {
        let content = std::fs::read_to_string(cce_ui::config::get_config_path()).unwrap();
        let val = cce_ui::config::parse_kdl_to_json(&content);
        let mut flat = Vec::new();
        flatten_json(&val, "", &mut flat);
        
        for (k, _) in &flat {
            let tokens = parse_path(k);
            let span = find_kdl_span(&content, &tokens);
            assert!(span.is_some(), "Should find span for path: {} with tokens: {:?}", k, tokens);
        }
    }
}
