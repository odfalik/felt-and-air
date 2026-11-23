//! One editor for the host and the standalone. Geometry is cached per rebuild, heat map
//! uploads happen at the snapshot rate, painting at 60 Hz. All controls use host gestures.
mod guide;
#[cfg(test)]
#[path = "editor/headless_tests.rs"]
mod headless_tests;
mod illustration;
use crate::{demo::Demo, presets, setup::Setup, shared::*, PianoParams, Setting};
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui::*, resizable_window::ResizableWindow};
use piano_engine::{params::PARAMS, GrandOptions, PianoDesign, Snapshot, Vec2 as Point};
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
    // Only musical key-down events acquire these flags, never pedal buttons/MIDI.
    key_sustain_down: bool,
    key_soft_down: bool,
    key_sostenuto_down: bool,
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
    active_param: usize,
    extra_monitors: bool,
    scroll_debug: Option<(Rect, Vec2, Vec2)>,
    last_wheel: Option<(Vec2, Option<Pos2>)>,
    preview_values: Vec<f64>,
    preview_design: PianoDesign,
    grand_art: illustration::GrandIllustration,
    action_art: illustration::action::ActionIllustration,
}

pub fn create(
    params: Arc<PianoParams>,
    shared: Arc<Shared>,
    demos: Arc<Vec<Demo>>,
) -> Option<Box<dyn Editor>> {
    let setups = setup_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|s| serde_json::from_slice(&s).ok())
        .unwrap_or_default();
    let state = State::new(params.clone(), shared, demos, setups);
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
#[cfg(test)]
fn trace_control(ui: &Ui, kind: &str, param: &impl Param, response: &Response) {
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            Id::new(format!("{kind}:{}", param.name())),
            (response.rect, ui.clip_rect(), response.enabled()),
        )
    });
}
#[cfg(test)]
fn trace_left_target(ui:&Ui,name:&str,rect:Rect) {
    let snapshot=(rect,ui.clip_rect());
    ui.ctx().data_mut(|data|data.insert_temp(Id::new(format!("left:{name}")),snapshot));
}
fn bounded_selector_menu(ui:&mut Ui,width:f32) {
    let inner=(width-Frame::popup(ui.style()).total_margin().sum().x).max(0.0);
    ui.set_width(ui.available_width().min(inner));
    ui.style_mut().wrap_mode=Some(TextWrapMode::Wrap);
}
#[cfg(test)]
type SelectorTrace=(String,Id,Rect,Rect);
#[cfg(test)]
fn trace_selector(ui:&Ui,name:&str,response:&Response) {
    let snapshot=(name.to_owned(),response.id,response.rect,ui.clip_rect());
    ui.ctx().data_mut(|data| {
        let id=Id::new("selector:responses");
        let mut responses=data.get_temp::<Vec<SelectorTrace>>(id).unwrap_or_default();
        responses.push(snapshot); data.insert_temp(id,responses);
    });
}
#[cfg(test)]
fn trace_compact_widget(ui:&Ui,name:&str,response:&Response) {
    // Snapshot fields outside the Context lock; never query focus inside data_mut.
    let snapshot=(name.to_owned(),response.rect,ui.clip_rect());
    ui.ctx().data_mut(|data| {
        let id=Id::new("compact:widgets");
        let mut widgets=data.get_temp::<Vec<(String,Rect,Rect)>>(id).unwrap_or_default();
        widgets.push(snapshot);
        data.insert_temp(id,widgets);
    });
}
/// All reset targets come from the same NIH parameter metadata as host defaults.
/// Compare the unmodulated base: host modulation is not itself resettable.
fn default_reset<P: Param>(ui: &mut Ui, setter: &ParamSetter, param: &P, value: String) -> bool {
    let enabled = param.unmodulated_plain_value() != param.default_plain_value();
    let help = format!("Reset to default: {value}");
    let response = ui
        .add_enabled(
            enabled,
            Button::new("")
                .min_size(vec2(18.0, 18.0))
                .corner_radius(9.0),
        )
        .on_hover_text(&help)
        .on_disabled_hover_text(&help);
    #[cfg(test)]
    trace_control(ui, "reset", param, &response);
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            enabled,
            format!("Reset {} to default", param.name()),
        )
    });
    let c = response.rect.center();
    let color = if enabled {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let points: Vec<_> = (0..=18)
        .map(|i| {
            let a = -0.6 * std::f32::consts::PI + i as f32 / 18.0 * 1.65 * std::f32::consts::PI;
            c + vec2(a.cos(), a.sin()) * 4.5
        })
        .collect();
    let tip = *points.last().unwrap();
    let back = points[points.len() - 3];
    let tangent = (tip - back).normalized();
    let normal = vec2(-tangent.y, tangent.x);
    ui.painter()
        .add(Shape::line(points, Stroke::new(1.1, color)));
    ui.painter().add(Shape::convex_polygon(
        vec![
            tip + tangent * 1.3,
            tip - tangent * 2.6 + normal * 2.0,
            tip - tangent * 2.6 - normal * 2.0,
        ],
        color,
        Stroke::NONE,
    ));
    if response.clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, param.default_plain_value());
        setter.end_set_parameter(param);
        true
    } else {
        false
    }
}
fn grid_label(index: i32) -> String {
    crate::setup::grid_scale(index)
        .map(|v| format!("{v}×"))
        .unwrap_or_else(|| "—".into())
}
fn volume_icon(ui: &mut Ui) {
    let (r, response) = ui.allocate_exact_size(vec2(20.0, 18.0), Sense::hover());
    let c = r.center();
    let color = ui.visuals().text_color();
    ui.painter().rect_filled(
        Rect::from_center_size(c + vec2(-5.0, 0.0), vec2(4.0, 6.0)),
        0.0,
        color,
    );
    ui.painter().add(Shape::convex_polygon(
        vec![
            c + vec2(-3.0, -3.0),
            c + vec2(1.0, -6.0),
            c + vec2(1.0, 6.0),
            c + vec2(-3.0, 3.0),
        ],
        color,
        Stroke::NONE,
    ));
    for radius in [4.0, 7.0] {
        let points = (0..=12)
            .map(|i| {
                let a = -0.7 + i as f32 / 12.0 * 1.4;
                c + vec2(1.0 + radius * a.cos(), radius * a.sin())
            })
            .collect();
        ui.painter()
            .add(Shape::line(points, Stroke::new(1.0, color)));
    }
    response.on_hover_text("Volume");
}
fn controls_scroll<R>(
    ui: &mut Ui,
    tab: usize,
    height: f32,
    body: impl FnOnce(&mut Ui) -> R,
) -> scroll_area::ScrollAreaOutput<R> {
    ScrollArea::vertical()
        .id_salt(("controls", tab))
        .max_height(height)
        .scroll_bar_visibility(scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, body)
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
        format!("{}{}{}", &fmt[..start], n, &fmt[end..]).replace(" deg", "°")
    } else {
        n
    }
}
impl State {
    fn new(
        params: Arc<PianoParams>,
        shared: Arc<Shared>,
        demos: Arc<Vec<Demo>>,
        setups: BTreeMap<String, Setup>,
    ) -> Self {
        let options = params.options();
        let preview_values = PARAMS.iter().map(|p| (p.get)(&options)).collect();
        let preview_design = piano_engine::make_grand(&options);
        Self {
            params,
            shared,
            demos,
            tab: 0,
            preset: "Recital room · listener by the lid".into(),
            demo: 0,
            octave: 4,
            velocity: 80,
            mouse_note: None,
            key_notes: BTreeMap::new(),
            key_sustain_down: false,
            key_soft_down: false,
            key_sostenuto_down: false,
            view: None,
            snap: Snapshot::default(),
            frame: u64::MAX,
            texture: None,
            board_scale: 2e-8,
            string_scale: 1e-4,
            setups,
            setup_name: String::new(),
            saving: false,
            message: String::new(),
            ui_cost: 0.0,
            active_param: 0,
            extra_monitors: false,
            scroll_debug: None,
            last_wheel: None,
            preview_values,
            preview_design,
            grand_art: illustration::GrandIllustration::default(),
            action_art: illustration::action::ActionIllustration::default(),
        }
    }
    fn sync_preview(&mut self) {
        let options = self.params.options();
        if PARAMS
            .iter()
            .zip(&self.preview_values)
            .any(|(p, value)| (p.get)(&options) != *value)
        {
            for (p, value) in PARAMS.iter().zip(&mut self.preview_values) {
                *value = (p.get)(&options);
            }
            // Pure geometry/design only: never construct or step a Piano on the UI thread.
            self.preview_design = piano_engine::make_grand(&options);
        }
    }
    fn room_preview(&mut self, ui: &mut Ui, setter: &ParamSetter, height: f32) {
        let options = self.params.options();
        let selected = PARAMS[self.active_param].name == "Lid angle";
        self.grand_art.paint(
            ui,
            (height - 170.0).max(170.0),
            options.lid_angle_deg as f32,
            selected,
        );
        let plan_width = ui.available_width() * 0.56;
        let tail_width = ui.available_width() - plan_width - 10.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(plan_width,150.0),Layout::top_down(Align::Min),|ui| {
                ui.set_max_width(plan_width);
                self.room(ui,setter,150.0);
            });
            ui.allocate_ui_with_layout(vec2(tail_width,150.0),Layout::top_down(Align::Min),|ui| {
                ui.set_max_width(tail_width);
                let tail=guide::room_decay(&options);
                ui.label(RichText::new(if options.room_enabled {format!("Room tail ≈ {tail:.2} s")} else {"Room tail off".into()}).color(AMBER))
                    .on_hover_text("Sabine estimate from room size and absorption, not measured audio. More absorption shortens this tail.");
                let _decay=ui.collapsing("Decay estimate",|ui|guide::decay(ui,&options,60.0));
                #[cfg(test)]
                {
                    trace_left_target(ui,"decay-header",_decay.header_response.rect);
                    if let Some(body)=&_decay.body_response { trace_left_target(ui,"decay-body",body.rect); }
                    let open=_decay.body_returned.is_some();
                    ui.ctx().data_mut(|data|data.insert_temp(Id::new("left:decay-open"),open));
                }
            });
        });
    }
    fn settings_preview(&mut self, ui: &mut Ui, height: f32) {
        let options = self.params.options();
        Frame::new().fill(PANEL).inner_margin(9.0).show(ui, |ui| {
            guide::instrument(
                ui,
                self.tab,
                &options,
                &self.preview_design,
                self.snap.focus_midi,
                height,
            );
        });
    }
    fn refresh(&mut self, ctx: &Context) {
        let frame = self.shared.display_frames.load(Ordering::Acquire);
        if frame == self.frame {
            return;
        }
        let generation;
        if let Ok(d) = self.shared.display.try_lock() {
            generation=d.generation;
            self.view = d.view.clone();
            self.snap.clone_from(&d.snap);
            self.frame = frame;
        } else {
            return;
        }
        // The generation and owned Snapshot were captured together; decode outside the lock.
        let now=ctx.input(|input|input.time);
        self.action_art.receive(generation,&self.snap.action,now);
        if self.tab != 3 && !self.extra_monitors {
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
        ctx.input(|i| {
            if i.raw_scroll_delta != Vec2::ZERO {
                self.last_wheel = Some((i.raw_scroll_delta, i.pointer.hover_pos()));
            }
        });
        #[cfg(test)]
        ctx.data_mut(|data|data.insert_temp(Id::new("compact:widgets"),Vec::<(String,Rect,Rect)>::new()));
        self.refresh(ctx);
        self.sync_preview();
        self.computer_keys(ctx);
        // Losing focus releases only this editor's notes, never another source's keys.
        if !ctx.input(|i| i.focused) {
            self.release_ui();
        }
        ResizableWindow::new("piano").min_size(vec2(800.0,600.0)).show(ctx,&self.params.editor_state.clone(),|ui| {
            Frame::new().inner_margin(14.0).show(ui,|ui| {
                self.header(ui,setter);
                ui.separator();
                // One category selection drives the visualization and its controls together.
                ui.horizontal(|ui| {
                    for (i,name) in guide::TABS.iter().enumerate() {
                        let tab_response=ui.selectable_label(self.tab==i,*name).on_hover_text(guide::category_help(i));
                        #[cfg(test)]
                        trace_compact_widget(ui,&format!("category:{i}"),&tab_response);
                        if tab_response.clicked() {
                            self.tab=i;
                            self.active_param=PARAMS.iter().position(|p|p.tab==i).unwrap_or(0);
                            self.frame=u64::MAX;
                        }
                    }
                });
                ui.separator();
                // Preserve the common footer reservation; scroll short body content.
                let h=(ui.available_height()-185.0).max(100.0);
                let body_width=(ui.available_width()-ui.spacing().item_spacing.x).max(0.0);
                let right=(body_width*0.46).min(440.0);
                let left=body_width-right;
                ui.allocate_ui_with_layout(vec2(ui.available_width(),h),Layout::left_to_right(Align::Min),|ui| {
                    ui.set_min_height(h);
                    ui.allocate_ui_with_layout(vec2(left,h),Layout::top_down(Align::Min),|ui| {
                        ui.set_max_width(left);
                        ui.style_mut().spacing.scroll=style::ScrollStyle::solid();
                        let _visual_scroll=ScrollArea::vertical().drag_to_scroll(false).max_height(h).id_salt(("visual",self.tab)).show(ui,|ui| {
                            match self.tab {
                                0=>self.room_preview(ui,setter,h-36.0),
                                1=>{self.action_art.paint(ui,&self.snap.action,PARAMS[self.active_param].name,(h-185.0).clamp(170.0,330.0));},
                                2=>{
                                    self.settings_preview(ui,125.0);
                                    self.strings(ui,vec2(ui.available_width(),115.0));
                                },
                                _=>{
                                    let board_width=ui.available_width()*0.61;
                                    let section_width=ui.available_width()-board_width-10.0;
                                    ui.horizontal_top(|ui| {
                                        ui.allocate_ui_with_layout(vec2(board_width,360.0),Layout::top_down(Align::Min),|ui| {
                                            ui.set_max_width(board_width);
                                            self.board(ui,vec2(board_width,(h-145.0).clamp(90.0,360.0)));
                                            self.motion_legend(ui);
                                        });
                                        ui.allocate_ui_with_layout(vec2(section_width,260.0),Layout::top_down(Align::Min),|ui| {
                                            ui.set_max_width(section_width);
                                            self.settings_preview(ui,180.0);
                                        });
                                    });
                                },
                            }
                            let more=CollapsingHeader::new("Other live motion monitors").id_salt("extra-motion").show(ui,|ui| {
                                if self.tab!=3 {
                                    ui.label(RichText::new("Simulated soundboard motion · magnified, not air pressure").color(Color32::from_rgb(232,231,221)));
                                    self.board(ui,vec2(ui.available_width(),190.0));
                                    self.motion_legend(ui);
                                }
                                if self.tab!=2 { self.strings(ui,vec2(ui.available_width(),85.0)); }
                            });
                            let open=more.body_returned.is_some();
                            if open!=self.extra_monitors { self.extra_monitors=open; self.frame=u64::MAX; }
                        });
                        #[cfg(test)]
                        {
                            let snapshot=(_visual_scroll.inner_rect,_visual_scroll.content_size,_visual_scroll.state.offset);
                            ui.ctx().data_mut(|data|data.insert_temp(Id::new("left:scroll"),snapshot));
                        }
                    });
                    ui.allocate_ui_with_layout(vec2(right,h),Layout::top_down(Align::Min),|ui| {
                        ui.set_max_width(right); ui.set_max_height(h);
                        let top=ui.cursor().top();
                        // Physical labels and contextual hover help live on the controls, not a second heading.
                        ui.style_mut().spacing.scroll=style::ScrollStyle::solid();
                        let remaining=(h-(ui.cursor().top()-top)-4.0).max(100.0);
                        let scroll=controls_scroll(ui,self.tab,remaining,|ui|self.controls(ui,setter));
                        self.scroll_debug=Some((scroll.inner_rect,scroll.content_size,scroll.state.offset));
                    });
                });
                ui.separator(); self.keyboard(ui);
                ui.horizontal(|ui| {
                    let help_response=ui.menu_button("Keyboard help",|ui| {
                        ui.label("A W S E D F T G Y H U J K O L P ;");
                        ui.label(format!("Z/X octave {} · C/V velocity {}",self.octave,self.velocity));
                        ui.label("Hold Space for sustain, comma for soft pedal, period for sostenuto.");
                    });
                    #[cfg(test)]
                    trace_compact_widget(ui,"footer:help",&help_response.response);

                });
            });
        });
        if self.saving {
            Window::new("Save setup")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Give this instrument a name");
                    let _name_response=ui.text_edit_singleline(&mut self.setup_name);
                    #[cfg(test)]
                    let name_focused=_name_response.has_focus();
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(Id::new("editor:setup-name"),_name_response.rect);
                        data.insert_temp(Id::new("editor:setup-name-focused"),name_focused);
                    });
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
                        let cancel_response=ui.button("Cancel");
                        #[cfg(test)]
                        ui.ctx().data_mut(|data|data.insert_temp(Id::new("editor:setup-cancel"),cancel_response.rect));
                        if cancel_response.clicked() {
                            self.saving = false;
                        }
                    });
                });
        }
        self.ui_cost = self.ui_cost * 0.95 + start.elapsed().as_secs_f32() * 1000.0 * 0.05;
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    fn header(&mut self, ui: &mut Ui, setter: &ParamSetter) {
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                let title_response=ui.heading(RichText::new("Felt & Air").color(Color32::from_rgb(232, 231, 221)));
                #[cfg(test)]
                trace_compact_widget(ui,"header:title",&title_response);
            });
            ui.add_space(16.0);
            let selector_height=2.0*ui.spacing().interact_size.y+ui.spacing().item_spacing.y;
            ui.allocate_ui_with_layout(vec2(235.0,selector_height),Layout::top_down(Align::Min),|ui| {
                ui.set_width(235.0);
                let preset_response=ComboBox::from_id_salt("preset")
                    .width(235.0)
                    .truncate()
                    .selected_text(&self.preset)
                    .show_ui(ui, |ui| {
                        bounded_selector_menu(ui,235.0);
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
                            let saved_response=ui.button(format!("Saved · {name}"));
                            #[cfg(test)]
                            trace_selector(ui,&format!("setup:{name}"),&saved_response);
                            if saved_response.clicked() {
                                self.apply_setup(setter, &setup);
                                self.preset = name;
                            }
                        }
                        #[cfg(test)]
                        {
                            let snapshot=(ui.min_rect(),ui.clip_rect());
                            ui.ctx().data_mut(|data|data.insert_temp(Id::new("selector:popup:preset"),snapshot));
                        }
                    });
                preset_response.response.clone().on_hover_text(self.preset.clone());
                #[cfg(test)]
                {
                    trace_compact_widget(ui,"header:preset",&preset_response.response);
                    trace_selector(ui,"preset",&preset_response.response);
                }
                ui.horizontal(|ui| {
                    let save_response=ui.small_button("Save setup…");
                    #[cfg(test)]
                    ui.ctx().data_mut(|data|data.insert_temp(Id::new("editor:save-open"),save_response.rect));
                    #[cfg(test)]
                    trace_compact_widget(ui,"header:save",&save_response);
                    if save_response.clicked() {
                        self.saving = true;
                    }
                    if !self.message.is_empty() {
                        ui.label(RichText::new(&self.message).small().color(MUTED));
                    }
                });
            });
            ui.separator();
            ui.allocate_ui_with_layout(vec2(240.0,selector_height),Layout::top_down(Align::Min),|ui| {
                ui.set_width(240.0);
                let demo_response=ComboBox::from_id_salt("demo")
                    .width(240.0)
                    .truncate()
                    .selected_text(
                        self.demos
                            .get(self.demo)
                            .map_or("No demos installed", |d| d.title.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        bounded_selector_menu(ui,240.0);
                        for (i, d) in self.demos.iter().enumerate() {
                            let _demo_item=ui.selectable_value(&mut self.demo, i, &d.title)
                                .on_hover_text(&d.note);
                            #[cfg(test)]
                            trace_selector(ui,&format!("demo-item:{i}"),&_demo_item);
                        }
                        #[cfg(test)]
                        {
                            let snapshot=(ui.min_rect(),ui.clip_rect());
                            ui.ctx().data_mut(|data|data.insert_temp(Id::new("selector:popup:demo"),snapshot));
                        }
                    });
                demo_response.response.clone().on_hover_text(
                    self.demos.get(self.demo).map_or("No demos installed",|d|d.title.as_str()));
                #[cfg(test)]
                {
                    trace_compact_widget(ui,"header:demo",&demo_response.response);
                    trace_selector(ui,"demo",&demo_response.response);
                }
                ui.horizontal(|ui| {
                    let playing = self.shared.demo_playing.load(Ordering::Relaxed);
                    let play_response=ui.add_enabled(
                        !self.demos.is_empty(),
                        Button::new(if playing < 0 { "▶ Play" } else { "■ Stop" }),
                    );
                    #[cfg(test)]
                    trace_compact_widget(ui,"header:play",&play_response);
                    if play_response.clicked() {
                        self.shared.send(if playing < 0 {
                            Input::PlayDemo(self.demo)
                        } else {
                            Input::StopDemo
                        });
                    }
                    let seconds = self.shared.demo_seconds.load();
                    let elapsed_response=ui.label(
                        RichText::new(format!(
                            "{}:{:02}",
                            seconds as u32 / 60,
                            seconds as u32 % 60
                        ))
                        .monospace()
                        .small(),
                    );
                    #[cfg(test)]
                    trace_compact_widget(ui,"header:elapsed",&elapsed_response);
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
            // Reserve the complete existing two-row group BEFORE wrapped placement.
            // 20 icon + slider rail + DragValue minimum + 18 reset + three gaps.
            let volume_group_size=vec2(
                20.0+ui.spacing().slider_width+ui.spacing().interact_size.x+18.0+3.0*ui.spacing().item_spacing.x,
                2.0*ui.spacing().interact_size.y+ui.spacing().item_spacing.y,
            );
            ui.allocate_ui_with_layout(volume_group_size,Layout::top_down(Align::Min),|ui| {
                ui.horizontal(|ui| {
                    volume_icon(ui);
                    let mut vol = self.params.volume.value();
                    let response = ui
                        .add(Slider::new(&mut vol, 0.0..=3.0).fixed_decimals(2))
                        .on_hover_text("Volume");
                    #[cfg(test)]
                    trace_control(ui, "control", &self.params.volume, &response);
                    response
                        .widget_info(|| WidgetInfo::slider(ui.is_enabled(), vol as f64, "Volume"));
                    if response.changed() {
                        set_number(setter, &self.params.volume, vol);
                        self.preset = "Custom settings".into();
                    }
                    if default_reset(
                        ui,
                        setter,
                        &self.params.volume,
                        value_label("%.2f", self.params.volume.default_plain_value() as f64),
                    ) {
                        self.preset = "Custom settings".into();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Grid").on_hover_text(
                        "Soundboard simulation grid spacing. Wider spacing reduces CPU work.",
                    );
                    let grid_response=ComboBox::from_id_salt("grid")
                        .width(70.0)
                        .selected_text(grid_label(self.params.grid.value()))
                        .show_ui(ui, |ui| {
                            let range = self.params.grid.range();
                            let a = range.unnormalize(0.0);
                            let b = range.unnormalize(1.0);
                            for i in a.min(b)..=a.max(b) {
                                if ui
                                    .selectable_label(self.params.grid.value() == i, grid_label(i))
                                    .clicked()
                                    && self.params.grid.value() != i
                                {
                                    setter.begin_set_parameter(&self.params.grid);
                                    setter.set_parameter(&self.params.grid, i);
                                    setter.end_set_parameter(&self.params.grid);
                                    self.preset = "Custom settings".into();
                                }
                            }
                        });
                    #[cfg(test)]
                    trace_compact_widget(ui,"header:grid",&grid_response.response);
                    if default_reset(
                        ui,
                        setter,
                        &self.params.grid,
                        grid_label(self.params.grid.default_plain_value()),
                    ) {
                        self.preset = "Custom settings".into();
                    }
                });
            });
            let button_width = ["Sound on", "Sound off", "Diagnostics"]
                .into_iter()
                .map(|label| WidgetText::from(label)
                    .into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Button)
                    .size().x + 2.0 * ui.spacing().button_padding.x)
                .fold(ui.spacing().interact_size.x, f32::max);
            let info_rows = 1.0 + u8::from(STANDALONE.load(Ordering::Relaxed)) as f32
                + u8::from(self.shared.building.load(Ordering::Relaxed)) as f32;
            let info_group_size = vec2(button_width,
                info_rows * ui.spacing().interact_size.y
                    + (info_rows - 1.0) * ui.spacing().item_spacing.y);
            ui.allocate_ui_with_layout(info_group_size, Layout::top_down(Align::Min), |ui| {
                if STANDALONE.load(Ordering::Relaxed) {
                    let mut sound = self.shared.sound_on.load(Ordering::Relaxed);
                    let label = if sound { "Sound on" } else { "Sound off" };
                    let sound_response=ui.toggle_value(&mut sound, label);
                    #[cfg(test)]
                    trace_compact_widget(ui,"header:sound",&sound_response);
                    if sound_response.changed() {
                        self.shared.sound_on.store(sound, Ordering::Relaxed);
                    }
                }
                if self.shared.building.load(Ordering::Relaxed) {
                    ui.label("Rebuilding…");
                }
                let diagnostics_response=ui.menu_button("Diagnostics", |ui| {
                    ui.label(format!(
                        "Render load: {:.0}%",
                        self.shared.load.load() * 100.0
                    ));
                    ui.label(format!("Awake string simulations: {}", self.snap.awake));
                    ui.label(format!(
                        "Render over-budget count: {}",
                        self.shared.over_budget.load(Ordering::Relaxed)
                    ));
                    ui.label(format!(
                        "UI: {:.2} ms · {:.0} Hz",
                        self.ui_cost,
                        self.shared.sample_rate.load()
                    ));
                    if let Some((viewport, content, offset)) = self.scroll_debug {
                        ui.label(format!("Controls viewport: {viewport:?}"));
                        ui.label(format!("Content: {content:?} · offset: {offset:?}"));
                    }
                    ui.label(format!(
                        "Last wheel (logical points, position): {:?}",
                        self.last_wheel
                    ));
                });
                #[cfg(test)]
                trace_compact_widget(ui,"header:diagnostics",&diagnostics_response.response);
            });
        });
    }
    fn motion_legend(&self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Amber: up · Blue: down").small().color(AMBER));
            ui.label(
                RichText::new(format!("Auto-scaled: ± {:.2} μm", self.board_scale * 1e6))
                    .small()
                    .color(MUTED),
            );
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
        let h = (r.height() - 60.0).min((r.width() - 210.0) / aspect);
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
        for (index, bridge) in v.bridges.iter().enumerate() {
            p.add(Shape::line(
                bridge.iter().map(|q| to(*q)).collect(),
                Stroke::new(2.0, Color32::from_gray(183)),
            ));
            if let Some(mid) = bridge.get(bridge.len() / 2) {
                let point = to(*mid);
                let x = if index == 0 {
                    br.left() - 12.0
                } else {
                    br.right() + 12.0
                };
                p.line_segment([point, pos2(x, point.y)], Stroke::new(0.7, MUTED));
                p.text(
                    pos2(x, point.y),
                    if index == 0 {
                        Align2::RIGHT_CENTER
                    } else {
                        Align2::LEFT_CENTER
                    },
                    if index == 0 {
                        "Bass bridge"
                    } else {
                        "Treble bridge"
                    },
                    FontId::proportional(11.0),
                    Color32::from_gray(195),
                );
            }
        }
        p.text(
            r.left_top() + vec2(10.0, 8.0),
            Align2::LEFT_TOP,
            "Simulated panel motion · bridges in white; string ends glow",
            FontId::proportional(11.0),
            MUTED,
        );
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
            "Keyboard edge",
            FontId::proportional(9.0),
            MUTED,
        );
    }
    fn strings(&mut self, ui: &mut Ui, size: Vec2) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Individual string motion")
                    .strong()
                    .color(Color32::from_rgb(232, 231, 221)),
            );
            if self.snap.focus_midi >= 21 {
                ui.label(RichText::new(note_name(self.snap.focus_midi)).color(AMBER));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(if self.snap.focus_midi >= 21 {
                        format!(
                            "Felt contact now: {:.1} N",
                            self.snap.hammer_force.iter().sum::<f32>()
                        )
                    } else {
                        "No note yet".into()
                    })
                    .small()
                    .color(MUTED),
                );
            });
        });
        let (r, response) = ui.allocate_exact_size(size, Sense::hover());
        #[cfg(test)]
        trace_left_target(ui,"strings-panel",response.rect);
        response.on_hover_text("Up to three wires make one note. Left to right is string length, not time.");
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
            format!("Height magnified: ± {:.3} mm", self.string_scale * 1000.0),
            FontId::monospace(10.0),
            MUTED,
        );
        p.text(
            r.left_bottom() + vec2(10.0, -18.0),
            Align2::LEFT_BOTTOM,
            "Fixed string end",
            FontId::proportional(10.0),
            MUTED,
        );
        p.text(
            r.right_bottom() + vec2(-10.0, -18.0),
            Align2::RIGHT_BOTTOM,
            "Bridge end",
            FontId::proportional(10.0),
            MUTED,
        );
        if self.snap.focus_midi < 21 {
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "Play the on-screen keyboard to see actual string motion.",
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }
    fn controls(&mut self, ui: &mut Ui, setter: &ParamSetter) {
        // add_sized alone does not override egui Slider's preferred rail width.
        // This child UI owns the parameter-column style; header Volume stays unchanged.
        let rail_width = ui.available_width();
        ui.spacing_mut().slider_width = rail_width;
        let tab = self.tab;
        let room_switch =
            |i: usize| tab == 0 && PARAMS[i].kind == piano_engine::params::Kind::Switch;
        let indices = (0..PARAMS.len())
            .filter(|&i| PARAMS[i].tab == tab && room_switch(i))
            .chain((0..PARAMS.len()).filter(|&i| PARAMS[i].tab == tab && !room_switch(i)));
        for index in indices {
            let p = &PARAMS[index];
            let setting = &self.params.settings[index];
            let (label, why) = guide::copy(p.name);
            let help = if p.help.is_empty() {
                why.into()
            } else {
                format!("{why}\n{}", p.help)
            };
            match setting {
                Setting::Number(param) => {
                    ui.horizontal(|ui| {
                        ui.label(label).on_hover_text(&help);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if default_reset(
                                ui,
                                setter,
                                param,
                                guide::value(p, param.default_plain_value() as f64),
                            ) {
                                self.active_param = index;
                                self.preset = "Custom settings".into();
                            }
                            ui.label(
                                RichText::new(guide::value(p, param.value() as f64))
                                    .monospace()
                                    .color(AMBER),
                            );
                        });
                    });
                    let mut value = param.value();
                    let response = ui
                        .add_sized(
                            vec2(ui.available_width(), 18.0),
                            Slider::new(&mut value, p.lo as f32..=p.hi as f32)
                                .logarithmic(p.log)
                                .show_value(false),
                        )
                        .on_hover_text(&help);
                    #[cfg(test)]
                    trace_control(ui, "control", param, &response);
                    if response.hovered() || response.dragged() {
                        self.active_param = index;
                    }
                    if response.drag_started() {
                        setter.begin_set_parameter(param);
                    }
                    if response.changed() {
                        if !response.dragged() {
                            setter.begin_set_parameter(param);
                        }
                        setter.set_parameter(param, value);
                        self.preset = "Custom settings".into();
                        if !response.dragged() {
                            setter.end_set_parameter(param);
                        }
                    }
                    if response.drag_stopped() {
                        setter.end_set_parameter(param);
                    }
                }
                Setting::Switch(param) => {
                    ui.horizontal(|ui| {
                        let mut value = param.value();
                        let response = ui.checkbox(&mut value, label).on_hover_text(&help);
                        #[cfg(test)]
                        trace_control(ui, "control", param, &response);
                        if response.hovered() || response.changed() {
                            self.active_param = index;
                        }
                        if response.changed() {
                            set_switch(setter, param, value);
                            self.preset = "Custom settings".into();
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let default = param.default_plain_value() as u8 as f64;
                            if default_reset(ui, setter, param, guide::value(p, default)) {
                                self.active_param = index;
                                self.preset = "Custom settings".into();
                            }
                        });
                    });
                }
            }
            ui.add_space(4.0);
        }
    }
    fn room(&mut self, ui: &mut Ui, setter: &ParamSetter, height: f32) {
        let mut o = self.params.options();
        let (r, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        #[cfg(test)]
        trace_left_target(ui,"room-panel",response.rect);
        response.on_hover_text("Room plan: drag the piano or listener. The ring is the listener handle; the two dots are virtual pickup points. Paths are illustrative, not a measured sound field.");
        let p = ui.painter_at(r);
        p.rect_filled(r, 6.0, PANEL);
        let area = Rect::from_min_max(r.min + vec2(12.0, 10.0), r.max - vec2(12.0, 20.0));
        let scale = (area.width() / o.room_lx as f32).min(area.height() / o.room_ly as f32);
        let room = Rect::from_center_size(
            area.center(),
            vec2(o.room_lx as f32 * scale, o.room_ly as f32 * scale),
        );
        let to = |x: f64, y: f64| {
            pos2(
                room.left() + x as f32 * scale,
                room.bottom() - y as f32 * scale,
            )
        };
        p.rect_stroke(
            room,
            0.0,
            Stroke::new(
                1.0,
                if o.room_enabled {
                    MUTED
                } else {
                    Color32::from_gray(62)
                },
            ),
            StrokeKind::Inside,
        );
        let listener = to(o.listener_x, o.listener_y);
        let piano = to(o.piano_x + 0.7, o.piano_y + 0.7);
        let points: Vec<_> = self
            .preview_design
            .board
            .outline
            .iter()
            .map(|q| to(o.piano_x + q.x, o.piano_y + q.y))
            .collect();
        p.add(Shape::convex_polygon(
            points,
            AMBER.gamma_multiply(0.45),
            Stroke::new(1.0, AMBER),
        ));
        p.circle_stroke(listener, 5.5, Stroke::new(1.3, Color32::LIGHT_BLUE));
        for ear in &self.preview_design.ears {
            p.circle_filled(
                to(o.piano_x + ear.x, o.piano_y + ear.y),
                2.5,
                Color32::LIGHT_BLUE,
            );
        }
        p.line_segment(
            [listener, piano],
            Stroke::new(1.0, Color32::LIGHT_BLUE.gamma_multiply(0.65)),
        );
        if o.room_enabled {
            // Example top-down wall path, not the engine's full radiation/pressure solution.
            let sx = o.piano_x + 0.7;
            let sy = o.piano_y + 0.7;
            let image_y = 2.0 * o.room_ly - sy;
            let t = (o.room_ly - image_y) / (o.listener_y - image_y).min(-0.001);
            let wall = to(sx + (o.listener_x - sx) * t, o.room_ly);
            p.add(Shape::line(
                vec![piano, wall, listener],
                Stroke::new(1.2, AMBER.gamma_multiply((1.0 - o.room_absorption) as f32)),
            ));
        }
        p.text(
            r.left_bottom() + vec2(3.0, -1.0),
            Align2::LEFT_BOTTOM,
            format!("{:.1} × {:.1} m", o.room_lx, o.room_ly),
            FontId::proportional(10.0),
            MUTED,
        );
        p.text(
            piano + vec2(-2.0, 15.0),
            Align2::CENTER_TOP,
            "Piano",
            FontId::proportional(10.0),
            AMBER,
        );
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
                let field = match (is_listener, dx.abs() >= dy.abs()) {
                    (true, true) => "Listener position, along the keyboard",
                    (true, false) => "Listener position, along the strings",
                    (false, true) => "Piano position, along the keyboard",
                    _ => "Piano position, along the strings",
                };
                self.active_param = PARAMS
                    .iter()
                    .position(|p| p.name == field)
                    .unwrap_or(self.active_param);
                self.preset = "Custom settings".into();
            }
        }
        ui.add_space(4.0);
    }
    fn keyboard(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (text, event, on) in [
                (
                    format!("Sustain {}%",(self.shared.sustain.load(Ordering::Relaxed) as f32/127.0*100.0).round() as u32),
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
                    "Soft pedal".to_owned(),
                    Input::Soft(!self.shared.soft.load(Ordering::Relaxed), FROM_UI),
                    self.shared.soft.load(Ordering::Relaxed),
                ),
                (
                    "Sostenuto".to_owned(),
                    Input::Sostenuto(!self.shared.sostenuto.load(Ordering::Relaxed), FROM_UI),
                    self.shared.sostenuto.load(Ordering::Relaxed),
                ),
            ] {
                let help = match event {
                    Input::Sustain(..) => "Accepts intermediate CC64 values when the controller sends half-pedal MIDI. Space is full on while held; this button toggles on/off. The percentage is the combined requested pedal position.",
                    Input::Soft(..) => "Una corda: the hammers miss one wire where available and contact softer felt. Hold comma on the computer keyboard.",
                    _ => "Sostenuto holds only notes that were down when this pedal was pressed; later notes still release normally. Hold period on the computer keyboard.",
                };
                let pedal_response=ui.selectable_label(on, text).on_hover_text(help);
                #[cfg(test)]
                trace_compact_widget(ui,match event {
                    Input::Sustain(..)=>"footer:sustain",
                    Input::Soft(..)=>"footer:soft",
                    _=>"footer:sostenuto",
                },&pedal_response);
                #[cfg(test)]
                {
                    let key=match event {
                        Input::Sustain(..)=>"editor:pedal-sustain",
                        Input::Soft(..)=>"editor:pedal-soft",
                        _=>"editor:pedal-sostenuto",
                    };
                    ui.ctx().data_mut(|data|data.insert_temp(Id::new(key),pedal_response.rect));
                }
                if pedal_response.clicked() {
                    self.shared.send(event);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center),|ui| {
                let settings_response=ui.label(RichText::new(format!("Octave {} · Velocity {}",self.octave,self.velocity)).small().color(MUTED))
                    .on_hover_text("Computer-keyboard settings: Z/X changes octave; C/V changes velocity.");
                #[cfg(test)]
                trace_compact_widget(ui,"footer:settings",&settings_response);
            });
        });
        let (r, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 90.0), Sense::click_and_drag());
        #[cfg(test)]
        trace_compact_widget(ui,"footer:keyboard",&response);
        #[cfg(test)]
        ui.ctx().data_mut(|data| data.insert_temp(Id::new("editor:keyboard"), r));
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
                        Key::Space if pressed || self.key_sustain_down => {
                            self.key_sustain_down = pressed;
                            self.shared.send(Input::Sustain(if pressed { 1.0 } else { 0.0 }, FROM_UI));
                        }
                        Key::Comma if pressed || self.key_soft_down => {
                            self.key_soft_down = pressed;
                            self.shared.send(Input::Soft(pressed, FROM_UI));
                        }
                        Key::Period if pressed || self.key_sostenuto_down => {
                            self.key_sostenuto_down = pressed;
                            self.shared.send(Input::Sostenuto(pressed, FROM_UI));
                        }
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
        if std::mem::take(&mut self.key_sustain_down) {
            self.shared.send(Input::Sustain(0.0,FROM_UI));
        }
        if std::mem::take(&mut self.key_soft_down) {
            self.shared.send(Input::Soft(false,FROM_UI));
        }
        if std::mem::take(&mut self.key_sostenuto_down) {
            self.shared.send(Input::Sostenuto(false,FROM_UI));
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
        self.params.capture_setup()
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

#[cfg(test)]
mod control_tests {
    use super::*;

    #[test]
    fn actual_controls_scroll_container_accepts_point_and_line_wheel() {
        for unit in [MouseWheelUnit::Point, MouseWheelUnit::Line] {
            let ctx = Context::default();
            let mut result = None;
            for frame in 0..8 {
                let mut input = RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 860.0))),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                };
                input.events.push(Event::PointerMoved(pos2(100.0, 100.0)));
                if frame == 1 {
                    let amount = if unit == MouseWheelUnit::Point {
                        120.0
                    } else {
                        3.0
                    };
                    input.events.push(Event::MouseWheel {
                        unit,
                        delta: vec2(0.0, -amount),
                        modifiers: Modifiers::default(),
                    });
                }
                let _ = ctx.run(input, |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        ui.set_max_width(440.0);
                        // Exact app container, no mock host, copied parameter UI,
                        // native window or audio device. This is not native-wheel proof.
                        let scroll = controls_scroll(ui, 0, 240.0, |ui| {
                            ui.allocate_exact_size(vec2(400.0, 1200.0), Sense::hover());
                        });
                        result =
                            Some((scroll.inner_rect, scroll.content_size, scroll.state.offset));
                    });
                });
            }
            let (viewport, content, offset) = result.unwrap();
            assert!(viewport.height() <= 240.0);
            assert!(viewport.contains(pos2(100.0, 100.0)));
            assert!(content.y > viewport.height());
            assert!(
                offset.y > 0.0,
                "logical wheel did not move controls: {unit:?} {offset:?}"
            );
        }
    }

    #[test]
    fn reset_targets_are_the_existing_factory_parameter_defaults() {
        let params = PianoParams::new();
        let options = GrandOptions::default();
        for (meta, param) in PARAMS.iter().zip(&params.settings) {
            match param {
                Setting::Number(p) => {
                    assert_eq!(p.default_plain_value(), (meta.get)(&options) as f32);
                    assert_eq!(p.unmodulated_plain_value(), p.default_plain_value());
                    assert!(!guide::value(meta, p.default_plain_value() as f64).is_empty());
                }
                Setting::Switch(p) => {
                    assert_eq!(p.default_plain_value(), (meta.get)(&options) >= 0.5);
                    assert_eq!(p.unmodulated_plain_value(), p.default_plain_value());
                }
            }
        }
        let setup = params.capture_setup();
        assert_eq!(params.volume.default_plain_value(), setup.volume);
        assert_eq!(params.grid.default_plain_value(), setup.grid);
        assert_eq!(
            grid_label(params.grid.default_plain_value()),
            format!("{}×", crate::setup::grid_scale(setup.grid).unwrap())
        );
    }
}
