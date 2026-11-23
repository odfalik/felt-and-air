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
fn actual_soundboard_caption_paints_all_text_inside_its_real_panel() {
    both_editor_modes(|| {
        for size in TEXT_SIZES {
            let mut f=Fixture::new(size); select_category(&mut f,3);
            let output=f.frame(Vec::new());
            let (panel,panel_clip)=f.ctx.data(|d|d.get_temp::<(Rect,Rect)>(Id::new("compact:anatomy-panel:3")))
                .expect("fresh actual guide::instrument allocated panel/clip");
            let options=f.state.params.options();
            let expected=format!("{:.1} mm wood • {:.0} mm ribs • {:.0} mm apart",options.board_thickness_mm,options.rib_height_mm,options.rib_spacing_mm);
            let texts=painted_text(&output);
            let matching:Vec<_>=texts.iter().filter(|(text,_)|text.galley.job.text.contains("mm wood") && text.galley.job.text.contains("mm ribs") && text.galley.job.text.contains("mm apart")).collect();
            assert_eq!(matching.len(),1,"one actual painted physical-caption galley required");
            let (text,clip)=matching[0];
            assert_eq!(text.galley.job.text,expected,"physical caption text/units changed");
            assert!(!text.galley.elided && text.angle==0.0,"caption truncated/rotated");
            assert!(text.galley.job.sections.iter().all(|section|section.format.font_id==FontId::proportional(11.0)),"caption font must stay11pt proportional");
            // Compare painted semantic characters, independent of soft-wrap whitespace.
            let painted: String=text.galley.rows.iter().flat_map(|row|row.glyphs.iter().map(|glyph|glyph.chr)).filter(|c|!c.is_whitespace()).collect();
            let semantic: String=expected.chars().filter(|c|!c.is_whitespace()).collect();
            assert_eq!(painted,semantic,"wrapped caption lost/reordered physical text or units");
            let available=panel.intersect(panel_clip).intersect(*clip).intersect(f.screen());
            let logical=text.galley.rect.translate(text.pos.to_vec2());
            let visual=text.visual_bounding_rect();
            assert!(inside(logical,available) && inside(visual,available)
                && text.galley.rows.iter().all(|row|inside(row.rect.translate(text.pos.to_vec2()),available)),
                "actual Soundboard caption glyphs/rows leave allocated panel: size={size:?} api={:?} panel={panel:?} logical={logical:?} glyphs={visual:?}",f.host.api);
            if size.x<=1000.0 { assert!(text.galley.rows.len()>1,"compact caption must wrap full preserved text, not clip or shrink it"); }
            f.host.assert_no_edits(); assert!(f.musical_inputs().is_empty());
        }
    });
}

fn left_scroll(f:&Fixture)->(Rect,Vec2,Vec2) {
    f.ctx.data(|d|d.get_temp(Id::new("left:scroll"))).expect("fresh actual left ScrollArea output")
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
// Observe targets at every small forward step: whole expanded cards need not
// fit at once. The reverse traversal independently proves return to the top.
fn walk_left(f:&mut Fixture,mut observe:impl FnMut(&Fixture,&FullOutput)) {
    let rhs=f.sidebar().2;
    let output=f.frame(Vec::new()); observe(f,&output);
    assert!(left_scroll(f).2.y.abs()<=TEXT_TOLERANCE,"fresh fixture starts at actual left top");
    let check=|f:&Fixture| {
        let (viewport,content,offset)=left_scroll(f);
        assert!(viewport.is_finite() && content.is_finite() && offset.is_finite());
        assert!(viewport.width()>0.0 && viewport.height()>0.0 && content.y>=0.0);
        let max=(content.y-viewport.height()).max(0.0);
        assert!(offset.y>=-TEXT_TOLERANCE && offset.y<=max+TEXT_TOLERANCE,"actual left offset outside content");
        assert_eq!(f.sidebar().2,rhs,"left wheel moved right controls");
        f.host.assert_no_edits();
        assert!(f.musical_inputs().is_empty(),"left wheel/collapse emitted musical inputs");
        let visible=viewport.intersect(f.screen());
        assert!(visible.width()>0.0 && visible.height()>0.0,"on-screen actual left wheel target");
        (visible,max,offset.y)
    };
    for direction in [1.0f32,-1.0] {
        for step in 0..=64 {
            let (visible,max,before)=check(f);
            let remaining=if direction>0.0 {max-before} else {before};
            if remaining<=TEXT_TOLERANCE {
                if max<=TEXT_TOLERANCE {
                    // Fitting content must not acquire phantom scroll in either direction.
                    f.wheel(visible.center(),MouseWheelUnit::Point,24.0*direction);
                    let output=f.frame(Vec::new()); let (_,_,after)=check(f);
                    assert!((after-before).abs()<=TEXT_TOLERANCE);
                    if direction>0.0 {observe(f,&output);}
                }
                break;
            }
            assert!(step<64,"bounded actual left wheel traversal did not reach end");
            f.wheel(visible.center(),MouseWheelUnit::Point,24.0*direction);
            let output=f.frame(Vec::new()); let (_,_,after)=check(f);
            assert!((after-before)*direction>0.0,"overflowing actual left viewport did not progress");
            if direction>0.0 {observe(f,&output);}
        }
        let (_,max,offset)=check(f);
        let expected=if direction>0.0 {max} else {0.0};
        assert!((offset-expected).abs()<=TEXT_TOLERANCE,"actual left end/top not reached");
    }
}

#[test]
fn actual_left_room_map_and_expanded_decay_targets_are_wheel_reachable() {
    both_editor_modes(|| {
        for size in TEXT_SIZES {
            let mut f=Fixture::new(size); select_category(&mut f,0);
            let rhs=f.sidebar().2;
            let (header,clip)=left_target(&f,"decay-header");
            let visible=left_scroll(&f).0.intersect(clip).intersect(f.screen());
            assert!(inside(header,visible),"real decay header must be fully reachable for the click");
            f.click_at(header.center());
            for _ in 0..12 {f.frame(Vec::new());}
            assert_eq!(f.sidebar().2,rhs,"real decay click moved right controls");
            assert_eq!(f.ctx.data(|d|d.get_temp::<bool>(Id::new("left:decay-open"))),Some(true),
                "real Decay estimate click did not open its actual body");
            let o=f.state.params.options();
            let map_text=[format!("{:.1} × {:.1} m",o.room_lx,o.room_ly),"Piano".into(),"Listener".into()];
            let decay_text=[format!("Room tail ≈ {:.2} s",guide::room_decay(&o)),
                "Time for echoes to become very quiet (−60 dB)".into(),
                "First 4 s · Sabine estimate, not live audio. Gray: default room.".into()];
            let plot_text=["0 dB","−60 dB","0s","1s","2s","3s","4s"];
            let mut seen=[false;15];
            walk_left(&mut f,|f,output| {
                let texts=painted_text(output); let viewport=left_scroll(f).0.intersect(f.screen());
                let (map,clip)=left_target(f,"room-panel");
                seen[0]|=inside(map,viewport.intersect(clip));
                for (i,label) in map_text.iter().enumerate() {seen[1+i]|=seen_text(&texts,label,map,viewport.intersect(clip));}
                let (body,clip)=left_target(f,"decay-body");
                for (i,label) in decay_text.iter().enumerate() {seen[4+i]|=seen_text(&texts,label,body,viewport.intersect(clip));}
                let (plot,clip)=left_target(f,"decay-plot");
                seen[7]|=inside(plot,viewport.intersect(clip));
                for (i,label) in plot_text.iter().enumerate() {seen[8+i]|=seen_text(&texts,label,plot,viewport.intersect(clip));}
            });
            assert!(seen.iter().all(|seen|*seen),
                "actual Room map/T60 targets not fully seen during wheel traversal: size={size:?} api={:?} targets={seen:?}",f.host.api);
        }
    });
}

#[test]
fn actual_left_strings_lower_panel_and_bottom_text_are_wheel_reachable() {
    both_editor_modes(|| {
        for size in TEXT_SIZES {
            let mut f=Fixture::new(size); select_category(&mut f,2);
            assert!(f.state.snap.focus_midi<21,"canonical empty fixture, not fabricated live curves");
            let mut seen=[false;5];
            walk_left(&mut f,|f,output| {
                let texts=painted_text(output); let (panel,clip)=left_target(f,"strings-panel");
                let visible=left_scroll(f).0.intersect(clip).intersect(f.screen());
                seen[0]|=inside(panel,visible);
                let labels=["Fixed string end".into(),"Bridge end".into(),
                    format!("Height magnified: ± {:.3} mm",f.state.string_scale*1000.0),
                    "Play the on-screen keyboard to see actual string motion.".into()];
                for (i,label) in labels.iter().enumerate() {seen[1+i]|=seen_text(&texts,label,panel,visible);}
            });
            assert!(seen.iter().all(|seen|*seen),
                "actual Strings lower targets not fully seen during wheel traversal: size={size:?} api={:?} targets={seen:?}",f.host.api);
        }
    });
}

#[test]
fn actual_state_single_key_accepted_build_publication_paints_measured_action() {
    use crate::editor::illustration::action::PaintTrace;
    use piano_engine::piano::{ActionPhase,ActionPose,ActionStroke};
    #[derive(Clone,PartialEq)]
    struct Geometry {felt:Vec<Pos2>,references:Vec<[Pos2;2]>}
    fn geometry(output:&FullOutput,paint:&PaintTrace,screen:Rect)->(Geometry,bool) {
        fn visit(shape:&Shape,clip:Rect,paint:&PaintTrace,screen:Rect,felt:&mut Vec<Vec<Pos2>>,references:&mut Vec<[Pos2;2]>,cue:&mut bool) {
            let available=paint.panel.intersect(paint.clip).intersect(clip).intersect(screen);
            match shape {
                Shape::Vec(shapes)=>for shape in shapes {visit(shape,clip,paint,screen,felt,references,cue);},
                Shape::Mesh(mesh) if mesh.vertices.len()>3 && mesh.vertices.iter().all(|v|v.color==paint.felt_color)
                    && shape.visual_bounding_rect().intersects(paint.panel)=> {
                    assert!(mesh.indices.iter().all(|&i|(i as usize)<mesh.vertices.len()));
                    assert!(mesh.vertices.iter().all(|v|v.pos.is_finite() && available.expand(TEXT_TOLERANCE).contains(v.pos)),
                        "actual felt mesh vertices leave paint clip");
                    felt.push(mesh.vertices.iter().map(|v|v.pos).collect());
                },
                Shape::LineSegment {points,stroke} if points[0].y==points[1].y
                    && stroke.width==1.0 && stroke.color==paint.reference_color
                    && points.iter().all(|p|available.expand(TEXT_TOLERANCE).contains(*p))=>references.push(*points),
                Shape::LineSegment {points,stroke} if stroke.width==2.0 && stroke.color==paint.cue_color
                    && points.iter().all(|p|available.expand(TEXT_TOLERANCE).contains(*p))=>*cue=true,
                _=>{},
            }
        }
        let mut felt=Vec::new(); let mut references=Vec::new(); let mut cue=false;
        for clipped in &output.shapes {visit(&clipped.shape,clipped.clip_rect,paint,screen,&mut felt,&mut references,&mut cue);}
        assert_eq!(felt.len(),1,"one ACTUAL filled felt mesh in Action paint scope");
        (Geometry {felt:felt.pop().unwrap(),references},cue)
    }
    fn head(meta:&ActionStroke,pose:&ActionPose)->f64 {
        match pose.phase {
            ActionPhase::Flight=>pose.eta_m,
            ActionPhase::Returning|ActionPhase::Checked|ActionPhase::Rest=>{
                assert!(meta.return_origin.valid && meta.return_origin.reference_observed);
                meta.return_origin.origin_m-pose.hy_m
            },
            ActionPhase::Invalid=>panic!("real painted bracket cannot be invalid"),
        }
    }
    fn paint_trace(f:&Fixture)->PaintTrace {
        f.ctx.data(|d|d.get_temp(Id::new("action:paint"))).expect("fresh last-frame actual paint scope")
    }
    let _mode=EditorModeGuard::new(true);
    for size in [vec2(1280.0,860.0),vec2(800.0,600.0)] {
        let mut p=crate::PhysicalPiano::with_demos(||Arc::new(Vec::new()));
        assert!(p.initialize_engine(48000.0,512));
        assert!(p.shared.display.lock().unwrap().view.is_some(),"REAL accepted build BoardView");
        let mut f=Fixture::from_shared(size,p.params.clone(),p.shared.clone(),p.demos.clone());
        f.settle();
        let category=actual_header(&f,"category:1");
        f.click_at(category.center()); f.settle();
        let initial=f.frame(Vec::new()); let paint=paint_trace(&f);
        assert!(paint.status.starts_with("Waiting for simulation samples"));
        let (neutral,_)=geometry(&initial,&paint,f.screen());
        assert!(paint.identity.is_none() && paint.bracket.is_none());
        assert!(painted_text(&initial).iter().all(|(text,_)|!text.galley.job.text.contains("effective let-off")));
        // Real blank click, not a memory/focus override, then actual keyboard events.
        f.click_at(paint.panel.left_top()+vec2(5.0,5.0)); f.settle();
        let key=|pressed|Event::Key {key:Key::A,physical_key:None,pressed,repeat:false,modifiers:Modifiers::default()};
        f.frame(vec![key(true)]);
        assert!(!p.shared.inputs.is_empty(),"actual State.show keydown must enter real queue");
        p.drain_editor_inputs();
        assert!(p.shared.inputs.is_empty());
        assert_eq!(p.shared.focus.load(Ordering::Relaxed),60);
        assert_ne!(p.shared.held[60].load(Ordering::Relaxed)&FROM_UI,0);
        f.frame(vec![key(false)]);
        assert!(!p.shared.inputs.is_empty(),"actual State.show keyup must enter real queue");
        p.drain_editor_inputs();
        assert_eq!(p.shared.held[60].load(Ordering::Relaxed)&FROM_UI,0);
        let mut frames=0usize; let mut until_publish=2048usize;
        let mut left=[0.0;512]; let mut right=[0.0;512];
        let mut live_count=0usize; let mut saw_buffering=false; let mut saw_cue=false;
        let mut previous:Option<(f64,f32,Vec<f64>,Vec<f32>)>=None;
        const REFERENCE_TOLERANCE:f64=0.003;
        let mut translated=false; let mut reference_signal=false; let mut last_live=None;
        for _ in 0..14 {
            let mut remaining=800usize; // 60Hz UI, real physics pieces<=512.
            while remaining>0 {
                let n=remaining.min(512).min(until_publish);
                p.built.as_mut().unwrap().piano.process(&mut left[..n],&mut right[..n]);
                assert!(left[..n].iter().chain(&right[..n]).all(|v|v.is_finite()));
                frames+=n; remaining-=n; until_publish-=n;
                assert!(frames<=12000,"hard real-output-frame bound per layout");
                if until_publish==0 {
                    assert!(p.try_publish_snapshot(),"PRODUCTION accepted/gen/try_lock publication body");
                    until_publish=2048;
                }
            }
            let output=f.frame(Vec::new()); let paint=paint_trace(&f);
            let (observed,cue_geometry)=geometry(&output,&paint,f.screen());
            saw_buffering|=paint.status.starts_with("Buffering measured stroke");
            if paint.status.starts_with("Measured ") {
                live_count+=1;
                let display=p.shared.display.lock().unwrap();
                let published=&display.snap.action;
                let displayed=paint.identity.expect("actual displayed retained stroke identity");
                let meta=published.stroke(displayed.2).expect("EXACT displayed metadata owner, never current-stroke substitution");
                assert_eq!(displayed,(display.generation,meta.midi,meta.stroke_sequence));
                assert_eq!(observed.references.len(),meta.string_count as usize);
                let (lower,upper,alpha,_)=paint.bracket.expect("ACTUAL painted real bracket");
                let a=&published.poses().filter(|r|r.stroke_sequence==meta.stroke_sequence && r.pose.pose_step==lower).last().expect("real owned lower endpoint").pose;
                let b=&published.poses().filter(|r|r.stroke_sequence==meta.stroke_sequence && r.pose.pose_step==upper).last().expect("real owned upper endpoint").pose;
                let expected_head=head(meta,a)+(head(meta,b)-head(meta,a))*alpha;
                let expected_along:Vec<_>=(0..meta.string_count as usize)
                    .map(|i|a.string_along_m[i]+(b.string_along_m[i]-a.string_along_m[i])*alpha).collect();
                let actual_head_y=observed.felt.iter().map(|p|p.y).fold(f32::INFINITY,f32::min);
                let actual_refs:Vec<_>=observed.references.iter().map(|points|points[0].y).collect();
                if let Some((old_head,old_y,old_along,old_refs))=&previous {
                    if (expected_head-old_head).abs()>1e-5 {
                        let scale=-(actual_head_y-*old_y) as f64/(expected_head-*old_head);
                        assert!(scale.is_finite() && scale>0.0,"ACTUAL mesh translates in model strike direction");
                        for i in 0..expected_along.len() {
                            let projected=scale*(expected_along[i]-old_along[i]);
                            reference_signal|=projected.abs()>REFERENCE_TOLERANCE;
                            assert!(((actual_refs[i]-old_refs[i]) as f64+projected).abs()<REFERENCE_TOLERANCE,
                                "ACTUAL local reference geometry must track coherent along, not a nominal line");
                        }
                        translated=true;
                    }
                }
                let gap_label=format!("Gap {:+.1} mm",(expected_along[0]-expected_head)*1000.0);
                previous=Some((expected_head,actual_head_y,expected_along,actual_refs));
                assert_eq!(observed.felt.len(),neutral.felt.len());
                let neutral_y=neutral.felt.iter().map(|p|p.y).fold(f32::INFINITY,f32::min);
                let translation=actual_head_y-neutral_y;
                for (actual,original) in observed.felt.iter().zip(&neutral.felt) {
                    assert!((actual.x-original.x).abs()<0.001,"ALL actual felt vertices must retain X");
                    assert!(((actual.y-original.y)-translation).abs()<0.001,"ALL actual felt vertices must share one rigid translation/extent");
                }
                let extent=|points:&[Pos2]|vec2(
                    points.iter().map(|p|p.x).fold(f32::NEG_INFINITY,f32::max)-points.iter().map(|p|p.x).fold(f32::INFINITY,f32::min),
                    points.iter().map(|p|p.y).fold(f32::NEG_INFINITY,f32::max)-points.iter().map(|p|p.y).fold(f32::INFINITY,f32::min));
                assert!((extent(&observed.felt)-extent(&neutral.felt)).length()<0.002,"ACTUAL felt mesh extent must remain constant");
                let texts=painted_text(&output);
                for label in ["Felt","Wood molding","Wooden shank"] {
                    assert!(seen_text(&texts,label,paint.panel,paint.clip.intersect(f.screen())),"actual anatomy label");
                }
                let now_label=match paint.status.split(" ·").next().unwrap() {
                    "Measured hammer motion"=>"NOW · measured hammer motion",
                    "Measured return"=>"NOW · measured return",
                    "Measured backcheck"=>"NOW · measured backcheck",
                    "Measured rest"=>"NOW · measured rest",
                    _=>panic!("unexpected actual measured display label"),
                };
                assert!(seen_text(&texts,now_label,paint.panel,paint.clip.intersect(f.screen())),"actual NOW pose label");
                assert!(seen_text(&texts,&gap_label,paint.panel,paint.clip.intersect(f.screen())),"actual measured gap in signed millimetres");
                let expected=format!("{} · {:.1} g · effective let-off {:.2} mm",note_name(meta.midi as i32),
                    meta.hammer_mass_kg*1000.0,meta.let_off_effective_m()*1000.0);
                assert!(seen_text(&texts,&expected,paint.panel,paint.clip.intersect(f.screen())),"actual same-generation model label");
                if let Some((owner,pose_step,solve_step))=paint.cue_steps {
                    assert_eq!(pose_step,solve_step+1);
                    let cue_meta=published.stroke(owner).expect("actual historical contact metadata owner");
                    assert!(published.contacts().any(|contact|contact.stroke_sequence==owner && (contact.entry_pose.pose_step==pose_step || contact.peak_pose.pose_step==pose_step)));
                    let cue_label=format!("EARLIER · recorded contact {}",note_name(cue_meta.midi as i32));
                    assert!(cue_geometry && seen_text(&texts,&cue_label,paint.panel,paint.clip.intersect(f.screen())), 
                        "ACTUAL historical contact line/galley must be present");
                    saw_cue=true;
                }
                last_live=Some((observed,paint.bracket));
            }
        }
        assert!(saw_buffering && live_count>1 && translated && saw_cue,"real State.show motion/reference/contact path");
        assert!(reference_signal,"REAL projected local-string motion never exceeded asserted tolerance; nominal-reference rejection is unproved");
        assert_eq!(frames,11200);
        let publications=p.shared.display_frames.load(Ordering::Acquire);
        let mut paused=0;
        for _ in 0..12 {
            let output=f.frame(Vec::new()); let paint=paint_trace(&f);
            let (observed,_)=geometry(&output,&paint,f.screen());
            if paint.status.starts_with("Measured ") {last_live=Some((observed,paint.bracket));}
            else if paint.status.starts_with("Paused ") {
                let (last_geometry,last_bracket)=last_live.as_ref().unwrap();
                assert!(observed==*last_geometry,"ACTUAL head/reference vertices rewind or move after pause");
                assert_eq!(&paint.bracket,last_bracket);
                assert!(seen_text(&painted_text(&output),paint.status,paint.panel,paint.clip.intersect(f.screen())));
                assert!(seen_text(&painted_text(&output),"NOW · paused measured bracket",paint.panel,paint.clip.intersect(f.screen())),"actual frozen NOW label");
                paused+=1;
            }
        }
        assert!(paused>=2);
        assert_eq!(p.shared.display_frames.load(Ordering::Acquire),publications,"no synthetic publication while physics pauses");
        f.host.assert_no_edits(); assert!(p.shared.inputs.is_empty());
        assert!(f.frame<=80,"bounded actual UI frames per layout");
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
