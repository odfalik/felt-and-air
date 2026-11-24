//! Actual editor layout/input tests. No native window, device, engine, copied UI,
//! rasterizer or host-value simulation. This does NOT certify native event delivery,
//! visual appearance, or a parameter reset value round-trip through a real host.

use super::*;
use std::sync::{Mutex, MutexGuard};

#[path = "compact_tests.rs"]
mod compact_tests;

#[path = "visual_text_tests.rs"]
mod visual_text_tests;

#[path = "action_export_tests.rs"]
mod action_export_tests;

// Run this crate's UI suite with --test-threads=1: unrelated plugin tests can
// also read STANDALONE. This guard additionally serializes these fixtures and
// restores the global on normal return AND panic, without production changes.
static EDITOR_MODE_LOCK: Mutex<()> = Mutex::new(());
struct EditorModeGuard { previous: bool, _lock: MutexGuard<'static, ()> }
impl EditorModeGuard {
    fn new(standalone: bool) -> Self {
        let lock = EDITOR_MODE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Self { previous: STANDALONE.swap(standalone, Ordering::Relaxed), _lock: lock }
    }
}
impl Drop for EditorModeGuard {
    fn drop(&mut self) { STANDALONE.store(self.previous, Ordering::Relaxed); }
}
fn both_editor_modes(check: impl Fn()) {
    for standalone in [false, true] {
        let _mode = EditorModeGuard::new(standalone);
        check();
    }
}

// Explicit opt-in artifact export; renderer/shaders live only in temporary scratch.
// This exports ACTUAL editor primitives, not a native screenshot or input/host proof.
#[test]
#[ignore = "requires explicit FELT_EDITOR_REPLAY_OUTPUT and FELT_EDITOR_REPLAY_REV"]
fn export_one_actual_standalone_room_frame() {
    let path = std::env::var_os("FELT_EDITOR_REPLAY_OUTPUT").expect("explicit new output file required");
    let revision = std::env::var("FELT_EDITOR_REPLAY_REV").expect("exact reviewed source revision required");
    assert!(std::path::Path::new(&path).is_absolute(), "explicit absolute scratch output required");
    assert!(revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()));
    let _mode = EditorModeGuard::new(true);
    let category = match std::env::var("FELT_EDITOR_REPLAY_CATEGORY") {
        Ok(value) => value.parse::<usize>().expect("category must be an integer0..3"),
        Err(std::env::VarError::NotPresent) => 0,
        Err(error) => panic!("invalid category: {error}"),
    };
    assert!(category < 4, "category must be0..3");
    let viewport = match std::env::var("FELT_EDITOR_REPLAY_VIEWPORT") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "1280x860".into(),
        Err(error) => panic!("invalid viewport preset: {error}"),
    };
    let logical_size = match viewport.as_str() {
        "800x600" => [800, 600],
        "1000x650" => [1000, 650],
        "1280x720" => [1280, 720],
        "1280x860" => [1280, 860],
        _ => panic!("viewport must be one of800x600/1000x650/1280x720/1280x860"),
    };
    assert!(logical_size[0] * logical_size[1] <= 1280 * 860);
    let mut f = Fixture::new(vec2(logical_size[0] as f32, logical_size[1] as f32));
    f.state.tab = category;
    f.state.active_param = PARAMS.iter().position(|p| p.tab == category).unwrap_or(0);
    f.state.frame = u64::MAX;
    let mut frames = Vec::new();
    for pass in 0..4 {
        let output=f.frame(Vec::new());
        assert_eq!(output.pixels_per_point,1.0); // Preserve ALL four static-frame assertions.
        let frame=action_export_tests::serialize_replay_frame(&f.ctx,&output.textures_delta,if pass==3 {Some(&output)} else {None});
        action_export_tests::real_gpu_roundtrip(&frame);frames.push(frame);
    }
    f.host.assert_no_edits();
    let document=action_export_tests::StaticDocument {format:"actual-egui-frame-v1",revision:&revision,mode:"Standalone",
        category_index:category,category_name:guide::TABS[category],viewport_preset:&viewport,
        logical_size,pixels_per_point:1.0,frames:&frames,
        limits:"HEADLESS ACTUAL-EDITOR FRAME REPLAY; not native screenshot/input/host reset/audio proof; live engine monitors empty"};
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).expect("new scratch export file");
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, &document).unwrap();
    std::io::Write::flush(&mut writer).expect("complete scratch export flush");
}

type Trace = (Rect, Rect, bool); // actual response rectangle, clip rectangle, enabled

#[derive(Debug)]
enum Intent { Begin(ParamPtr), Set(ParamPtr, f32), End(ParamPtr) }

struct RecordingHost {
    params: Arc<PianoParams>,
    api: PluginApi,
    intents: Mutex<Vec<Intent>>,
}
impl RecordingHost {
    fn validate(&self, ptr: ParamPtr) {
        assert!(self.params.param_map().iter().any(|(_, p, _)| *p == ptr));
    }
    fn assert_no_edits(&self) {
        let intents = self.intents.lock().unwrap();
        assert!(intents.is_empty(), "wheel/layout unexpectedly emitted host edit intents: {intents:?}");
    }
}
impl GuiContext for RecordingHost {
    fn plugin_api(&self) -> PluginApi { self.api }
    fn request_resize(&self) -> bool { true }
    unsafe fn raw_begin_set_parameter(&self, ptr: ParamPtr) {
        self.validate(ptr); self.intents.lock().unwrap().push(Intent::Begin(ptr));
    }
    unsafe fn raw_set_parameter_normalized(&self, ptr: ParamPtr, value: f32) {
        self.validate(ptr); assert!(value.is_finite() && (0.0..=1.0).contains(&value));
        self.intents.lock().unwrap().push(Intent::Set(ptr, value));
        // NIH owns value mutation. Do not pretend this recorder applies the edit.
    }
    unsafe fn raw_end_set_parameter(&self, ptr: ParamPtr) {
        self.validate(ptr); self.intents.lock().unwrap().push(Intent::End(ptr));
    }
    fn get_state(&self) -> PluginState { panic!("headless layout fixture does not emulate host state") }
    fn set_state(&self, _: PluginState) { panic!("headless layout fixture does not emulate host state") }
}

struct Fixture {
    ctx: Context,
    state: State,
    host: RecordingHost,
    size: Vec2,
    frame: u64,
    focused: bool,
    replay:Option<action_export_tests::Capture>,
}
impl Fixture {
    fn new(size: Vec2) -> Self {
        Self::from_shared(size,Arc::new(PianoParams::new()),Shared::new(),Arc::new(Vec::new()))
    }
    fn from_shared(size:Vec2,params:Arc<PianoParams>,shared:Arc<Shared>,demos:Arc<Vec<Demo>>)->Self {
        Self::from_catalog(size,params,shared,demos,BTreeMap::new())
    }
    fn catalog(size:Vec2,params:Arc<PianoParams>,demos:Arc<Vec<Demo>>,setups:BTreeMap<String,Setup>)->Self {
        Self::from_catalog(size,params,Shared::new(),demos,setups)
    }
    fn from_catalog(size:Vec2,params:Arc<PianoParams>,shared:Arc<Shared>,demos:Arc<Vec<Demo>>,setups:BTreeMap<String,Setup>)->Self {
        let ctx = Context::default(); theme(&ctx);
        let state = State::new(params.clone(), shared, demos, setups);
        Self { ctx, state, host: RecordingHost { params, api: if STANDALONE.load(Ordering::Relaxed) { PluginApi::Standalone } else { PluginApi::Clap }, intents: Mutex::new(Vec::new()) }, size, frame: 0, focused: true,replay:None }
    }
    fn frame(&mut self, events: Vec<Event>) -> FullOutput {
        // Actual widgets may be off-screen or absent after a tab change. Never
        // mistake a cached rectangle from a preceding frame for a live response.
        self.ctx.data_mut(|data| {
            data.remove::<Rect>(Id::new("editor:keyboard"));
            data.remove::<Vec<SelectorTrace>>(Id::new("selector:responses"));
            for kind in ["preset","demo"] {data.remove::<(Rect,Rect)>(Id::new(format!("selector:popup:{kind}")));}
            data.remove::<crate::editor::illustration::action::PaintTrace>(Id::new("action:paint"));
            for name in ["room-panel","strings-panel","board-panel"] {
                data.remove::<(Rect,Rect)>(Id::new(format!("left:{name}")));
            }
            data.remove::<bool>(Id::new("editor:setup-name-focused"));
            for name in ["editor:save-open", "editor:setup-name", "editor:setup-cancel",
                "editor:pedal-sustain", "editor:pedal-soft", "editor:pedal-sostenuto"] {
                data.remove::<Rect>(Id::new(name));
            }
            for name in PARAMS.iter().map(|p| p.name).chain(["Volume", "Board grid"]) {
                data.remove::<Trace>(Id::new(format!("control:{name}")));
                data.remove::<Trace>(Id::new(format!("reset:{name}")));
            }
        });
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.frame as f64 / 60.0), focused: self.focused, events, ..Default::default() };
        self.frame += 1;
        let setter = ParamSetter::new(&self.host);
        let output=self.ctx.run(input, |ctx| self.state.show(ctx, &setter));
        if let Some(replay)=&mut self.replay {replay.push(&self.ctx,&output,&self.state,self.frame-1);}
        output
    }
    fn settle(&mut self) { for _ in 0..3 { self.frame(Vec::new()); } }
    fn trace(&self, kind: &str, name: &str) -> Option<Trace> {
        self.ctx.data(|data| data.get_temp(Id::new(format!("{kind}:{name}"))))
    }
    fn sidebar(&self) -> (Rect, Vec2, Vec2) { self.state.scroll_debug.expect("actual State.show sidebar") }
    fn screen(&self) -> Rect { Rect::from_min_size(Pos2::ZERO, self.size) }
    fn visible_control(&self, kind: piano_engine::params::Kind) -> Pos2 {
        let (viewport, _, _) = self.sidebar();
        PARAMS.iter().filter(|p| p.tab == 0 && p.kind == kind).find_map(|p| {
            let (rect, clip, _) = self.trace("control", p.name)?;
            let visible = rect.intersect(clip).intersect(viewport).intersect(self.screen());
            (visible.width() > 8.0 && visible.height() > 8.0).then(|| visible.center())
        }).expect("an actual visible Room control")
    }
    fn focused_room_slider(&self) -> Option<Response> {
        // Separate unlocked API calls: never re-enter Context inside data/memory locks.
        let focused = self.ctx.memory(|memory| memory.focused())?;
        let response = self.ctx.read_response(focused)?;
        if !response.has_focus() || !response.sense.senses_drag() || response.sense.senses_click() { return None; }
        let is_actual_slider = PARAMS.iter().filter(|p| p.tab == 0 && p.kind != piano_engine::params::Kind::Switch)
            .any(|p| self.trace("control", p.name).is_some_and(|(rect, clip, enabled)| {
                enabled && rect == response.rect && rect.intersect(clip).intersect(self.screen()).height() > 8.0
            }));
        is_actual_slider.then_some(response)
    }
    fn click_editor_widget(&mut self, key: &str) {
        let rect = self.ctx.data(|data| data.get_temp::<Rect>(Id::new(key))).expect("fresh actual editor widget rectangle");
        self.click_at(rect.intersect(self.screen()).center());
        self.settle(); // real popup placement/focus must settle before targeting its child
    }
    fn click_at(&mut self, point: Pos2) {
        assert!(self.screen().contains(point));
        for pressed in [true, false] {
            self.frame(vec![Event::PointerMoved(point), Event::PointerButton {
                pos: point, button: PointerButton::Primary, pressed, modifiers: Modifiers::default(),
            }]);
        }
    }
    fn setup_name_focused(&self) -> bool {
        self.ctx.data(|data| data.get_temp::<bool>(Id::new("editor:setup-name-focused")))
            .expect("fresh actual TextEdit focus response")
    }
    fn musical_inputs(&self) -> Vec<Input> {
        std::iter::from_fn(|| self.state.shared.inputs.pop()).collect()
    }
    fn wheel(&mut self, point: Pos2, unit: MouseWheelUnit, magnitude: f32) {
        self.frame(vec![Event::PointerMoved(point), Event::MouseWheel {
            unit, delta: vec2(0.0, -magnitude), modifiers: Modifiers::default(),
        }]);
        for _ in 0..12 { self.frame(vec![Event::PointerMoved(point)]); }
    }
}

#[test]
fn actual_room_editor_wheel_respects_real_overflow_without_edit_intents() {
    both_editor_modes(check_room_editor_wheel);
}
fn check_room_editor_wheel() {
    let mut overflow_cases = 0;
    for size in [vec2(1180.0, 800.0), vec2(1280.0, 860.0), vec2(1600.0, 1000.0)] {
        for unit in [MouseWheelUnit::Point, MouseWheelUnit::Line] {
            for target in ["slider", "checkbox", "reset", "blank"] {
                let mut f = Fixture::new(size); f.settle();
                let (viewport, content, before) = f.sidebar();
                let overflows = content.y > viewport.height() + 1.0;
                if overflows { overflow_cases += 1; }
                else {
                    let last = PARAMS.iter().find(|p| p.name == "Listener position, along the strings").unwrap();
                    let (rect, clip, _) = f.trace("control", last.name).expect("actual final Room control");
                    assert!(rect.intersect(clip).intersect(viewport).intersect(f.screen()).height() > 8.0,
                        "fitting content must have its final control already visible");
                }
                if target == "slider" && matches!(unit, MouseWheelUnit::Point) {
                    println!("REAL Room api={:?} size={size:?} viewport={viewport:?} content={content:?} overflows={overflows}", f.host.api);
                }
                let point = match target {
                    "slider" => f.visible_control(piano_engine::params::Kind::Slider),
                    "checkbox" => f.visible_control(piano_engine::params::Kind::Switch),
                    "reset" => PARAMS.iter().filter(|p| p.tab == 0).find_map(|p| {
                        let (rect, clip, enabled) = f.trace("reset", p.name)?;
                        let visible = rect.intersect(clip).intersect(viewport).intersect(f.screen());
                        assert!(!enabled, "factory-value reset should be disabled");
                        (visible.width() > 8.0 && visible.height() > 8.0).then(|| visible.center())
                    }).expect("visible actual reset button"),
                    _ => PARAMS.iter().filter(|p| p.tab == 0).find_map(|p| {
                        let (rect, clip, _) = f.trace("control", p.name)?;
                        let point = pos2(viewport.center().x, rect.bottom() + 2.0);
                        (viewport.contains(point) && clip.contains(point) && f.screen().contains(point)).then_some(point)
                    }).expect("blank gap after an actual control"),
                };
                assert!(viewport.contains(point) && f.screen().contains(point));
                f.wheel(point, unit, if unit == MouseWheelUnit::Point { 180.0 } else { 4.0 });
                let (_, _, after) = f.sidebar();
                if overflows {
                    assert!(after.y > before.y, "Room wheel blocked over {target}, size={size:?}, unit={unit:?}, before={before:?}, after={after:?}");
                } else {
                    assert_eq!(before.y, 0.0); assert_eq!(after.y, 0.0, "fitting Room must not scroll");
                }
                f.host.assert_no_edits();
            }
        }
    }
    if overflow_cases == 0 {
        println!("REAL Room layout: no overflow in tested valid viewports; overflow-wheel regression UNEXERCISED (no dummy filler)");
    } else { println!("REAL Room layout: {overflow_cases} cases exercised actual overflow"); }
}

#[test]
fn actual_room_last_control_is_reachable_and_factory_resets_are_disabled() {
    both_editor_modes(check_room_last_control_and_factory_resets);
}
fn check_room_last_control_and_factory_resets() {
    for size in [vec2(1180.0, 800.0), vec2(1280.0, 860.0), vec2(1600.0, 1000.0)] {
        let mut f = Fixture::new(size); f.settle();
        let point = PARAMS.iter().filter(|p| p.tab == 0).find_map(|p| {
            let (rect, clip, enabled) = f.trace("reset", p.name)?;
            assert!(!enabled);
            let visible = rect.intersect(clip).intersect(f.sidebar().0).intersect(f.screen());
            (visible.width() > 8.0 && visible.height() > 8.0).then(|| visible.center())
        }).expect("visible factory reset");
        for pressed in [true, false] {
            f.frame(vec![Event::PointerMoved(point), Event::PointerButton { pos: point,
                button: PointerButton::Primary, pressed, modifiers: Modifiers::default() }]);
        }
        f.host.assert_no_edits(); // Clicking an actual disabled reset emits no gesture.
        for _ in 0..12 {
            let (viewport, _, _) = f.sidebar();
            f.wheel(viewport.center(), MouseWheelUnit::Point, 1200.0);
        }
        let (viewport, content, offset) = f.sidebar();
        if content.y > viewport.height() + 1.0 { assert!(offset.y > 0.0); }
        else { assert_eq!(offset.y, 0.0, "fitting Room controls must already be visible"); }
        assert!(offset.y <= (content.y - viewport.height()).max(0.0) + 1.0);
        let last = PARAMS.iter().find(|p| p.name == "Listener position, along the strings").unwrap();
        let (rect, clip, _) = f.trace("control", last.name).expect("actual final Room slider response");
        assert!(rect.intersect(clip).intersect(viewport).intersect(f.screen()).height() > 8.0, "last Room control unreachable: {}", last.name);
        for p in PARAMS.iter().filter(|p| p.tab == 0) {
            let (_, _, enabled) = f.trace("reset", p.name).expect("actual Room reset response");
            assert!(!enabled, "{} default reset unexpectedly enabled", p.name);
        }
        f.host.assert_no_edits();
    }
}

fn key_event(key: Key, pressed: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::default() }
}

#[test]
fn actual_save_name_typing_owns_text_without_musical_side_effects() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        f.click_editor_widget("editor:save-open");
        f.click_editor_widget("editor:setup-name");
        assert!(f.state.saving && f.setup_name_focused());
        assert!(f.musical_inputs().is_empty());
        let (octave, velocity) = (f.state.octave, f.state.velocity);
        for (key, text) in [(Key::A,"a"),(Key::W,"w"),(Key::Z,"z"),(Key::X,"x"),(Key::C,"c"),(Key::V,"v"),
            (Key::Space," "),(Key::Comma,","),(Key::Period,".")] {
            f.frame(vec![key_event(key, true), Event::Text(text.into())]);
            f.frame(vec![key_event(key, false)]);
            assert_eq!((f.state.octave, f.state.velocity), (octave, velocity));
        }
        assert_eq!(f.state.setup_name, "awzxcv ,."); // actual TextEdit accepted the text
        assert_eq!((f.state.octave, f.state.velocity), (octave, velocity));
        assert!(f.state.key_notes.is_empty());
        let leaked = f.musical_inputs();
        assert!(leaked.is_empty(), "typing actual Save name leaked musical commands: {leaked:?}");
        f.host.assert_no_edits();
    });
}

#[test]
fn actual_save_name_focus_still_releases_an_owned_musical_key() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        f.frame(vec![key_event(Key::A, true)]);
        let notes = f.musical_inputs();
        assert!(matches!(notes.as_slice(), [Input::NoteOn { midi:60, from:FROM_UI, .. }]));
        f.click_editor_widget("editor:save-open"); f.click_editor_widget("editor:setup-name");
        assert!(f.setup_name_focused());
        f.frame(vec![key_event(Key::A, false)]);
        let notes = f.musical_inputs();
        assert!(matches!(notes.as_slice(), [Input::NoteOff { midi:60, from:FROM_UI, .. }]));
        assert!(f.state.key_notes.is_empty());
        f.frame(vec![key_event(Key::A, false)]); assert!(f.musical_inputs().is_empty());
        f.host.assert_no_edits();
    });
}

#[test]
fn actual_text_focus_releases_owned_pedals_once_not_unowned_typing() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, true)]); }
        let commands = f.musical_inputs();
        assert!(matches!(commands.as_slice(), [Input::Sustain(1.0,FROM_UI), Input::Soft(true,FROM_UI), Input::Sostenuto(true,FROM_UI)]));
        f.click_editor_widget("editor:save-open"); f.click_editor_widget("editor:setup-name");
        assert!(f.setup_name_focused());
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, false)]); }
        let commands = f.musical_inputs();
        assert!(matches!(commands.as_slice(), [Input::Sustain(0.0,FROM_UI), Input::Soft(false,FROM_UI), Input::Sostenuto(false,FROM_UI)]));
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, false)]); }
        let duplicates = f.musical_inputs();
        assert!(duplicates.is_empty(), "unowned repeated keyup emitted pedal commands: {duplicates:?}");
    });
}

#[test]
fn actual_focus_loss_releases_keyboard_owned_pedals_once() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, true)]); }
        assert_eq!(f.musical_inputs().len(), 3);
        f.focused = false; f.frame(Vec::new()); // actual State.show focus-loss cleanup; not native delivery proof
        assert!(!f.ctx.input(|input| input.focused), "focus-loss control did not deliver focused=false to actual egui input");
        let released = f.musical_inputs();
        assert!(matches!(released.as_slice(), [Input::Sustain(0.0,FROM_UI), Input::Soft(false,FROM_UI), Input::Sostenuto(false,FROM_UI)]),
            "focus-loss missing keyboard-owned pedal releases: {released:?}");
        f.frame(Vec::new()); assert!(f.musical_inputs().is_empty());
        f.focused = true;
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, false)]); }
        assert!(f.musical_inputs().is_empty(), "later keyup must not duplicate focus-loss release");
    });
}

#[test]
fn actual_editor_drop_releases_keyboard_owned_pedals_once() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, true)]); }
        assert_eq!(f.musical_inputs().len(), 3);
        let shared = f.state.shared.clone();
        drop(f); // actual State::drop -> release_ui, no audio/engine required
        let released: Vec<_> = std::iter::from_fn(|| shared.inputs.pop()).collect();
        assert!(matches!(released.as_slice(), [Input::Sustain(0.0,FROM_UI), Input::Soft(false,FROM_UI), Input::Sostenuto(false,FROM_UI)]),
            "editor Drop missing keyboard-owned pedal releases: {released:?}");
    });
}

#[test]
fn actual_button_only_pedal_commands_survive_unowned_keyups_and_cleanup() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        for widget in ["editor:pedal-sustain", "editor:pedal-soft", "editor:pedal-sostenuto"] { f.click_editor_widget(widget); }
        let commands = f.musical_inputs();
        assert!(matches!(commands.as_slice(), [Input::Sustain(1.0,FROM_UI), Input::Soft(true,FROM_UI), Input::Sostenuto(true,FROM_UI)]));
        // Buttons emit their real UI commands; no fake applied pedal/audio state.
        for key in [Key::Space, Key::Comma, Key::Period] { f.frame(vec![key_event(key, false)]); }
        assert!(f.musical_inputs().is_empty(), "unowned keyups must not cancel button-only commands");
        f.focused = false; f.frame(Vec::new()); assert!(f.musical_inputs().is_empty());
        let shared = f.state.shared.clone(); drop(f);
        assert!(shared.inputs.pop().is_none(), "drop must not cancel button-only pedal commands");
    });
}

#[test]
fn actual_slider_focus_does_not_indefinitely_disable_musical_typing() {
    both_editor_modes(|| {
        let mut f = Fixture::new(vec2(1280.0, 860.0)); f.settle();
        f.click_editor_widget("editor:save-open"); f.click_editor_widget("editor:setup-name");
        f.click_editor_widget("editor:setup-cancel"); assert!(!f.state.saving);
        let point = f.visible_control(piano_engine::params::Kind::Slider);
        f.click_at(point);
        assert!(f.musical_inputs().is_empty());
        // Mouse clicking this drag-only Slider need not take keyboard focus.
        // Preserve the actual click -> musical typing contract independently.
        f.host.intents.lock().unwrap().clear();
        f.frame(vec![key_event(Key::A, true)]);
        assert!(matches!(f.musical_inputs().as_slice(), [Input::NoteOn { midi:60, from:FROM_UI, .. }]));
        f.frame(vec![key_event(Key::A, false)]);
        assert!(matches!(f.musical_inputs().as_slice(), [Input::NoteOff { midi:60, from:FROM_UI, .. }]));
        // Establish REAL focus through egui's user-facing Tab navigation. No
        // request_focus(), fake Context state, new widget or synthetic focus hook.
        for _ in 0..64 {
            if f.focused_room_slider().is_some() { break; }
            f.frame(vec![key_event(Key::Tab, true)]);
            f.frame(vec![key_event(Key::Tab, false)]);
        }
        let response = f.focused_room_slider().expect("Tab navigation did not focus an actual visible Room Slider");
        assert!(response.has_focus() && f.ctx.wants_keyboard_input(), "real slider focus must exercise the blanket-focus-guard regression");
        f.host.intents.lock().unwrap().clear();
        f.frame(vec![key_event(Key::ArrowRight, true)]);
        f.frame(vec![key_event(Key::ArrowRight, false)]);
        assert!(f.host.intents.lock().unwrap().iter().any(|intent| matches!(intent, Intent::Set(_, _))),
            "keyboard-focused actual Slider did not emit a real parameter-edit intent");
        assert!(f.musical_inputs().is_empty());
        assert!(f.focused_room_slider().is_some() && f.ctx.wants_keyboard_input());
        // Recorder observes genuine host intents, never fakes NIH value mutation.
        f.host.intents.lock().unwrap().clear();
        f.frame(vec![key_event(Key::A, true)]);
        assert!(matches!(f.musical_inputs().as_slice(), [Input::NoteOn { midi:60, from:FROM_UI, .. }]));
        f.frame(vec![key_event(Key::A, false)]);
        assert!(matches!(f.musical_inputs().as_slice(), [Input::NoteOff { midi:60, from:FROM_UI, .. }]));
    });
}

#[test]
fn actual_editor_keyboard_rect_is_category_invariant() {
    both_editor_modes(|| {
        for size in [vec2(1180.0, 800.0), vec2(1280.0, 860.0), vec2(1600.0, 1000.0)] {
            let mut f = Fixture::new(size);
            let mut expected = None;
            for tab in 0..guide::TABS.len() {
                f.state.tab = tab;
                f.state.active_param = PARAMS.iter().position(|p| p.tab == tab).unwrap_or(0);
                f.state.frame = u64::MAX;
                f.settle();
                let rect = f.ctx.data(|data| data.get_temp::<Rect>(Id::new("editor:keyboard")))
                    .expect("fresh actual State.keyboard rectangle in this frame");
                assert!(rect.is_finite() && f.screen().contains_rect(rect));
                assert_eq!(rect.height(), 90.0);
                println!("ACTUAL keyboard api={:?} size={size:?} category={} rect={rect:?}", f.host.api, guide::TABS[tab]);
                if let Some(reference) = expected {
                    assert_eq!(rect, reference, "actual keyboard moved forcategory{} api={:?} size={size:?}", guide::TABS[tab], f.host.api);
                } else { expected = Some(rect); }
            }
            f.host.assert_no_edits();
        }
    });
}

#[test]
fn actual_editor_category_tessellation_and_clip_rects_are_finite() {
    both_editor_modes(check_category_tessellation);
}
fn check_category_tessellation() {
    // The clipped small viewport is diagnostic layout coverage, not a claim that
    // a native host accepts a window smaller than the editor's resize minimum.
    for size in [vec2(1000.0, 700.0), vec2(1180.0, 800.0), vec2(1280.0, 860.0)] {
        let mut f = Fixture::new(size);
        for tab in 0..guide::TABS.len() {
            f.state.tab = tab;
            f.state.active_param = PARAMS.iter().position(|p| p.tab == tab).unwrap_or(0);
            f.state.frame = u64::MAX;
            f.settle();
            let out = f.frame(Vec::new());
            assert!(!out.shapes.is_empty());
            let primitives = f.ctx.tessellate(out.shapes, out.pixels_per_point);
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            let mut visible_meshes = 0;
            for primitive in primitives {
                let clip = primitive.clip_rect;
                assert!(clip.min.is_finite() && clip.max.is_finite());
                match primitive.primitive {
                    epaint::Primitive::Mesh(mesh) => {
                        assert!(mesh.vertices.iter().all(|v| v.pos.is_finite() && v.uv.is_finite()));
                        assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.vertices.len()));
                        // Geometry only, not pixel visibility/occlusion or visual clipping proof.
                        let observed = mesh.calc_bounds().intersect(clip).intersect(screen);
                        if observed.width() > 0.0 && observed.height() > 0.0 && !mesh.indices.is_empty() { visible_meshes += 1; }
                    }
                    epaint::Primitive::Callback(_) => panic!("headless structural gate cannot inspect native paint callbacks"),
                }
            }
            assert!(visible_meshes > 0);
            f.host.assert_no_edits();
        }
    }
}
