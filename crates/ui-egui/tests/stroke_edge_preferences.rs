//! Stroke-edge snapping's two UI controls share one persisted, off-by-default preference.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Mutex, MutexGuard};

use craft::ui::runtime::frame::take_output;
use craft::ui::shell::embed;
use egui::accesskit::{Node, Role};
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use serde_json::{Value, json};
use vectorcraft_engine::{Prefs, Session};
use vectorcraft_ui_egui::{UiState, VectorcraftApp, control, i18n, prefs_dialog};

const KEY: &str = "snapToStrokeEdges";
const LABEL: &str = "Snap to Stroke Edges";
static UI_STATE: Mutex<()> = Mutex::new(());

struct Language {
    previous: i18n::Lang,
    _lock: MutexGuard<'static, ()>,
}

impl Language {
    fn english() -> Self {
        let lock = UI_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let previous = i18n::current();
        i18n::set_current(i18n::Lang::EN);
        Self { previous, _lock: lock }
    }
}

impl Drop for Language {
    fn drop(&mut self) {
        i18n::set_current(self.previous);
    }
}

struct Scene {
    app: VectorcraftApp,
    ctx: egui::Context,
    time: f64,
    nodes: Vec<Node>,
}

impl Scene {
    fn new() -> Self {
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.run("app.language", json!({"lang": "en"})).unwrap();
        app.run("file.new", json!({"width": 500, "height": 350})).unwrap();
        let id = app.run("shape.rectangle", json!({"x": 70, "y": 60, "width": 120, "height": 80})).unwrap()["id"].clone();
        app.run("select.set", json!({"ids": [id]})).unwrap();
        app.select_tool("selection");
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        embed::install(&ctx, craft::ui::testing::install_spec(), &vectorcraft_ui_egui::ui_fonts()).unwrap();
        app.adopt_context(&ctx);
        let mut s = Self { app, ctx, time: 0.0, nodes: vec![] };
        s.settle();
        s.app.take_prefs_changed();
        s
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 30.0;
        let mut raw = RawInput {
            events,
            time: Some(self.time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 1000.0))),
            focused: true,
            ..Default::default()
        };
        self.app.raw_input(&self.ctx, &mut raw);
        embed::before_input(&self.ctx, |out| self.app.frame_input(out));
        let mut output = self.ctx.run_ui(raw, |ui| {
            self.app.tick(ui.ctx());
            self.app.ui(ui);
        });
        let _ = self.app.frame_input(take_output(&self.ctx));
        self.nodes =
            output.platform_output.accesskit_update.take().map(|update| update.nodes.into_iter().map(|(_, node)| node).collect()).unwrap_or_default();
        output.textures_delta.clear();
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            self.frame(vec![]);
        }
    }

    fn rect(node: &Node) -> Option<Rect> {
        let bounds = node.bounds()?;
        Some(Rect::from_min_max(egui::pos2(bounds.x0 as f32, bounds.y0 as f32), egui::pos2(bounds.x1 as f32, bounds.y1 as f32)))
    }

    fn button(&self, label: &str) -> Pos2 {
        let nodes: Vec<_> = self.nodes.iter().filter(|n| n.role() == Role::Button && n.label() == Some(label)).filter_map(Self::rect).collect();
        assert_eq!(nodes.len(), 1, "one actual native button {label:?}");
        nodes[0].center()
    }

    fn dialog(&self) -> Value {
        control::inspect(&self.app, &self.ctx)["ui"]["dialog"].clone()
    }

    fn draft(&self) -> bool {
        self.dialog()["fields"][KEY].as_bool().expect("the existing Preferences draft field")
    }

    fn toggle(&self, role: Role) -> Option<&Node> {
        self.nodes.iter().find(|n| n.role() == role && n.label() == Some(LABEL)
            && Self::rect(n).is_some_and(|r| r.is_positive() && self.ctx.content_rect().contains(r.center())))
    }

    fn menu_open(&self) -> bool {
        self.toggle(Role::MenuItemCheckBox).is_some()
    }

    fn click(&mut self, pos: Pos2) {
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE), Event::PointerMoved(pos)]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.settle();
    }

    fn value(&mut self) -> bool {
        self.app.run("prefs.get", json!({"key": KEY})).unwrap().as_bool().unwrap()
    }

    fn open_preferences(&mut self) {
        self.app.run("edit.preferences", json!({"category": "Smart Guides"})).unwrap();
        self.settle();
        let dialog = self.dialog();
        assert_eq!(dialog["kind"], "preferences");
        assert_eq!(dialog["fields"]["__category"], "Smart Guides");
        assert!(self.nodes.iter().any(|node| matches!(node.role(), Role::Dialog | Role::Window) && node.label() == Some("Preferences")),
            "the native Preferences window is drawn");
        // Scroll the actual settings column if Geometric Guides is below its viewport.
        if self.toggle(Role::CheckBox).is_none() {
            let in_fields = self.nodes.iter().find(|n| n.role() == Role::CheckBox && n.label() == Some("Alignment Guides"))
                .and_then(Self::rect).expect("the actual Alignment Guides control").center();
            for _ in 0..8 {
                self.frame(vec![Event::PointerMoved(in_fields), Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -120.0), phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown, modifiers: Modifiers::NONE,
                }]);
                for _ in 0..10 { self.frame(vec![]); }
                if self.toggle(Role::CheckBox).is_some() { break; }
            }
        }
        self.checkbox_point();
    }

    fn checkbox_point(&self) -> Pos2 {
        let node = self.toggle(Role::CheckBox).expect("the visible native Preferences checkbox");
        assert!(!node.is_disabled());
        Self::rect(node).unwrap().center()
    }

    fn menu_point(&self) -> Pos2 {
        assert!(self.menu_open(), "the native Snapping menu is open");
        let node = self.toggle(Role::MenuItemCheckBox).expect("the actual native checked menu row");
        assert!(!node.is_disabled());
        Self::rect(node).unwrap().center()
    }

    fn assert_toggle_value(&self, role: Role, expected: bool) {
        let node = self.toggle(role).expect("the real toggle response is visible");
        let expected = if expected { egui::accesskit::Toggled::True } else { egui::accesskit::Toggled::False };
        assert_eq!(node.toggled(), Some(expected), "the actual native control exposes its displayed value");
    }

    fn assert_checkbox_value(&self, expected: bool) {
        self.checkbox_point();
        self.assert_toggle_value(Role::CheckBox, expected);
    }

    fn assert_menu_value(&self, expected: bool) {
        self.menu_point();
        self.assert_toggle_value(Role::MenuItemCheckBox, expected);
    }
}

#[test]
fn stroke_edge_preferences_generic_commands_and_saved_ui_restore_one_false_default() {
    let _language = Language::english();
    let mut s = Scene::new();
    assert!(!s.value());
    let mut legacy = Prefs::default().to_json();
    legacy.as_object_mut().unwrap().remove(KEY);
    let loaded: Prefs = serde_json::from_value(legacy).unwrap();
    assert_eq!(loaded.to_json()[KEY], json!(false), "old preference JSON keeps the opt-in off");
    let state = s.app.session.active().unwrap();
    let (document, selection, history) = (state.doc.clone(), state.selection.clone(), (state.history.undo.len(), state.history.redo.len()));
    let listed = s.app.run("prefs.list", json!({})).unwrap();
    let entries: Vec<_> = listed.as_array().unwrap().iter().filter(|entry| entry["key"] == KEY).collect();
    assert_eq!(entries.len(), 1);
    let entry = entries[0];
    assert_eq!(entry["category"], "Smart Guides");
    assert_eq!(entry["section"], "Geometric Guides");
    assert_eq!(entry["label"], LABEL);
    assert_eq!(entry["kind"], "bool");
    assert_eq!(entry["value"], false);
    s.app.run("prefs.set", json!({"key": "keyboardIncrement", "value": 3})).unwrap();
    assert_eq!(s.app.run("prefs.set", json!({"key": KEY, "value": true})).unwrap()[KEY], true);
    assert!(s.value());
    assert!(s.app.run("prefs.set", json!({"key": KEY, "value": "not a bool"})).is_err());
    assert!(s.value());
    // Use the same snapshot/UiState/restore path as the host; never touch user preference files.
    prefs_dialog::snapshot(&mut s.app);
    let bytes = serde_json::to_vec(&s.app.ui).unwrap();
    let mut reopened = VectorcraftApp::new(Session::new(), Default::default());
    reopened.ui = serde_json::from_slice::<UiState>(&bytes).unwrap().sanitized();
    prefs_dialog::restore(&mut reopened);
    assert_eq!(reopened.run("prefs.get", json!({"key": KEY})).unwrap(), true);
    reopened.run("prefs.reset", json!({"category": "Smart Guides"})).unwrap();
    assert_eq!(reopened.run("prefs.get", json!({"key": KEY})).unwrap(), false);
    assert_eq!(reopened.run("prefs.get", json!({"key": "keyboardIncrement"})).unwrap(), 3.0);
    prefs_dialog::snapshot(&mut reopened);
    assert_eq!(reopened.ui.engine_prefs[KEY], false);
    let mut legacy_ui = reopened.ui.clone();
    legacy_ui.engine_prefs.as_object_mut().unwrap().remove(KEY);
    let bytes = serde_json::to_vec(&legacy_ui).unwrap();
    let mut reset_reopened = VectorcraftApp::new(Session::new(), Default::default());
    reset_reopened.ui = serde_json::from_slice::<UiState>(&bytes).unwrap().sanitized();
    prefs_dialog::restore(&mut reset_reopened);
    assert_eq!(
        reset_reopened.run("prefs.get", json!({"key": KEY})).unwrap(),
        false,
        "legacy saved UiState without this key restores the false default"
    );
    let state = s.app.session.active().unwrap();
    assert_eq!(state.doc, document);
    assert_eq!(state.selection, selection);
    assert_eq!((state.history.undo.len(), state.history.redo.len()), history);
}

#[test]
fn stroke_edge_preferences_painted_smart_guides_checkbox_is_draft_until_ok() {
    let _language = Language::english();
    let mut s = Scene::new();
    let state = s.app.session.active().unwrap();
    let (document, selection, history) = (state.doc.clone(), state.selection.clone(), (state.history.undo.len(), state.history.redo.len()));
    s.open_preferences();
    assert!(!s.draft());
    s.assert_checkbox_value(false);
    s.click(s.checkbox_point());
    assert!(s.draft(), "actual checkbox changes the dialog draft");
    assert!(!s.value(), "draft is not a live tool preference");
    assert!(!s.app.take_prefs_changed());
    s.click(s.button("Cancel"));
    assert!(s.dialog().is_null());
    assert!(!s.value());
    assert!(!s.app.take_prefs_changed());
    s.open_preferences();
    assert!(!s.draft(), "Cancel discarded the changed draft");
    s.click(s.checkbox_point());
    s.click(s.button("OK"));
    assert!(s.dialog().is_null());
    assert!(s.value(), "OK applies the same engine preference");
    assert!(s.app.take_prefs_changed());
    s.open_preferences();
    assert!(s.draft(), "reopening reads the applied value");
    s.assert_checkbox_value(true);
    s.click(s.button("Cancel"));
    let state = s.app.session.active().unwrap();
    assert_eq!(state.doc, document);
    assert_eq!(state.selection, selection);
    assert_eq!((state.history.undo.len(), state.history.redo.len()), history);
}

#[test]
fn stroke_edge_preferences_control_bar_snapping_click_and_preferences_share_the_value() {
    let _language = Language::english();
    let mut s = Scene::new();
    s.app.run("window.control", json!({})).unwrap();
    assert!(s.app.ui.control_bar, "Window > Control shows the normally hidden bar");
    s.settle();
    let state = s.app.session.active().unwrap();
    let (document, selection, history) = (state.doc.clone(), state.selection.clone(), (state.history.undo.len(), state.history.redo.len()));
    assert!(!s.value());
    s.click(s.button("Snapping"));
    assert!(s.dialog().is_null(), "this checkbox is in the real Control bar popup");
    s.assert_menu_value(false);
    s.click(s.menu_point());
    assert!(s.value(), "actual popup checkbox applies the shared preference immediately");
    // The host's debounced save snapshots Session prefs; this menu route need not set the
    // immediate Preferences-confirmation flag. Exercise the existing public equivalent.
    prefs_dialog::snapshot(&mut s.app);
    let bytes = serde_json::to_vec(&s.app.ui).unwrap();
    let mut reopened = VectorcraftApp::new(Session::new(), Default::default());
    reopened.ui = serde_json::from_slice::<UiState>(&bytes).unwrap().sanitized();
    prefs_dialog::restore(&mut reopened);
    assert_eq!(reopened.run("prefs.get", json!({"key": KEY})).unwrap(), true, "the actual popup change survives the host snapshot/restore path");
    // Dismiss the menu normally before opening the other UI surface.
    if s.menu_open() {
        s.click(s.button("Snapping"));
    }
    s.open_preferences();
    assert!(s.draft());
    s.assert_checkbox_value(true);
    s.click(s.checkbox_point());
    assert!(s.value(), "the reverse change stays a draft until OK");
    s.click(s.button("OK"));
    assert!(!s.value());
    assert!(s.app.take_prefs_changed());
    s.click(s.button("Snapping"));
    s.assert_menu_value(false);
    assert!(!s.value(), "reopened Snapping popup reads the value applied by Preferences");
    let state = s.app.session.active().unwrap();
    assert_eq!(state.doc, document);
    assert_eq!(state.selection, selection);
    assert_eq!((state.history.undo.len(), state.history.redo.len()), history);
}
