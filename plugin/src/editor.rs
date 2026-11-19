//! One editor for the host and the standalone. Geometry is cached per rebuild, heat map
//! uploads happen at the snapshot rate, painting at 60 Hz. All controls use host gestures.
use crate::{demo::Demo, presets, shared::*, PianoParams, Setting};
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui::*, resizable_window::ResizableWindow};
use piano_engine::{
    params::{PARAMS, TAB_NAMES},
    GrandOptions, Snapshot, Vec2 as Point,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

const AMBER: Color32 = Color32::from_rgb(224, 174, 103);
const MUTED: Color32 = Color32::from_rgb(141, 148, 151);
const BG: Color32 = Color32::from_rgb(18, 22, 25);
const PANEL: Color32 = Color32::from_rgb(25, 30, 34);

#[derive(Serialize, Deserialize, Clone)]
struct Setup {
    values: Vec<f64>,
    grid: i32,
    volume: f32,
}
struct State {
    params: Arc<PianoParams>,
    shared: Arc<Shared>,
    demos: Arc<Vec<Demo>>,
    tab: usize,
    preset: String,
    demo: usize,
    octave: i32,
    velocity: i32,
    mouse_note: Option<i32>,
    key_notes: BTreeMap<Key, i32>,
    view: Option<Arc<BoardView>>,
    snap: Snapshot,
    frame: u64,
    texture: Option<TextureHandle>,
    board_scale: f32,
    string_scale: f32,
    setups: BTreeMap<String, Setup>,
    setup_name: String,
    saving: bool,
    message: String,
    ui_cost: f32,
}

pub fn create(
    params: Arc<PianoParams>,
    shared: Arc<Shared>,
    demos: Arc<Vec<Demo>>,
) -> Option<Box<dyn Editor>> {
    let state = State {
        params: params.clone(),
        shared,
        demos,
        tab: 0,
        preset: "Recital room · listener by the lid".into(),
        demo: 0,
        octave: 4,
        velocity: 80,
        mouse_note: None,
        key_notes: BTreeMap::new(),
        view: None,
        snap: Snapshot::default(),
        frame: u64::MAX,
        texture: None,
        board_scale: 2e-8,
        string_scale: 1e-4,
        setups: setup_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|s| serde_json::from_slice(&s).ok())
            .unwrap_or_default(),
        setup_name: String::new(),
        saving: false,
        message: String::new(),
        ui_cost: 0.0,
    };
    create_egui_editor(
        params.editor_state.clone(),
        state,
        |ctx, _| theme(ctx),
        |ctx, setter, s| s.show(ctx, setter),
    )
}
fn theme(ctx: &Context) {
    let mut style = Style::default();
    style.visuals = Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = Color32::from_rgb(12, 16, 19);
    style.visuals.selection.bg_fill = AMBER;
    style.visuals.selection.stroke = Stroke::new(1.0, BG);
    style.visuals.widgets.active.bg_fill = AMBER;
    style.visuals.widgets.active.fg_stroke.color = BG;
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(37, 43, 47);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(59, 63, 65);
    style.spacing.item_spacing = vec2(10.0, 9.0);
    style.spacing.slider_width = 160.0;
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(23.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.0));
    ctx.set_style(style);
}
fn setup_path() -> Option<PathBuf> {
    crate::paths::support_dir().map(|dir| dir.join("setups.json"))
}
fn set_number(setter: &ParamSetter, p: &FloatParam, v: f32) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}
fn set_switch(setter: &ParamSetter, p: &BoolParam, v: bool) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}
fn caption(ui: &mut Ui, text: impl Into<String>) {
    ui.label(
        RichText::new(text)
            .size(10.0)
            .color(MUTED)
            .extra_letter_spacing(1.5),
    );
}
fn black(m: i32) -> bool {
    matches!(m % 12, 1 | 3 | 6 | 8 | 10)
}
fn note_name(m: i32) -> String {
    if m < 0 {
        return "Play a note".into();
    }
    format!(
        "{}{}",
        ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"][(m % 12) as usize],
        m / 12 - 1
    )
}
fn value_label(format: &str, v: f64) -> String {
    let n = if format.contains(".0") {
        format!("{v:.0}")
    } else if format.contains(".1") {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    let fmt = format.replace("%%", "%");
    if let Some(start) = fmt.find('%') {
        let end = fmt[start..]
            .find('f')
            .map(|i| start + i + 1)
            .unwrap_or(start + 1);
        format!("{}{}{}", &fmt[..start], n, &fmt[end..])
    } else {
        n
    }
}
impl State {
    fn refresh(&mut self, ctx: &Context) {
        let frame = self.shared.display_frames.load(Ordering::Acquire);
        if frame == self.frame {
            return;
        }
        if let Ok(d) = self.shared.display.try_lock() {
            self.view = d.view.clone();
            self.snap.clone_from(&d.snap);
            self.frame = frame;
        } else {
            return;
        }
        let Some(v) = &self.view else { return };
        let peak = self.snap.board.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        self.board_scale = peak.max(self.board_scale * 0.97).max(2e-8);
        let inv = 1.0 / self.board_scale;
        let colors = v
            .pixels
            .iter()
            .map(|(indices, weights)| {
                if weights.iter().sum::<f32>() < 0.2 {
                    return Color32::TRANSPARENT;
                }
                let x = (0..4)
                    .map(|k| {
                        self.snap
                            .board
                            .get(indices[k] as usize)
                            .copied()
                            .unwrap_or(0.0)
                            * weights[k]
                    })
                    .sum::<f32>()
                    * inv;
                let a = x.abs().min(1.0).sqrt();
                if x >= 0.0 {
                    Color32::from_rgb(
                        (36.0 + 188.0 * a) as u8,
                        (43.0 + 131.0 * a) as u8,
                        (48.0 + 55.0 * a) as u8,
                    )
                } else {
                    Color32::from_rgb(
                        (36.0 + 45.0 * a) as u8,
                        (43.0 + 111.0 * a) as u8,
                        (48.0 + 160.0 * a) as u8,
                    )
                }
            })
            .collect();
        let image = ColorImage {
            size: [v.width, v.height],
            pixels: colors,
        };
        if let Some(t) = &mut self.texture {
            t.set(image, TextureOptions::LINEAR);
        } else {
            self.texture = Some(ctx.load_texture("board", image, TextureOptions::LINEAR));
        }
    }
    fn show(&mut self, ctx: &Context, setter: &ParamSetter) {
        let start = Instant::now();
        self.refresh(ctx);
        self.computer_keys(ctx);
        // Losing focus releases only this editor's notes, never another source's keys.
        if !ctx.input(|i| i.focused) {
            self.release_ui();
        }
        ResizableWindow::new("piano").min_size(vec2(1180.0,800.0)).show(ctx,&self.params.editor_state.clone(),|ui| {
          Frame::new().inner_margin(14.0).show(ui, |ui| {
            ui.add_space(6.0); self.header(ui,setter); ui.add_space(5.0); ui.separator();
            let h=(ui.available_height()-185.0).max(400.0);
            let width = ui.available_width();
            ui.allocate_ui_with_layout(vec2(width,h), Layout::left_to_right(Align::Min), |ui| {
                let right=410.0; let left=(width-right-20.0).max(400.0);
                ui.allocate_ui_with_layout(vec2(left,h), Layout::top_down(Align::Min),|ui| {
                    ui.add_space(8.0); ui.horizontal(|ui| { caption(ui,"SOUNDBOARD"); ui.with_layout(Layout::right_to_left(Align::Center),|ui| { ui.label(RichText::new("DISPLACEMENT · LIVE").small().color(MUTED)); }); });
                    self.board(ui,vec2(left,h-100.0));
                    ui.horizontal(|ui| { ui.label(RichText::new("−").color(Color32::LIGHT_BLUE)); ui.label(RichText::new(format!("± {:.2} μm",self.board_scale*1e6)).monospace().small().color(MUTED)); ui.label(RichText::new("+").color(AMBER));
                        if let Some(v)=&self.view { ui.with_layout(Layout::right_to_left(Align::Center),|ui| { ui.label(RichText::new(format!("{} nodes · {:.0} × {:.0} mm",v.nodes,v.spacing.x*1000.0,v.spacing.y*1000.0)).small().color(MUTED)); }); } });
                    self.parts(ui,setter);
                });
                ui.allocate_ui_with_layout(vec2(right,h), Layout::top_down(Align::Min),|ui| {
                    ui.set_max_width(right);
                    self.strings(ui,vec2(right,103.0)); ui.add_space(5.0);
                    ui.horizontal(|ui| { for (i,n) in TAB_NAMES.iter().enumerate() { if ui.selectable_label(self.tab==i,*n).clicked() { self.tab=i; } } }); ui.separator();
                    // Solid bars stay visible without hover, but only when content overflows.
                    ui.style_mut().spacing.scroll = style::ScrollStyle::solid();
                    ScrollArea::vertical().scroll_bar_visibility(scroll_area::ScrollBarVisibility::VisibleWhenNeeded).id_salt("controls").max_height(h-175.0).show(ui,|ui| {
                        if self.tab==0 { self.room(ui,setter); }
                        self.controls(ui,setter);
                    });
                });
            });
            ui.separator(); self.keyboard(ui);
            ui.horizontal(|ui| { ui.label(RichText::new(format!("A W S E D F T G Y H U J K O L P ;     Z/X octave {}     C/V velocity {}",self.octave,self.velocity)).small().color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center),|ui| { ui.label(RichText::new(format!("UI {:.2} ms · {:.0} Hz",self.ui_cost,self.shared.sample_rate.load())).small().color(MUTED)); }); });
          });
        });
        if self.saving {
            Window::new("Save setup")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Give this instrument a name");
                    ui.text_edit_singleline(&mut self.setup_name);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.setup_name.trim().is_empty(), Button::new("Save"))
                            .clicked()
                        {
                            self.setups
                                .insert(self.setup_name.trim().into(), self.capture());
                            self.message = match self.save_setups() {
                                Ok(()) => "Setup saved".into(),
                                Err(e) => format!("Could not save: {e}"),
                            };
                            self.saving = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.saving = false;
                        }
                    });
                });
        }
        self.ui_cost = self.ui_cost * 0.95 + start.elapsed().as_secs_f32() * 1000.0 * 0.05;
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    fn header(&mut self, ui: &mut Ui, setter: &ParamSetter) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(RichText::new("Felt & Air").color(Color32::from_rgb(232, 231, 221)));
                caption(ui, "A MECHANICAL INSTRUMENT");
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ComboBox::from_id_salt("preset")
                    .width(235.0)
                    .selected_text(&self.preset)
                    .show_ui(ui, |ui| {
                        for (i, n) in presets::NAMES.iter().enumerate() {
                            if ui.selectable_label(self.preset == *n, *n).clicked() {
                                let mut o = self.params.options();
                                presets::apply(i, &mut o);
                                self.apply_options(setter, &o);
                                self.preset = (*n).into();
                            }
                        }
                        ui.separator();
                        for (name, setup) in self.setups.clone() {
                            if ui.button(format!("Saved · {name}")).clicked() {
                                self.apply_setup(setter, &setup);
                                self.preset = name;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.small_button("Save setup…").clicked() {
                        self.saving = true;
                    }
                    if !self.message.is_empty() {
                        ui.label(RichText::new(&self.message).small().color(MUTED));
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                ComboBox::from_id_salt("demo")
                    .width(240.0)
                    .selected_text(
                        self.demos
                            .get(self.demo)
                            .map_or("No demos installed", |d| d.title.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for (i, d) in self.demos.iter().enumerate() {
                            ui.selectable_value(&mut self.demo, i, &d.title)
                                .on_hover_text(&d.note);
                        }
                    });
                ui.horizontal(|ui| {
                    let playing = self.shared.demo_playing.load(Ordering::Relaxed);
                    if ui
                        .add_enabled(
                            !self.demos.is_empty(),
                            Button::new(if playing < 0 { "▶ Play" } else { "■ Stop" }),
                        )
                        .clicked()
                    {
                        self.shared.send(if playing < 0 {
                            Input::PlayDemo(self.demo)
                        } else {
                            Input::StopDemo
                        });
                    }
                    let seconds = self.shared.demo_seconds.load();
                    ui.label(
                        RichText::new(format!(
                            "{}:{:02}",
                            seconds as u32 / 60,
                            seconds as u32 % 60
                        ))
                        .monospace()
                        .small(),
                    );
                    if playing >= 0 {
                        if let Some(d) = self.demos.get(playing as usize) {
                            ui.add(
                                ProgressBar::new(
                                    (seconds / d.seconds.max(0.001) as f32).clamp(0.0, 1.0),
                                )
                                .desired_width(140.0),
                            );
                        }
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                let mut vol = self.params.volume.value();
                if ui
                    .add(
                        Slider::new(&mut vol, 0.0..=3.0)
                            .text("Volume")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    set_number(setter, &self.params.volume, vol);
                }
                ui.horizontal(|ui| {
                    caption(ui, "GRID");
                    ComboBox::from_id_salt("grid")
                        .width(70.0)
                        .selected_text(
                            ["1.0×", "1.25×", "1.5×", "2.0×"][self.params.grid.value() as usize],
                        )
                        .show_ui(ui, |ui| {
                            for (i, n) in ["1.0×", "1.25×", "1.5×", "2.0×"].iter().enumerate() {
                                if ui
                                    .selectable_label(self.params.grid.value() == i as i32, *n)
                                    .clicked()
                                {
                                    setter.begin_set_parameter(&self.params.grid);
                                    setter.set_parameter(&self.params.grid, i as i32);
                                    setter.end_set_parameter(&self.params.grid);
                                }
                            }
                        });
                });
            });
            ui.vertical(|ui| {
                if STANDALONE.load(Ordering::Relaxed) {
                    let mut sound = self.shared.sound_on.load(Ordering::Relaxed);
                    let label = if sound { "Sound on" } else { "Sound off" };
                    if ui.toggle_value(&mut sound, label).changed() {
                        self.shared.sound_on.store(sound, Ordering::Relaxed);
                    }
                }
                let load = self.shared.load.load();
                ui.label(
                    RichText::new(format!(
                        "{:3.0}%  ·  {} awake",
                        load * 100.0,
                        self.snap.awake
                    ))
                    .monospace()
                    .color(if load > 1.0 {
                        Color32::LIGHT_RED
                    } else {
                        AMBER
                    }),
                );
                ui.label(
                    RichText::new(if self.shared.building.load(Ordering::Relaxed) {
                        "Rebuilding…".into()
                    } else {
                        format!(
                            "{} over budget",
                            self.shared.over_budget.load(Ordering::Relaxed)
                        )
                    })
                    .small()
                    .color(MUTED),
                );
            });
        });
    }
    fn board(&self, ui: &mut Ui, size: Vec2) {
        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 8.0, PANEL);
        let Some(v) = &self.view else {
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "Building the instrument…",
                FontId::proportional(18.0),
                MUTED,
            );
            return;
        };
        let aspect = ((v.max.x - v.min.x) / (v.max.y - v.min.y)) as f32;
        let h = (r.height() - 28.0).min((r.width() - 80.0) / aspect);
        let br = Rect::from_center_size(r.center(), vec2(h * aspect, h));
        let to = |q: Point| {
            pos2(
                br.left() + (q.x - v.min.x) as f32 / (v.max.x - v.min.x) as f32 * br.width(),
                br.bottom() - (q.y - v.min.y) as f32 / (v.max.y - v.min.y) as f32 * br.height(),
            )
        };
        if let Some(t) = &self.texture {
            p.image(
                t.id(),
                br,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let mut outline: Vec<_> = v.outline.iter().map(|q| to(*q)).collect();
        if let Some(first) = outline.first().copied() {
            outline.push(first);
        }
        p.add(Shape::line(
            outline,
            Stroke::new(1.5, Color32::from_rgb(103, 103, 97)),
        ));
        for bridge in &v.bridges {
            p.add(Shape::line(
                bridge.iter().map(|q| to(*q)).collect(),
                Stroke::new(2.0, Color32::from_gray(183)),
            ));
        }
        for (i, q) in v.string_bridge.iter().enumerate() {
            let lv = self.snap.string_level.get(i).copied().unwrap_or(0.0);
            if lv > 2e-7 {
                let a = (0.3 + 0.12 * (lv / 2e-7).log10()).clamp(0.0, 1.0);
                p.circle_filled(to(*q), 2.8, AMBER.gamma_multiply(a));
            }
        }
        p.text(
            pos2(br.center().x, r.bottom() - 10.0),
            Align2::CENTER_BOTTOM,
            "KEYBOARD",
            FontId::proportional(9.0),
            MUTED,
        );
    }
    fn strings(&mut self, ui: &mut Ui, size: Vec2) {
        ui.horizontal(|ui| {
            caption(ui, "STRINGS");
            ui.label(RichText::new(note_name(self.snap.focus_midi)).color(AMBER));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "Felt {:.1} / {:.1} / {:.1} N",
                        self.snap.hammer_force[0],
                        self.snap.hammer_force[1],
                        self.snap.hammer_force[2]
                    ))
                    .small()
                    .color(MUTED),
                );
            });
        });
        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 6.0, PANEL);
        p.line_segment(
            [
                pos2(r.left() + 10.0, r.center().y),
                pos2(r.right() - 10.0, r.center().y),
            ],
            Stroke::new(1.0, Color32::from_gray(55)),
        );
        let peak = self
            .snap
            .shape
            .iter()
            .zip(self.snap.shape_nodes)
            .flat_map(|(s, n)| s[..n.min(s.len())].iter())
            .fold(1e-7f32, |m, x| m.max(x.abs()));
        self.string_scale = peak.max(self.string_scale * 0.98);
        for i in 0..3 {
            let n = self.snap.shape_nodes[i];
            if n < 2 {
                continue;
            }
            let points = (0..n)
                .map(|j| {
                    pos2(
                        r.left() + 10.0 + (r.width() - 20.0) * j as f32 / (n - 1) as f32,
                        r.center().y
                            - self.snap.shape[i][j] / self.string_scale * (r.height() * 0.4),
                    )
                })
                .collect();
            p.add(Shape::line(
                points,
                Stroke::new(
                    1.2,
                    [
                        AMBER,
                        Color32::from_rgb(106, 158, 187),
                        Color32::from_rgb(157, 178, 143),
                    ][i],
                ),
            ));
        }
        p.text(
            r.left_bottom() + vec2(10.0, -6.0),
            Align2::LEFT_BOTTOM,
            format!("± {:.3} mm", self.string_scale * 1000.0),
            FontId::monospace(10.0),
            MUTED,
        );
    }
    fn controls(&self, ui: &mut Ui, setter: &ParamSetter) {
        for (p, s) in PARAMS
            .iter()
            .zip(&self.params.settings)
            .filter(|(p, _)| p.tab == self.tab)
        {
            match s {
                Setting::Number(param) => {
                    ui.horizontal(|ui| {
                        ui.label(p.name);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(value_label(p.format, param.value() as f64))
                                    .monospace()
                                    .color(AMBER),
                            );
                        });
                    });
                    let mut v = param.value();
                    let response = ui
                        .add_sized(
                            vec2(ui.available_width(), 18.0),
                            Slider::new(&mut v, p.lo as f32..=p.hi as f32)
                                .logarithmic(p.log)
                                .show_value(false),
                        )
                        .on_hover_text(p.help);
                    if response.drag_started() {
                        setter.begin_set_parameter(param);
                    }
                    if response.changed() {
                        if !response.dragged() {
                            setter.begin_set_parameter(param);
                        }
                        setter.set_parameter(param, v);
                        if !response.dragged() {
                            setter.end_set_parameter(param);
                        }
                    }
                    if response.drag_stopped() {
                        setter.end_set_parameter(param);
                    }
                }
                Setting::Switch(param) => {
                    let mut v = param.value();
                    let label = p.name.split(" (").next().unwrap_or(p.name);
                    if ui
                        .checkbox(&mut v, label)
                        .on_hover_text(format!("{}\n{}", p.name, p.help))
                        .changed()
                    {
                        set_switch(setter, param, v);
                    }
                }
            }
        }
    }
    fn parts(&self, ui: &mut Ui, setter: &ParamSetter) {
        ui.horizontal_wrapped(|ui| {
            caption(ui, "PARTS");
            for (name, label) in [
                ("Room (walls, floor, ceiling, reverberation)", "Room"),
                ("Sideways string motion (of struck strings)", "Sideways"),
                (
                    "Duplex lengths behind the bridge (treble shimmer)",
                    "Duplex",
                ),
                ("Air loading (the air's mass and drag on the board)", "Air"),
            ] {
                if let Some((p, Setting::Switch(s))) = PARAMS
                    .iter()
                    .zip(&self.params.settings)
                    .find(|(p, _)| p.name == name)
                {
                    let mut v = s.value();
                    if ui
                        .toggle_value(&mut v, label)
                        .on_hover_text(p.name)
                        .changed()
                    {
                        set_switch(setter, s, v);
                    }
                }
            }
        });
    }
    fn room(&mut self, ui: &mut Ui, setter: &ParamSetter) {
        caption(ui, "LISTENING POSITION");
        ui.horizontal(|ui| {
            ui.label(RichText::new("Listener · spaced ears").color(AMBER));
            ui.label(RichText::new("Drag on plan").small().color(MUTED));
        });
        let mut o = self.params.options();
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 175.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 6.0, PANEL);
        let scale =
            ((r.width() - 24.0) / o.room_lx as f32).min((r.height() - 24.0) / o.room_ly as f32);
        let room = Rect::from_center_size(
            r.center(),
            vec2(o.room_lx as f32 * scale, o.room_ly as f32 * scale),
        );
        let to = |x: f64, y: f64| {
            pos2(
                room.left() + x as f32 * scale,
                room.bottom() - y as f32 * scale,
            )
        };
        p.rect_stroke(room, 0.0, Stroke::new(1.0, MUTED), StrokeKind::Inside);
        let listener = to(o.listener_x, o.listener_y);
        let piano = to(o.piano_x + 0.7, o.piano_y + 0.7);
        if let Some(v) = &self.view {
            let points: Vec<_> = v
                .outline
                .iter()
                .map(|q| to(o.piano_x + q.x, o.piano_y + q.y))
                .collect();
            p.add(Shape::convex_polygon(
                points,
                AMBER.gamma_multiply(0.45),
                Stroke::new(1.0, AMBER),
            ));
        }
        p.circle_filled(listener, 5.0, Color32::LIGHT_BLUE);
        p.line_segment([listener, piano], Stroke::new(0.5, Color32::from_gray(90)));
        p.text(
            listener + vec2(7.0, 0.0),
            Align2::LEFT_CENTER,
            "Listener",
            FontId::proportional(10.0),
            Color32::LIGHT_BLUE,
        );
        // Object-specific hit targets leave empty space available to the scroll area.
        let piano_hit = ui.interact(
            Rect::from_center_size(piano, vec2(1.4 * scale, 1.5 * scale)),
            ui.id().with("piano-drag"),
            Sense::drag(),
        );
        let listener_hit = ui.interact(
            Rect::from_center_size(listener, vec2(28.0, 28.0)),
            ui.id().with("listener-drag"),
            Sense::drag(),
        );
        for (is_listener, hit) in [(false, piano_hit), (true, listener_hit)] {
            if hit.dragged() {
                let delta = hit.drag_delta();
                let dx = delta.x as f64 / scale as f64;
                let dy = -delta.y as f64 / scale as f64;
                if is_listener {
                    o.listener_x = (o.listener_x + dx).clamp(0.3, o.room_lx - 0.3);
                    o.listener_y = (o.listener_y + dy).clamp(0.3, o.room_ly - 0.3);
                } else {
                    o.piano_x = (o.piano_x + dx).clamp(0.2, (o.room_lx - 1.6).max(0.2));
                    o.piano_y = (o.piano_y + dy).clamp(0.8, (o.room_ly - 1.7).max(0.8));
                }
                self.apply_options(setter, &o);
            }
        }
        ui.add_space(4.0);
    }
    fn keyboard(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            caption(ui, "88 KEYS");
            for (text, event, on) in [
                (
                    "Space · sustain",
                    Input::Sustain(
                        if self.shared.sustain.load(Ordering::Relaxed) > 0 {
                            0.0
                        } else {
                            1.0
                        },
                        FROM_UI,
                    ),
                    self.shared.sustain.load(Ordering::Relaxed) > 0,
                ),
                (
                    "Comma · soft",
                    Input::Soft(!self.shared.soft.load(Ordering::Relaxed), FROM_UI),
                    self.shared.soft.load(Ordering::Relaxed),
                ),
                (
                    "Period · sostenuto",
                    Input::Sostenuto(!self.shared.sostenuto.load(Ordering::Relaxed), FROM_UI),
                    self.shared.sostenuto.load(Ordering::Relaxed),
                ),
            ] {
                if ui.selectable_label(on, text).clicked() {
                    self.shared.send(event);
                }
            }
        });
        let (r, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 90.0), Sense::click_and_drag());
        let p = ui.painter();
        let ww = r.width() / 52.0;
        let mut rects = Vec::new();
        let mut whites = 0;
        for m in 21..=108 {
            if black(m) {
                rects.push((
                    m,
                    Rect::from_min_size(
                        pos2(r.left() + whites as f32 * ww - ww * 0.3, r.top()),
                        vec2(ww * 0.6, r.height() * 0.63),
                    ),
                ));
            } else {
                let kr = Rect::from_min_size(
                    pos2(r.left() + whites as f32 * ww, r.top()),
                    vec2(ww - 1.0, r.height()),
                );
                rects.push((m, kr));
                whites += 1;
            }
        }
        for is_black in [false, true] {
            for (m, kr) in rects.iter().filter(|(m, _)| black(*m) == is_black) {
                let down = self.shared.held[*m as usize].load(Ordering::Relaxed) != 0
                    || self.mouse_note == Some(*m)
                    || self.key_notes.values().any(|n| n == m);
                let color = if down {
                    AMBER
                } else if is_black {
                    Color32::from_rgb(24, 28, 30)
                } else {
                    Color32::from_rgb(218, 220, 213)
                };
                p.rect_filled(*kr, 2.0, color);
                if *m % 12 == 0 && !is_black {
                    p.text(
                        kr.center_bottom() + vec2(0.0, -7.0),
                        Align2::CENTER_BOTTOM,
                        note_name(*m),
                        FontId::proportional(9.0),
                        Color32::from_gray(80),
                    );
                }
            }
        }
        let held = ui.input(|i| i.pointer.primary_down());
        let pos = response.interact_pointer_pos();
        let hit = if held && (response.dragged() || response.is_pointer_button_down_on()) {
            pos.and_then(|pos| {
                rects
                    .iter()
                    .filter(|(m, _)| black(*m))
                    .chain(rects.iter().filter(|(m, _)| !black(*m)))
                    .find(|(_, r)| r.contains(pos))
                    .map(|(m, _)| *m)
            })
        } else {
            None
        };
        if hit != self.mouse_note {
            if let Some(m) = self.mouse_note {
                self.shared.send(Input::NoteOff {
                    midi: m,
                    from: FROM_MOUSE,
                    release: 1.0,
                });
            }
            if let Some(m) = hit {
                self.shared.send(Input::NoteOn {
                    midi: m,
                    velocity: self.velocity,
                    from: FROM_MOUSE,
                });
            }
            self.mouse_note = hit;
        }
    }
    fn computer_keys(&mut self, ctx: &Context) {
        let keys = [
            Key::A,
            Key::W,
            Key::S,
            Key::E,
            Key::D,
            Key::F,
            Key::T,
            Key::G,
            Key::Y,
            Key::H,
            Key::U,
            Key::J,
            Key::K,
            Key::O,
            Key::L,
            Key::P,
            Key::Semicolon,
        ];
        for event in ctx.input(|i| i.events.clone()) {
            if let Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            {
                if repeat || (pressed && (modifiers.command || modifiers.alt || self.saving)) {
                    continue;
                }
                if let Some(i) = keys.iter().position(|k| *k == key) {
                    if pressed {
                        let midi = (self.octave + 1) * 12 + i as i32;
                        if (21..=108).contains(&midi) {
                            self.key_notes.insert(key, midi);
                            self.shared.send(Input::NoteOn {
                                midi,
                                velocity: self.velocity,
                                from: FROM_UI,
                            });
                        }
                    } else if let Some(midi) = self.key_notes.remove(&key) {
                        self.shared.send(Input::NoteOff {
                            midi,
                            from: FROM_UI,
                            release: 1.0,
                        });
                    }
                } else {
                    match key {
                        Key::Z if pressed => self.octave = (self.octave - 1).max(0),
                        Key::X if pressed => self.octave = (self.octave + 1).min(7),
                        Key::C if pressed => self.velocity = (self.velocity - 15).max(10),
                        Key::V if pressed => self.velocity = (self.velocity + 15).min(127),
                        Key::Space => self
                            .shared
                            .send(Input::Sustain(if pressed { 1.0 } else { 0.0 }, FROM_UI)),
                        Key::Comma => self.shared.send(Input::Soft(pressed, FROM_UI)),
                        Key::Period => self.shared.send(Input::Sostenuto(pressed, FROM_UI)),
                        Key::Enter if pressed && STANDALONE.load(Ordering::Relaxed) => {
                            self.shared.sound_on.store(
                                !self.shared.sound_on.load(Ordering::Relaxed),
                                Ordering::Relaxed,
                            )
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    fn release_ui(&mut self) {
        for (_, midi) in std::mem::take(&mut self.key_notes) {
            self.shared.send(Input::NoteOff {
                midi,
                from: FROM_UI,
                release: 1.0,
            });
        }
        if let Some(midi) = self.mouse_note.take() {
            self.shared.send(Input::NoteOff {
                midi,
                from: FROM_MOUSE,
                release: 1.0,
            });
        }
    }
    fn apply_options(&self, setter: &ParamSetter, o: &GrandOptions) {
        for (p, s) in PARAMS.iter().zip(&self.params.settings) {
            let v = (p.get)(o);
            match s {
                Setting::Number(f) if (f.value() as f64 - v).abs() > 1e-6 => {
                    set_number(setter, f, v as f32)
                }
                Setting::Switch(b) if b.value() != (v >= 0.5) => set_switch(setter, b, v >= 0.5),
                _ => {}
            }
        }
    }
    fn capture(&self) -> Setup {
        let o = self.params.options();
        Setup {
            values: PARAMS.iter().map(|p| (p.get)(&o)).collect(),
            grid: self.params.grid.value(),
            volume: self.params.volume.value(),
        }
    }
    fn apply_setup(&self, setter: &ParamSetter, s: &Setup) {
        let mut o = self.params.options();
        for (p, v) in PARAMS.iter().zip(&s.values) {
            p.write(&mut o, *v);
        }
        self.apply_options(setter, &o);
        set_number(setter, &self.params.volume, s.volume);
        setter.begin_set_parameter(&self.params.grid);
        setter.set_parameter(&self.params.grid, s.grid.clamp(0, 3));
        setter.end_set_parameter(&self.params.grid);
    }
    fn save_setups(&self) -> Result<(), String> {
        let path = setup_path().ok_or("No home directory")?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(&self.setups).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.release_ui();
    }
}
