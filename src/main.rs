use wayland_client::QueueHandle;
use glyphon::{FontSystem, Buffer, Metrics, Attrs};
use cce_ui::engine::{Application, EngineState, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{
    MouseButton, ElementState, MouseScrollDelta, KeyEvent, TextItem, Element,
    TextBox, Button, TextLabel, Key, Backplate, TreeList, TreeElement, ColorSelector, Spinbox, FontSelector, Dropdown
};

#[derive(Debug, Clone)]
enum AppMessage {
    Exit,
    OpenDocument,
    SaveDocument,
    SaveDocumentAs,
    FormatJson,
    RefreshDocument,
    AddKey,
    ApplyValue,
    DeleteKey,
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

fn bottom_y_calc(height: u32) -> f32 {
    height as f32 - 180.0
}

struct DataEditorApp {
    // Toolbar Buttons
    btn_open: Button,
    btn_save: Button,
    btn_save_as: Button,
    btn_format: Button,
    btn_refresh: Button,
    btn_exit: Button,

    // Left Panel Form Edit
    flat_keys: Vec<(String, serde_json::Value)>,
    selected_key_idx: Option<usize>,
    tree_list: TreeList,

    // New Key input
    new_key_editor: TextBox,
    btn_add_key: Button,

    // Edit Value input
    selected_value_editor: TextBox,
    selected_color_editor: ColorSelector,
    selected_spinbox_editor: Spinbox,
    selected_font_editor: FontSelector,
    selected_choice_editor: Dropdown,
    btn_apply_val: Button,
    btn_delete_key: Button,

    // Right Panel Raw Json
    raw_json_editor: TextBox,

    // App state
    current_file_path: Option<std::path::PathBuf>,
    status_message: Option<(String, bool)>,

    // UI state
    root_window: Backplate,
    width: u32,
    height: u32,
    scale_factor: f64,
    text_items: Vec<TextItem>,
    font_system: FontSystem,
    needs_rebuild: bool,
    ui_context: cce_ui::context::UiContext,
    ctrl_pressed: bool,
    initial_focus: bool,
    widgets_registered: bool,
}

fn value_to_kdl(key: &str, val: &serde_json::Value, indent: usize) -> String {
    let indent_str = "    ".repeat(indent);
    match val {
        serde_json::Value::Object(map) => {
            let has_objects = map.values().any(|v| v.is_object());
            if has_objects {
                let mut out = format!("{}{} {{\n", indent_str, key);
                for (k, v) in map {
                    out.push_str(&value_to_kdl(k, v, indent + 1));
                }
                out.push_str(&format!("{}}}\n", indent_str));
                out
            } else {
                let mut prop_parts = Vec::new();
                for (prop_name, prop_val) in map {
                    let (val_str, val_ty) = match prop_val {
                        serde_json::Value::Bool(b) => (b.to_string(), Some("bool")),
                        serde_json::Value::Number(num) => {
                            if num.is_f64() {
                                (num.to_string(), Some("f64"))
                            } else {
                                (num.to_string(), Some("i64"))
                            }
                        }
                        serde_json::Value::String(s) => {
                            if s.starts_with('#') {
                                let s_clean = s.trim_start_matches('#');
                                let ty = if s_clean.len() == 8 { "rgba" } else { "rgb" };
                                (format!("\"{}\"", s), Some(ty))
                            } else {
                                (format!("\"{}\"", s), None)
                            }
                        }
                        _ => (prop_val.to_string(), None),
                    };
                    if let Some(ty) = val_ty {
                        prop_parts.push(format!("{}=({}){}", prop_name, ty, val_str));
                    } else {
                        prop_parts.push(format!("{}={}", prop_name, val_str));
                    }
                }
                format!("{}{} {}\n", indent_str, key, prop_parts.join(" "))
            }
        }
        serde_json::Value::Array(arr) => {
            let mut out = String::new();
            for item in arr {
                out.push_str(&value_to_kdl(key, item, indent));
            }
            out
        }
        _ => {
            let (val_str, val_ty) = match val {
                serde_json::Value::Bool(b) => (b.to_string(), Some("bool")),
                serde_json::Value::Number(num) => {
                    if num.is_f64() {
                        (num.to_string(), Some("f64"))
                    } else {
                        (num.to_string(), Some("i64"))
                    }
                }
                serde_json::Value::String(s) => {
                    if s.starts_with('#') {
                        let s_clean = s.trim_start_matches('#');
                        let ty = if s_clean.len() == 8 { "rgba" } else { "rgb" };
                        (format!("\"{}\"", s), Some(ty))
                    } else {
                        (format!("\"{}\"", s), None)
                    }
                }
                _ => (val.to_string(), None),
            };
            if let Some(ty) = val_ty {
                format!("{}{} ({}){}\n", indent_str, key, ty, val_str)
            } else {
                format!("{}{} {}\n", indent_str, key, val_str)
            }
        }
    }
}

fn json_to_kdl_string(val: &serde_json::Value) -> String {
    let mut out = String::new();
    if let serde_json::Value::Object(map) = val {
        for (sec_name, sec_val) in map {
            if let serde_json::Value::Object(sec_map) = sec_val {
                out.push_str(&format!("{} {{\n", sec_name));
                for (k, v) in sec_map {
                    out.push_str(&value_to_kdl(k, v, 1));
                }
                out.push_str("}\n");
            } else {
                out.push_str(&value_to_kdl(sec_name, sec_val, 0));
            }
        }
    }
    out
}

impl DataEditorApp {

    fn pick_file_to_open(&self) -> Result<std::path::PathBuf, String> {
        println!("[DEBUG] pick_file_to_open: Executing XDG desktop portal file chooser");
        match cce_ui::file_dialog::pick_file("Open KDL Document", &[("KDL Documents", &["kdl"]), ("All Files", &["*"])]) {
            Some(path) => Ok(path),
            None => Err("No file selected".to_string()),
        }
    }

    fn pick_file_to_save(&self) -> Result<std::path::PathBuf, String> {
        println!("[DEBUG] pick_file_to_save: Executing XDG desktop portal file chooser");
        match cce_ui::file_dialog::save_file("Save KDL Document", &[("KDL Documents", &["kdl"]), ("All Files", &["*"])]) {
            Some(path) => Ok(path),
            None => Err("No file selected".to_string()),
        }
    }

    fn update_raw_from_flat(&mut self) {
        let root = unflatten_json(&self.flat_keys);
        let pretty = json_to_kdl_string(&root);
        self.raw_json_editor.text = pretty;
        self.raw_json_editor.edit_buffer = self.raw_json_editor.text.clone();
        self.raw_json_editor.sync_editor_state();
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
                    return;
                }
            }
        }
        self.raw_json_editor.select_anchor = None;
        self.raw_json_editor.sync_editor_state();
    }

    fn rebuild_tree(&mut self) {
        self.tree_list.selected_key_idx = self.selected_key_idx;
        self.tree_list.set_flat_keys(self.flat_keys.clone());
    }

    fn add_element_labels(
        element: &dyn Element,
        ui_context: &cce_ui::context::UiContext,
        font_system: &mut FontSystem,
        text_items: &mut Vec<TextItem>,
        scale: f32,
    ) {
        for (label, font_family, bounds) in element.text_labels_with_font_and_bounds(ui_context) {
            let physical_size = label.font_size * scale;
            let metrics = Metrics::new(physical_size, physical_size * 1.4);
            let mut buf = Buffer::new(font_system, metrics);
            let mut attrs = Attrs::new();
            
            // Keep family_str alive for the whole iteration so Family::Name(&family) borrow is valid
            let family_str = font_family.clone();
            if let Some(ref family) = family_str {
                let family_val = match family.as_str() {
                    "monospace" => glyphon::Family::Name(cce_ui::layout::get_system_monospace_font()),
                    "sans-serif" => glyphon::Family::SansSerif,
                    "serif" => glyphon::Family::Serif,
                    _ => glyphon::Family::Name(family),
                };
                attrs = attrs.family(family_val);
            }
            buf.set_text(font_system, &label.text, attrs, glyphon::Shaping::Advanced);
            buf.shape_until_scroll(font_system, true);
            text_items.push(TextItem {
                buffer: buf,
                x: label.x,
                y: label.y,
                color: glyphon::Color::rgb(label.color[0], label.color[1], label.color[2]),
                bounds,
            });
        }
    }

    fn rebuild_text_items(&mut self) {
        self.rebuild_tree();
        self.raw_json_editor.prepare_text(&mut self.font_system);
        self.selected_value_editor.prepare_text(&mut self.font_system);
        self.new_key_editor.prepare_text(&mut self.font_system);
        self.text_items.clear();
        let scale = cce_ui::scale::scale_factor();
        let mut labels = Vec::new();

        // 1. Button labels
        labels.extend(self.btn_open.text_labels());
        labels.extend(self.btn_save.text_labels());
        labels.extend(self.btn_save_as.text_labels());
        labels.extend(self.btn_format.text_labels());
        labels.extend(self.btn_refresh.text_labels());
        labels.extend(self.btn_exit.text_labels());
        labels.extend(self.btn_add_key.text_labels());
        labels.extend(self.btn_apply_val.text_labels());
        labels.extend(self.btn_delete_key.text_labels());

        // 2. Section labels
        let bottom_y = bottom_y_calc(self.height);
        labels.push(TextLabel {
            text: "Add New Key:".to_string(),
            x: 10.0,
            y: bottom_y - 6.0,
            font_size: 11.0,
            color: [0x83, 0x83, 0x8a],
        });

        let selected_key_name = match self.selected_key_idx {
            Some(idx) => {
                let k = &self.flat_keys[idx].0;
                if k.len() > 30 {
                    format!("Edit Key: ...{}", &k[k.len() - 27..])
                } else {
                    format!("Edit Key: {}", k)
                }
            }
            None => "No Key Selected".to_string(),
        };
        labels.push(TextLabel {
            text: selected_key_name,
            x: 10.0,
            y: bottom_y + 54.0,
            font_size: 11.0,
            color: [0x83, 0x83, 0x8a],
        });

        // 3. File path info in toolbar
        let file_name_str = match &self.current_file_path {
            Some(path) => path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            None => "Untitled".to_string(),
        };
        labels.push(TextLabel {
            text: format!("File: {}", file_name_str),
            x: 420.0,
            y: 15.0,
            font_size: 12.0,
            color: [0xdd, 0xdd, 0xe2],
        });

        // 4. Status Bar indicators
        if let Some((msg, is_error)) = &self.status_message {
            let color = if *is_error { [0xfa, 0x52, 0x52] } else { [0x40, 0xc0, 0x57] };
            labels.push(TextLabel {
                text: msg.clone(),
                x: (self.width as f32 - 400.0).max(300.0),
                y: self.height as f32 - 20.0,
                font_size: 11.0,
                color,
            });
        } else {
            labels.push(TextLabel {
                text: format!("Keys: {} | Selected Index: {:?}", self.flat_keys.len(), self.selected_key_idx),
                x: 15.0,
                y: self.height as f32 - 20.0,
                font_size: 11.0,
                color: [0x83, 0x83, 0x8a],
            });
        }

        // 5. Add Textbox / Element contents to text_items
        Self::add_element_labels(
            &self.raw_json_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
        Self::add_element_labels(
            &self.selected_value_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
        Self::add_element_labels(
            &self.selected_color_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
        Self::add_element_labels(
            &self.selected_spinbox_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
        Self::add_element_labels(
            &self.selected_font_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
        Self::add_element_labels(
            &self.selected_choice_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
        Self::add_element_labels(
            &self.new_key_editor,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );

        // 6. Build static text items
        for label in labels {
            let physical_size = label.font_size * scale;
            let metrics = Metrics::new(physical_size, physical_size * 1.4);
            let mut buf = Buffer::new(&mut self.font_system, metrics);
            buf.set_text(&mut self.font_system, &label.text, Attrs::new(), glyphon::Shaping::Advanced);
            buf.shape_until_scroll(&mut self.font_system, true);
            self.text_items.push(TextItem {
                buffer: buf,
                x: label.x,
                y: label.y,
                color: glyphon::Color::rgb(label.color[0], label.color[1], label.color[2]),
                bounds: None,
            });
        }

        // 7. Left List rows text with clip bounds via TreeList
        Self::add_element_labels(
            &self.tree_list,
            &self.ui_context,
            &mut self.font_system,
            &mut self.text_items,
            scale,
        );
    }
}

impl Application for DataEditorApp {
    type Message = AppMessage;

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
    }

    fn new(_qh: &QueueHandle<EngineState<Self>>, _sender: calloop::channel::Sender<Self::Message>) -> Self {
        let btn_open = Button::new(10.0, 8.0, 70.0, 26.0).with_label("Open");
        let btn_save = Button::new(90.0, 8.0, 70.0, 26.0).with_label("Save");
        let btn_save_as = Button::new(170.0, 8.0, 80.0, 26.0).with_label("Save As");
        let btn_format = Button::new(260.0, 8.0, 80.0, 26.0).with_label("Format");
        let btn_refresh = Button::new(350.0, 8.0, 80.0, 26.0).with_label("Refresh");
        let btn_exit = Button::new(720.0, 8.0, 70.0, 26.0).with_label("Exit");

        let mut new_key_editor = TextBox::new(String::new()).with_multiline(false).with_draw_bg_border(true);
        new_key_editor.set_placeholder("new.key.path");
        let btn_add_key = Button::new(280.0, 430.0, 100.0, 26.0).with_label("Add Key");

        let mut selected_value_editor = TextBox::new(String::new()).with_multiline(false).with_draw_bg_border(true);
        selected_value_editor.set_placeholder("value (e.g. 42, true, \"hello\")");
        let selected_color_editor = ColorSelector::new([255, 255, 255]);
        let selected_spinbox_editor = Spinbox::new(0, -1000000, 1000000, 1);
        let selected_font_editor = FontSelector::new(String::new());
        let selected_choice_editor = Dropdown::new(Vec::new(), 0);
        let btn_apply_val = Button::new(10.0, 490.0, 100.0, 26.0).with_label("Apply");
        let btn_delete_key = Button::new(120.0, 490.0, 100.0, 26.0).with_label("Delete");

        let mut raw_json_editor = TextBox::new(String::new())
            .with_multiline(true)
            .with_draw_bg_border(true)
            .with_max_width(None);
        raw_json_editor.font_family = "monospace".to_string();
        raw_json_editor.font_size = 13.0;

        // Auto-load argument path if passed
        let args: Vec<String> = std::env::args().collect();
        let mut current_file_path = None;
        let mut flat_keys = Vec::new();
        if args.len() > 1 {
            let path = std::path::PathBuf::from(&args[1]);
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

        let mut tree_list = TreeList::new();
        tree_list.set_flat_keys(flat_keys.clone());

        let root_window = Backplate::new(0.0, 0.0, 800.0, 600.0)
            .with_movable(true);

        Self {
            root_window,
            btn_open,
            btn_save,
            btn_save_as,
            btn_format,
            btn_refresh,
            btn_exit,
            flat_keys,
            selected_key_idx: None,
            tree_list,
            new_key_editor,
            btn_add_key,
            selected_value_editor,
            selected_color_editor,
            selected_spinbox_editor,
            selected_font_editor,
            selected_choice_editor,
            btn_apply_val,
            btn_delete_key,
            raw_json_editor,
            current_file_path,
            status_message: None,
            width: 800,
            height: 600,
            scale_factor: 1.0,
            text_items: Vec::new(),
            font_system: {
                let mut fs = FontSystem::new();
                fs.db_mut().load_fonts_dir("/home/lsgalante/Dropbox/Fonts");
                fs
            },
            needs_rebuild: true,
            ui_context: cce_ui::context::UiContext::new(),
            ctrl_pressed: false,
            initial_focus: true,
            widgets_registered: false,
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
                println!("[DEBUG] update: AppMessage::OpenDocument received");
                match self.pick_file_to_open() {
                    Ok(path) => {
                        println!("[DEBUG] pick_file_to_open succeeded, path = {:?}", path);
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
                                self.current_file_path = Some(path);
                                self.sync_preview_selection();
                            }
                            Err(e) => {
                                self.status_message = Some((format!("Error opening: {}", e), true));
                            }
                        }
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                    }
                    Err(e) => {
                        println!("[DEBUG] pick_file_to_open failed, error = {:?}", e);
                        if e != "No file selected" {
                            self.status_message = Some((format!("File picker error: {}", e), true));
                            *needs_rebuild = true;
                            self.needs_rebuild = true;
                        }
                    }
                }
            }
            AppMessage::SaveDocument => {
                let content = if self.raw_json_editor.editing { &self.raw_json_editor.edit_buffer } else { &self.raw_json_editor.text };
                if let Err(e) = content.parse::<kdl::KdlDocument>() {
                    self.status_message = Some((format!("Cannot save: invalid KDL ({})", e), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return;
                }
                
                if let Some(ref path) = self.current_file_path {
                    match std::fs::write(path, content) {
                        Ok(_) => {
                            if self.raw_json_editor.editing {
                                self.raw_json_editor.text = self.raw_json_editor.edit_buffer.clone();
                            }
                            self.status_message = Some((format!("Saved to {}", path.file_name().unwrap_or_default().to_string_lossy()), false));
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
                
                match self.pick_file_to_save() {
                    Ok(path) => {
                        match std::fs::write(&path, content) {
                            Ok(_) => {
                                if self.raw_json_editor.editing {
                                    self.raw_json_editor.text = self.raw_json_editor.edit_buffer.clone();
                                }
                                self.current_file_path = Some(path.clone());
                                self.status_message = Some((format!("Saved to {}", path.file_name().unwrap_or_default().to_string_lossy()), false));
                            }
                            Err(e) => {
                                self.status_message = Some((format!("Save failed: {}", e), true));
                            }
                        }
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                    }
                    Err(e) => {
                        if e != "No file selected" {
                            self.status_message = Some((format!("File picker error: {}", e), true));
                            *needs_rebuild = true;
                            self.needs_rebuild = true;
                        }
                    }
                }
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
                            
                            self.selected_key_idx = None;
                            self.selected_value_editor.text.clear();
                            self.selected_value_editor.edit_buffer.clear();
                            self.selected_value_editor.editing = false;
                            self.sync_preview_selection();
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
            AppMessage::AddKey => {
                let new_key = if self.new_key_editor.editing { &self.new_key_editor.edit_buffer } else { &self.new_key_editor.text };
                let trimmed = new_key.trim().to_string();
                if trimmed.is_empty() {
                    self.status_message = Some(("Key path cannot be empty".to_string(), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return;
                }
                
                if self.flat_keys.iter().any(|(k, _)| k == &trimmed) {
                    self.status_message = Some(("Key path already exists".to_string(), true));
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return;
                }
                
                self.flat_keys.push((trimmed.clone(), serde_json::Value::String(String::new())));
                self.update_raw_from_flat();
                
                self.new_key_editor.text.clear();
                self.new_key_editor.edit_buffer.clear();
                self.new_key_editor.editing = false;
                
                if let Some(idx) = self.flat_keys.iter().position(|(k, _)| k == &trimmed) {
                    self.selected_key_idx = Some(idx);
                    self.selected_value_editor.text = serde_json::to_string(&self.flat_keys[idx].1).unwrap_or_default();
                    self.selected_value_editor.edit_buffer = self.selected_value_editor.text.clone();
                    self.selected_value_editor.editing = false;
                    self.sync_preview_selection();
                }
                
                self.status_message = Some((format!("Added key: {}", trimmed), false));
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            AppMessage::ApplyValue => {
                if let Some(idx) = self.selected_key_idx {
                    let val_str = if self.selected_value_editor.editing { &self.selected_value_editor.edit_buffer } else { &self.selected_value_editor.text };
                    let val_trimmed = val_str.trim();
                    match serde_json::from_str::<serde_json::Value>(val_trimmed) {
                        Ok(parsed) => {
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

    fn view(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, size: LogicalSize, scale: f64) {
        if !self.widgets_registered {
            self.widgets_registered = true;
            let self_ptr = self as *mut Self;
            unsafe {
                self.ui_context.register_widget(self.root_window.base().unwrap().id(), &mut (*self_ptr).root_window as *mut Backplate as *mut (dyn Element + 'static));
                self.ui_context.register_widget(self.tree_list.base().unwrap().id(), &mut (*self_ptr).tree_list as *mut TreeList as *mut (dyn Element + 'static));
                self.ui_context.register_widget(self.selected_color_editor.base().unwrap().id(), &mut (*self_ptr).selected_color_editor as *mut ColorSelector as *mut (dyn Element + 'static));
                self.ui_context.register_widget(self.selected_spinbox_editor.base().unwrap().id(), &mut (*self_ptr).selected_spinbox_editor as *mut Spinbox as *mut (dyn Element + 'static));
                self.ui_context.register_widget(self.selected_font_editor.base().unwrap().id(), &mut (*self_ptr).selected_font_editor as *mut FontSelector as *mut (dyn Element + 'static));
                self.ui_context.register_widget(self.selected_choice_editor.base().unwrap().id(), &mut (*self_ptr).selected_choice_editor as *mut Dropdown as *mut (dyn Element + 'static));
                
                self.root_window.add_child(self.btn_open.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_save.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_save_as.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_format.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_refresh.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_exit.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_add_key.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_apply_val.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.btn_delete_key.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.tree_list.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.new_key_editor.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.selected_value_editor.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.selected_color_editor.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.selected_spinbox_editor.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.selected_font_editor.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.selected_choice_editor.as_ptr_mut(), &mut self.ui_context);
                self.root_window.add_child(self.raw_json_editor.as_ptr_mut(), &mut self.ui_context);
            }
            self.ui_context.rebuild_spatial_grid();
        }

        if self.initial_focus {
            self.initial_focus = false;
            self.ui_context.set_focused(&mut self.raw_json_editor);
            TextBox::focus(&mut self.raw_json_editor);
            self.needs_rebuild = true;
        }
        
        let size_changed = self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale;
        if self.needs_rebuild || size_changed {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            
            self.root_window.set_rect(0.0, 0.0, self.width as f32, self.height as f32);
            
            // Top bar
            self.btn_open.set_rect(10.0, 8.0, 70.0, 26.0);
            self.btn_save.set_rect(90.0, 8.0, 70.0, 26.0);
            self.btn_save_as.set_rect(170.0, 8.0, 80.0, 26.0);
            self.btn_format.set_rect(260.0, 8.0, 80.0, 26.0);
            self.btn_refresh.set_rect(350.0, 8.0, 80.0, 26.0);
            self.btn_exit.set_rect((self.width as f32 - 80.0).max(440.0), 8.0, 70.0, 26.0);
            
            // Bottom edit area in left panel
            let bottom_y = bottom_y_calc(self.height);
            self.new_key_editor.set_rect(10.0, bottom_y + 10.0, 260.0, 26.0);
            self.btn_add_key.set_rect(280.0, bottom_y + 10.0, 100.0, 26.0);
            
            self.btn_apply_val.set_rect(10.0, bottom_y + 70.0, 100.0, 26.0);
            self.btn_delete_key.set_rect(120.0, bottom_y + 70.0, 100.0, 26.0);

            // Position tree_list
            let list_top = 52.0;
            let list_bottom = bottom_y_calc(self.height);
            let list_height = list_bottom - list_top;
            self.tree_list.set_rect(10.0, list_top, 380.0, list_height);

            // Position the selected value editor inline inside the list if visible
            if let Some(selected_idx) = self.selected_key_idx {
                if let Some((row_x, row_y, _row_w, _row_h)) = self.tree_list.get_row_rect(selected_idx) {
                    let val = &self.flat_keys[selected_idx].1;
                    let key_name = &self.flat_keys[selected_idx].0;
                    let is_font_type = key_name == "font" || key_name.ends_with("_font") || key_name.ends_with(".font");
                    
                    let mut is_menu_type = false;
                    let mut menu_options = Vec::new();
                    if let Some(annotation) = cce_ui::config::get_kdl_type_annotation(&self.raw_json_editor.text, key_name) {
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
                        self.selected_choice_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                        self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    } else {
                        self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        if let serde_json::Value::String(s) = val {
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
                            self.selected_spinbox_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                            self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        } else {
                            self.selected_value_editor.set_rect(row_x + 245.0, row_y + 1.0, 125.0, 26.0);
                            self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                            self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                        }
                    }
                } else {
                    self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                    self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                }
            } else {
                self.selected_value_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_color_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_spinbox_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_font_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
                self.selected_choice_editor.set_rect(-1000.0, -1000.0, 1.0, 1.0);
            }
            
            // Right pane raw editor
            let right_w = (self.width as f32 - 420.0).max(100.0);
            let right_h = (self.height as f32 - 92.0).max(100.0);
            self.raw_json_editor.set_rect(410.0, 52.0, right_w, right_h);
            
            self.rebuild_text_items();
            self.ui_context.rebuild_spatial_grid();
            self.needs_rebuild = false;
        }

        // 1. Editor Window Background
        quads.push((0.0, 0.0, self.width as f32, self.height as f32, [0.05, 0.05, 0.07, 1.0]));

        // 2. Toolbar Header
        quads.push((0.0, 0.0, self.width as f32, 42.0, [0.08, 0.08, 0.12, 1.0]));
        quads.push((0.0, 42.0, self.width as f32, 1.0, [0.18, 0.18, 0.22, 1.0]));

        // 3. Status Bar
        let status_y = self.height as f32 - 30.0;
        quads.push((0.0, status_y, self.width as f32, 30.0, [0.08, 0.08, 0.10, 1.0]));
        quads.push((0.0, status_y, self.width as f32, 1.0, [0.18, 0.18, 0.22, 1.0]));
        


        // 5. Collect all quads recursively from Backplate
        quads.extend(self.root_window.all_quads(&self.ui_context));
    }

    fn text_items(&self) -> &[TextItem] {
        &self.text_items
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let mut changed = false;
        let px = pos.x as f32;
        let py = pos.y as f32;

        if self.btn_open.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_save.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_save_as.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_format.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_refresh.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_exit.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_add_key.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_apply_val.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.btn_delete_key.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        
        if self.new_key_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.selected_value_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.selected_color_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.selected_spinbox_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.selected_font_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.selected_choice_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }
        if self.raw_json_editor.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }

        if self.tree_list.on_cursor_moved(px, py, &mut self.ui_context) { changed = true; }

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

        if self.btn_open.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_open.take_click() {
                println!("[DEBUG] btn_open click registered! Dispatching OpenDocument");
                msg_out = Some(AppMessage::OpenDocument);
            }
        }
        if self.btn_save.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_save.take_click() {
                msg_out = Some(AppMessage::SaveDocument);
            }
        }
        if self.btn_save_as.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_save_as.take_click() {
                msg_out = Some(AppMessage::SaveDocumentAs);
            }
        }
        if self.btn_format.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_format.take_click() {
                msg_out = Some(AppMessage::FormatJson);
            }
        }
        if self.btn_refresh.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_refresh.take_click() {
                msg_out = Some(AppMessage::RefreshDocument);
            }
        }
        if self.btn_exit.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_exit.take_click() {
                msg_out = Some(AppMessage::Exit);
            }
        }
        if self.btn_add_key.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_add_key.take_click() {
                msg_out = Some(AppMessage::AddKey);
            }
        }
        if self.btn_apply_val.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_apply_val.take_click() {
                msg_out = Some(AppMessage::ApplyValue);
            }
        }
        if self.btn_delete_key.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            if state == ElementState::Released && self.btn_delete_key.take_click() {
                msg_out = Some(AppMessage::DeleteKey);
            }
        }

        let mut editor_handled = false;
        if self.new_key_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }
        if self.selected_value_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }
        if self.selected_color_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }
        if self.selected_spinbox_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }
        if self.selected_font_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }
        if self.selected_choice_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }
        if self.raw_json_editor.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
            editor_handled = true;
        }

        if !editor_handled && self.tree_list.mouse_input(button, state, px, py, &mut self.ui_context) {
            changed = true;
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
                        if let Some(annotation) = cce_ui::config::get_kdl_type_annotation(&self.raw_json_editor.text, key_name) {
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
                        }
                        
                        if is_menu_type {
                            self.ui_context.set_focused(&mut self.selected_choice_editor);
                        } else if let serde_json::Value::String(s) = val {
                            if s.starts_with('#') {
                                self.ui_context.set_focused(&mut self.selected_color_editor);
                            } else if is_font_type {
                                self.ui_context.set_focused(&mut self.selected_font_editor);
                            } else {
                                self.ui_context.set_focused(&mut self.selected_value_editor);
                                TextBox::focus(&mut self.selected_value_editor);
                            }
                        } else if val.is_i64() {
                            self.ui_context.set_focused(&mut self.selected_spinbox_editor);
                        } else {
                            self.ui_context.set_focused(&mut self.selected_value_editor);
                            TextBox::focus(&mut self.selected_value_editor);
                        }
                        self.sync_preview_selection();
                        changed = true;
                    }
                }
            }
        } else if button == MouseButton::Left && state == ElementState::Pressed {
            let bottom_y = bottom_y_calc(self.height);
            let in_new_key = px >= 10.0 && px <= 270.0 && py >= bottom_y + 10.0 && py <= bottom_y + 36.0;
            let in_sel_val = px >= 10.0 && px <= 270.0 && py >= bottom_y + 70.0 && py <= bottom_y + 96.0;
            let in_raw = px >= 410.0 && px <= self.width as f32 - 10.0 && py >= 52.0 && py <= self.height as f32 - 40.0;
            
            if !in_new_key && !in_sel_val && !in_raw {
                self.raw_json_editor.unfocus();
                self.new_key_editor.unfocus();
                self.selected_value_editor.unfocus();
                self.selected_color_editor.unfocus();
                self.selected_spinbox_editor.unfocus();
                self.selected_font_editor.unfocus();
                self.selected_choice_editor.unfocus();
                self.tree_list.unfocus();
                changed = true;
            }
        }

        if changed {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        msg_out
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let px = pos.x as f32;
        let py = pos.y as f32;
        if self.tree_list.mouse_wheel(delta, px, py, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_color_editor.mouse_wheel(delta, px, py, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_spinbox_editor.mouse_wheel(delta, px, py, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_font_editor.mouse_wheel(delta, px, py, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.selected_choice_editor.mouse_wheel(delta, px, py, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Self::Message> {
        self.ctrl_pressed = event.ctrl;

        if event.state == ElementState::Pressed && self.status_message.is_some() {
            self.status_message = None;
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        let mut handled = false;
        let mut msg_out = None;

        // Keyboard Shortcuts
        if event.ctrl && event.state == ElementState::Pressed {
            if let Key::Character(ref ch) = event.logical_key {
                match ch.to_lowercase().as_str() {
                    "o" => {
                        msg_out = Some(AppMessage::OpenDocument);
                        handled = true;
                    }
                    "s" => {
                        msg_out = Some(AppMessage::SaveDocument);
                        handled = true;
                    }
                    "q" => {
                        msg_out = Some(AppMessage::Exit);
                        handled = true;
                    }
                    "f" => {
                        msg_out = Some(AppMessage::FormatJson);
                        handled = true;
                    }
                    "=" | "+" => {
                        self.raw_json_editor.font_size = (self.raw_json_editor.font_size + 1.0).min(72.0);
                        self.selected_value_editor.font_size = (self.selected_value_editor.font_size + 1.0).min(72.0);
                        self.new_key_editor.font_size = (self.new_key_editor.font_size + 1.0).min(72.0);
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        handled = true;
                    }
                    "-" | "_" => {
                        self.raw_json_editor.font_size = (self.raw_json_editor.font_size - 1.0).max(6.0);
                        self.selected_value_editor.font_size = (self.selected_value_editor.font_size - 1.0).max(6.0);
                        self.new_key_editor.font_size = (self.new_key_editor.font_size - 1.0).max(6.0);
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        handled = true;
                    }
                    _ => {}
                }
            }
        }

        if !handled {
            if self.raw_json_editor.keyboard_input(event, &mut self.ui_context) { handled = true; }
            if self.selected_value_editor.keyboard_input(event, &mut self.ui_context) {
                handled = true;
                if event.state == ElementState::Pressed && event.logical_key == Key::Named(cce_ui::widget::NamedKey::Enter) {
                    msg_out = Some(AppMessage::ApplyValue);
                }
            }
            if self.selected_color_editor.keyboard_input(event, &mut self.ui_context) { handled = true; }
            if self.selected_spinbox_editor.keyboard_input(event, &mut self.ui_context) { handled = true; }
            if self.selected_font_editor.keyboard_input(event, &mut self.ui_context) { handled = true; }
            if self.selected_choice_editor.keyboard_input(event, &mut self.ui_context) { handled = true; }
            if self.new_key_editor.keyboard_input(event, &mut self.ui_context) {
                handled = true;
                if event.state == ElementState::Pressed && event.logical_key == Key::Named(cce_ui::widget::NamedKey::Enter) {
                    msg_out = Some(AppMessage::AddKey);
                }
            }
        }

        if handled {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        msg_out
    }
}

fn parse_hex_color_rgba(s: &str) -> Option<[u8; 4]> {
    let s = s.trim_matches(|c| c == '"' || c == '\'' || c == ' ');
    let s = s.trim_start_matches('#');
    if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        let a = u8::from_str_radix(&s[6..8], 16).ok()?;
        Some([r, g, b, a])
    } else if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some([r, g, b, 255])
    } else {
        None
    }
}

fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    parse_hex_color_rgba(s).map(|rgba| [rgba[0], rgba[1], rgba[2]])
}

fn format_hex_color(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
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
        let content = std::fs::read_to_string("/home/lsgalante/.config/cce/config.kdl").unwrap();
        let val = cce_ui::config::parse_kdl_to_json(&content);
        let mut flat = Vec::new();
        flatten_json(&val, "", &mut flat);
        
        let mut found = false;
        for (k, v) in &mut flat {
            if k == "style.status.background_color" {
                *v = serde_json::Value::String("#151520e6".to_string());
                found = true;
            }
        }
        assert!(found, "Should find style.status.background_color!");

        let root = unflatten_json(&flat);
        let new_kdl = json_to_kdl_string(&root);
        println!("Generated KDL:\n{}", new_kdl);
        match new_kdl.parse::<kdl::KdlDocument>() {
            Ok(_) => println!("Parsed OK!"),
            Err(e) => panic!("Failed to parse generated KDL: {}", e),
        }
    }
}
