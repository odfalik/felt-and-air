//! SOURCE-ONLY actual editor painted-text gates; child of headless_tests.
//! No native/font-raster/host/audio claim; inspect REAL output galleys, not planned sizes.
use super::*;
const TEXT_SIZES: [Vec2; 4] = [vec2(800.0,600.0),vec2(1000.0,650.0),vec2(1280.0,720.0),vec2(1280.0,860.0)];
const TEXT_TOLERANCE: f32 = 1.0;

fn painted_text(output: &FullOutput) -> Vec<(epaint::TextShape,Rect)> {
    fn visit(shape:&Shape,clip:Rect,out:&mut Vec<(epaint::TextShape,Rect)>) {
        match shape {
            Shape::Text(text)=>out.push((text.clone(),clip)),
            Shape::Vec(shapes)=>for shape in shapes { visit(shape,clip,out); },
            _=>{},
        }
    }
    let mut out=Vec::new();
    for clipped in &output.shapes { visit(&clipped.shape,clipped.clip_rect,&mut out); }
    out
}
fn inside(rect:Rect,container:Rect)->bool {
    rect.is_finite() && container.is_finite() && rect.width()>0.0 && rect.height()>0.0
        && container.expand(TEXT_TOLERANCE).contains_rect(rect)
}
fn actual_header(f:&Fixture,name:&str)->Rect {
    let widgets=f.ctx.data(|d|d.get_temp::<Vec<(String,Rect,Rect)>>(Id::new("compact:widgets"))).expect("fresh actual header responses");
    let found:Vec<_>=widgets.iter().filter(|(key,_,_)|key==name).collect();
    assert_eq!(found.len(),1,"one actual header response required: {name}");
    found[0].1
}
fn select_category(f:&mut Fixture,tab:usize) {
    f.state.tab=tab;
    f.state.active_param=PARAMS.iter().position(|p|p.tab==tab).unwrap();
    f.state.frame=u64::MAX; f.settle();
}
fn check_header_text(f:&Fixture,output:&FullOutput) {
    let texts=painted_text(output);
    let mut labels=vec![("header:diagnostics","Diagnostics")];
    if STANDALONE.load(Ordering::Relaxed) {
        labels.push(("header:sound",if f.state.shared.sound_on.load(Ordering::Relaxed) {"Sound on"} else {"Sound off"}));
    }
    for (key,label) in labels {
        let response=actual_header(f,key);
        let matching:Vec<_>=texts.iter().filter(|(text,_)|text.galley.job.text==label && text.visual_bounding_rect().intersects(response)).collect();
        assert_eq!(matching.len(),1,"actual header painted text missing/ambiguous: {label}");
        let (text,clip)=matching[0];
        assert!(text.angle==0.0 && !text.galley.elided && text.galley.rows.len()==1,
            "actual header text is not a single unelided row: label={label} rows={} api={:?} size={:?}",text.galley.rows.len(),f.host.api,f.size);
        assert_eq!(text.galley.rows.iter().map(|row|row.glyphs.len()).sum::<usize>(),label.chars().count());
        let visual=text.visual_bounding_rect();
        assert!(inside(visual,response) && inside(visual,clip.intersect(f.screen())),
            "actual header glyphs leave response/clip: label={label} glyphs={visual:?} response={response:?}");
    }
}

#[test]
fn actual_header_sound_and_diagnostics_paint_natural_single_rows() {
    both_editor_modes(|| {
        for size in TEXT_SIZES {
            let mut f=Fixture::new(size);
            for tab in 0..guide::TABS.len() {
                select_category(&mut f,tab);
                let toggles=if STANDALONE.load(Ordering::Relaxed) {2} else {1};
                for _ in 0..toggles {
                    let output=f.frame(Vec::new()); check_header_text(&f,&output);
                    if STANDALONE.load(Ordering::Relaxed) {
                        // Real Sound button toggles the actual UI atomic; no audio/engine.
                        let response=actual_header(&f,"header:sound");
                        let before=f.state.shared.sound_on.load(Ordering::Relaxed);
                        f.click_at(response.center()); f.settle();
                        assert_eq!(f.state.shared.sound_on.load(Ordering::Relaxed),!before,
                            "actual Sound button click did not toggle the UI state");
                    }
                }
                f.host.assert_no_edits(); assert!(f.musical_inputs().is_empty());
            }
        }
    });
}

#[test]
fn header_height_is_invariant_during_background_rebuild_preventing_relayout_jitter() {
    both_editor_modes(|| {
        for size in TEXT_SIZES {
            let mut f = Fixture::new(size);
            select_category(&mut f, 3);
            let before_sidebar = f.sidebar().0;
            let (before_board, _) = left_target(&f, "board-panel");
            f.state.shared.building.store(true, Ordering::Relaxed);
            f.settle();
            let during_sidebar = f.sidebar().0;
            let (during_board, _) = left_target(&f, "board-panel");
            f.state.shared.building.store(false, Ordering::Relaxed);
            f.settle();
            let after_sidebar = f.sidebar().0;
            assert!((before_sidebar.top() - during_sidebar.top()).abs() <= 0.5,
                "header grew during rebuild: before={before_sidebar:?} during={during_sidebar:?}");
            assert!((before_sidebar.height() - during_sidebar.height()).abs() <= 0.5);
            assert!((before_board.top() - during_board.top()).abs() <= 0.5);
            assert!((before_sidebar.top() - after_sidebar.top()).abs() <= 0.5);
        }
    });
}

fn left_target(f:&Fixture,name:&str)->(Rect,Rect) {
    f.ctx.data(|d|d.get_temp(Id::new(format!("left:{name}")))).expect("fresh actual left response")
}
fn seen_text(texts:&[(epaint::TextShape,Rect)],expected:&str,panel:Rect,visible:Rect)->bool {
    let found:Vec<_>=texts.iter().filter(|(text,_)|
        text.galley.job.text==expected && text.visual_bounding_rect().intersects(panel)).collect();
    assert!(found.len()<=1,"ambiguous actual left text: {expected}");
    let Some((text,clip))=found.first().copied() else { return false; };
    let available=panel.intersect(visible).intersect(*clip);
    !text.galley.elided && text.angle==0.0
        && text.galley.rows.iter().flat_map(|row|row.glyphs.iter().map(|glyph|glyph.chr))
            .filter(|c|!c.is_whitespace()).eq(expected.chars().filter(|c|!c.is_whitespace()))
        && inside(text.visual_bounding_rect(),available)
        && inside(text.galley.rect.translate(text.pos.to_vec2()),available)
        && text.galley.rows.iter().all(|row|inside(row.rect.translate(text.pos.to_vec2()),available))
}

#[test]
fn unified_room_strings_and_soundboard_canvases_render_within_viewport() {
    both_editor_modes(|| {
        for size in TEXT_SIZES {
            let mut f = Fixture::new(size);
            // Tab 0: Unified Room Floor Plan + Side Elevation
            select_category(&mut f, 0);
            let out = f.frame(Vec::new());
            let (panel, clip) = left_target(&f, "room-panel");
            let vis = clip.intersect(f.screen());
            assert!(inside(panel, vis), "Room canvas overflows viewport: {size:?}");
            let texts = painted_text(&out);
            assert!(seen_text(&texts, "Piano", panel, vis));
            assert!(seen_text(&texts, "Listener", panel, vis));
            assert!(seen_text(&texts, "SIDE ELEVATION · FLOOR, CEILING & LID", panel, vis));

            // Tab 2: Unified Multi-Lane Strings
            select_category(&mut f, 2);
            let out = f.frame(Vec::new());
            let (panel, clip) = left_target(&f, "strings-panel");
            let vis = clip.intersect(f.screen());
            assert!(inside(panel, vis), "Strings canvas overflows viewport: {size:?}");
            let texts = painted_text(&out);
            assert!(seen_text(&texts, "Agraffe", panel, vis));
            assert!(seen_text(&texts, "Bridge", panel, vis));

            // Tab 3: Unified Soundboard
            select_category(&mut f, 3);
            let out = f.frame(Vec::new());
            let (panel, clip) = left_target(&f, "board-panel");
            let vis = clip.intersect(f.screen());
            assert!(inside(panel, vis), "Soundboard canvas overflows viewport: {size:?}");
        }
    });
}

#[test]
fn actual_state_single_key_accepted_build_publication_paints_rotational_action() {
    use crate::editor::illustration::action::PaintTrace;
    fn paint_trace(f: &Fixture) -> PaintTrace {
        f.ctx.data(|d| d.get_temp(Id::new("action:paint"))).expect("fresh action paint trace")
    }
    fn hero_felt_vertices(output: &FullOutput, paint: &PaintTrace) -> Vec<Pos2> {
        fn visit(shape: &Shape, paint: &PaintTrace, out: &mut Vec<Vec<Pos2>>) {
            match shape {
                Shape::Vec(shapes) => for s in shapes { visit(s, paint, out); },
                Shape::Mesh(mesh) if mesh.vertices.len() > 20 && mesh.vertices.iter().all(|v| v.color == paint.felt_color)
                    && shape.visual_bounding_rect().intersects(paint.panel) => {
                    out.push(mesh.vertices.iter().map(|v| v.pos).collect());
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for c in &output.shapes { visit(&c.shape, paint, &mut found); }
        assert_eq!(found.len(), 1, "one hero felt mesh expected");
        found.pop().unwrap()
    }
    let _mode = EditorModeGuard::new(true);
    for size in [vec2(1280.0, 860.0), vec2(800.0, 600.0)] {
        let mut p = crate::PhysicalPiano::with_demos(|| Arc::new(Vec::new()));
        assert!(p.initialize_engine(48000.0, 512));
        let mut f = Fixture::from_shared(size, p.params.clone(), p.shared.clone(), p.demos.clone());
        f.settle();
        let category = actual_header(&f, "category:1");
        f.click_at(category.center());
        f.settle();
        let initial = f.frame(Vec::new());
        let paint0 = paint_trace(&f);
        assert_eq!(paint0.status, "Neutral");
        let neutral_felt = hero_felt_vertices(&initial, &paint0);

        // Press and release C4 through real editor input path
        f.click_at(paint0.panel.left_top() + vec2(5.0, 5.0));
        f.settle();
        let key = |pressed| Event::Key { key: Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::default() };
        f.frame(vec![key(true)]);
        p.drain_editor_inputs();
        f.frame(vec![key(false)]);
        p.drain_editor_inputs();

        let mut left = [0.0; 512];
        let mut right = [0.0; 512];
        let mut until_publish = 2048usize;
        let mut live_count = 0usize;
        let mut saw_rotation = false;
        for _ in 0..14 {
            let mut rem = 800usize;
            while rem > 0 {
                let n = rem.min(512).min(until_publish);
                p.built.as_mut().unwrap().piano.process(&mut left[..n], &mut right[..n]);
                rem -= n;
                until_publish -= n;
                if until_publish == 0 {
                    assert!(p.try_publish_snapshot());
                    until_publish = 2048;
                }
            }
            let out = f.frame(Vec::new());
            let paint = paint_trace(&f);
            let felt = hero_felt_vertices(&out, &paint);
            if paint.status == "Live" {
                live_count += 1;
                // Rotational arc moves BOTH x and y relative to neutral pose!
                let dx_max = felt.iter().zip(&neutral_felt).map(|(a, b)| (a.x - b.x).abs()).fold(0.0f32, f32::max);
                let dy_max = felt.iter().zip(&neutral_felt).map(|(a, b)| (a.y - b.y).abs()).fold(0.0f32, f32::max);
                if dx_max > 0.2 && dy_max > 1.0 {
                    saw_rotation = true;
                }
            }
        }
        assert!(live_count > 1 && saw_rotation, "hero hammer must rotate along circular arc during real strike");
    }
}

fn selector_catalog(size:Vec2)->(Fixture,[String;2]) {
    let (mut f,names)=selector_catalog_unsettled(size);f.settle();(f,names)
}
fn selector_catalog_unsettled(size:Vec2)->(Fixture,[String;2]) {
    let names=[format!("Recital {} · First","W".repeat(128)),format!("Recital {} · Second","W".repeat(128))];
    let params=Arc::new(PianoParams::new());
    let setups=names.iter().map(|name|(name.clone(),params.capture_setup())).collect();
    // Existing minimal SMF fixture, parsed in memory; no user/catalog files or playback.
    let mut events=piano_engine::midi::parse_midi(b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk\0\0\0\x0d\0\x90\x3c\x40\x83\x60\x80\x3c\x40\0\xff\x2f\0");
    assert!(!events.is_empty());
    piano_engine::midi::lead_notes_by_flight(&mut events,params.options().let_off_mm*1e-3,
        |velocity|piano_engine::midi_velocity_to_hammer_speed(velocity));
    let seconds=events.last().unwrap().seconds;
    let demos=Arc::new(names.iter().map(|title|Demo {title:title.clone(),
        note:"In-memory parsed MIDI fixture; never played".into(),events:events.clone(),seconds}).collect());
    (Fixture::catalog(size,params,demos,setups),names)
}
fn selector_trace(f:&Fixture,key:&str)->Option<(Id,Rect,Rect)> {
    let responses=f.ctx.data(|d|d.get_temp::<Vec<SelectorTrace>>(Id::new("selector:responses"))).unwrap_or_default();
    let found:Vec<_>=responses.iter().filter(|(name,_,_,_)|name==key).collect();
    assert!(found.len()<=1,"one fresh actual selector response: {key}");
    found.first().map(|(_,id,rect,clip)|(*id,*rect,*clip))
}
fn popup_trace(f:&Fixture,kind:&str)->(Rect,Rect) {
    f.ctx.data(|d|d.get_temp(Id::new(format!("selector:popup:{kind}")))).expect("fresh real open popup content/clip")
}
fn open_selector(f:&mut Fixture,kind:&str)->Id {
    let (id,rect,clip)=selector_trace(f,kind).expect("actual closed selector");
    let visible=rect.intersect(clip).intersect(f.screen());
    assert!(visible.width()>8.0 && visible.height()>8.0,"real selector click target");
    f.click_at(visible.center()); f.settle();
    assert!(ComboBox::is_open(&f.ctx,id),"real selector click did not open actual popup");
    id
}
fn item_key(kind:&str,index:usize,names:&[String;2])->String {
    if kind=="preset" {format!("setup:{}",names[index])} else {format!("demo-item:{index}")}
}
fn select_catalog_item(f:&mut Fixture,kind:&str,index:usize,names:&[String;2]) {
    let id=open_selector(f,kind); let key=item_key(kind,index,names);
    for step in 0..=64 {
        if let Some((response_id,rect,clip))=selector_trace(f,&key) {
            let visible=rect.intersect(clip).intersect(f.screen());
            // A real visible portion is sufficient to click even on the NEG
            // unbounded row; the closed-width gate must reach its own assertion.
            if visible.width()>8.0 && visible.height()>8.0 {
                assert_eq!(f.ctx.read_response(response_id).expect("real item response").rect,rect);
                f.click_at(visible.center()); f.settle();
                assert!(!ComboBox::is_open(&f.ctx,id),"real item selection did not close popup");
                if kind=="preset" {assert_eq!(f.state.preset,names[index]);}
                else {assert_eq!(f.state.demo,index); assert_eq!(f.state.demos[index].title,names[index]);}
                assert!(names.iter().all(|name|f.state.setups.contains_key(name)),"stored full setup keys changed");
                assert_eq!(f.state.shared.demo_playing.load(Ordering::Relaxed),-1,"name gate must never start playback");
                assert!(f.musical_inputs().is_empty(),"name selection emitted musical input");
                // RecordingHost validates real callbacks; it does NOT apply host values.
                return;
            }
        }
        assert!(step<64,"real popup item not reachable for selection");
        let (content,clip)=popup_trace(f,kind);
        let visible=content.intersect(clip).intersect(f.screen());
        assert!(visible.width()>0.0 && visible.height()>0.0);
        let direction=selector_trace(f,&key).map_or(1.0,|(_,rect,_)|
            if rect.center().y<visible.center().y {-1.0} else {1.0});
        f.wheel(visible.center(),MouseWheelUnit::Point,24.0*direction);
        assert!(ComboBox::is_open(&f.ctx,id),"wheel unexpectedly closed actual popup");
    }
}
#[path = "selector_export_tests.rs"]
mod selector_export_tests;

#[test]
fn actual_long_closed_selectors_are_bounded_and_full_names_remain_discoverable() {
    both_editor_modes(|| {
        for size in TEXT_SIZES.into_iter().filter(|size|size.x<=1000.0) {
            for kind in ["preset","demo"] {
                let (mut f,names)=selector_catalog(size);
                for index in [1,0] {
                    select_catalog_item(&mut f,kind,index,&names);
                    let output=f.frame(Vec::new());
                    let (_,rect,clip)=selector_trace(&f,kind).expect("fresh selected closed response");
                    let width=if kind=="preset" {235.0} else {240.0};
                    assert!((rect.width()-width).abs()<=TEXT_TOLERANCE && inside(rect,clip.intersect(f.screen())),
                        "actual closed long selector exceeds prescribed bound: kind={kind} size={size:?} rect={rect:?}");
                    let texts=painted_text(&output);
                    let found:Vec<_>=texts.iter().filter(|(text,_)|text.galley.job.text==names[index]
                        && text.visual_bounding_rect().intersects(rect)).collect();
                    assert_eq!(found.len(),1,"real selected name galley");
                    let (text,text_clip)=found[0]; let available=rect.intersect(clip).intersect(*text_clip).intersect(f.screen());
                    // Deliberate selected-display elision only; old seen_text is unchanged.
                    assert!(text.galley.elided && text.galley.rows.len()==1 && text.angle==0.0);
                    assert!(inside(text.visual_bounding_rect(),available)
                        && inside(text.galley.rect.translate(text.pos.to_vec2()),available)
                        && text.galley.rows.iter().all(|row|inside(row.rect.translate(text.pos.to_vec2()),available)));
                    let mut tooltip_seen=false;
                    f.frame(vec![Event::PointerMoved(rect.center())]);
                    for _ in 0..48 {
                        let output=f.frame(Vec::new());
                        let tooltip:Vec<_>=painted_text(&output).into_iter()
                            .filter(|(text,_)|text.galley.job.text==names[index] && !text.galley.elided
                            && !text.visual_bounding_rect().intersects(rect)).collect();
                        tooltip_seen|=seen_text(&tooltip,&names[index],f.screen(),f.screen());
                    }
                    assert!(tooltip_seen,"actual full-name hover tooltip missing/clipped: kind={kind} size={size:?}");
                    assert_eq!(f.state.shared.demo_playing.load(Ordering::Relaxed),-1);
                    assert!(f.musical_inputs().is_empty());
                }
            }
        }
    });
}
#[test]
fn actual_long_popup_rows_wrap_all_text_and_keep_selection_identity() {
    both_editor_modes(|| {
        for size in TEXT_SIZES.into_iter().filter(|size|size.x<=1000.0) {
            for kind in ["preset","demo"] {
                let (mut f,names)=selector_catalog(size);
                let id=open_selector(&mut f,kind); let mut seen=[false;2];
                for step in 0..=64 {
                    let output=f.frame(Vec::new()); let texts=painted_text(&output);
                    let (content,clip)=popup_trace(&f,kind); let visible=content.intersect(clip).intersect(f.screen());
                    let width=if kind=="preset" {235.0} else {240.0};
                    assert!(content.width()<=width+TEXT_TOLERANCE
                        && content.left()>=visible.left()-TEXT_TOLERANCE && content.right()<=visible.right()+TEXT_TOLERANCE,
                        "actual long popup content not horizontally bounded: kind={kind} size={size:?} content={content:?} clip={visible:?}");
                    for i in 0..2 {
                        let (_,rect,item_clip)=selector_trace(&f,&item_key(kind,i,&names)).expect("fresh real long row");
                        // These known long fixtures must fit vertically per row.
                        // This does not require arbitrarily huge names/whole menus to fit at once.
                        assert!(rect.height()<=visible.height()+TEXT_TOLERANCE,"known fixture row too tall for single-row gate");
                        let expected=if kind=="preset" {format!("Saved · {}",names[i])} else {names[i].clone()};
                        let row_scope=rect.intersect(visible).intersect(item_clip);
                        let candidates:Vec<_>=texts.iter().filter(|(text,shape_clip)|
                            text.visual_bounding_rect().intersects(row_scope.intersect(*shape_clip))).cloned().collect();
                        seen[i]|=seen_text(&candidates,&expected,rect,visible.intersect(item_clip));
                    }
                    if seen.iter().all(|seen|*seen) {break;}
                    assert!(step<64,"actual full long popup rows not reached");
                    assert!(visible.width()>0.0 && visible.height()>0.0);
                    f.wheel(visible.center(),MouseWheelUnit::Point,24.0);
                    assert!(ComboBox::is_open(&f.ctx,id));
                }
                // Close through the actual header, then use the same real choice path.
                let (_,header,clip)=selector_trace(&f,kind).expect("real open selector header");
                f.click_at(header.intersect(clip).intersect(f.screen()).center()); f.settle();
                assert!(!ComboBox::is_open(&f.ctx,id));
                for index in [1,0] {select_catalog_item(&mut f,kind,index,&names);}
            }
        }
    });
}
