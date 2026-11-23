//! Authored section anatomy driven ONLY by measured strike-axis positions.
//! No physical hinge, pre-let-off upswing, felt deformation or independent return-shank claim.
use super::*;
use piano_engine::piano::{ActionPhase,ActionPose,ActionPoseRecord,ActionStroke,ActionTrace};

const CROWN:Pos2=Pos2::new(410.0,100.0);
const CURVE_TOLERANCE:f32=0.2;
// Configured for the bounded known512-frame/44.1–48k cadence; not universal A/V latency.
const LOOKBEHIND_S:f64=0.060;
const STALE_S:f64=0.100;
const CONTACT_CUE_S:f64=0.090;
// Declared, PROVISIONAL scene bounds, not physical limits. Real overshoot is not clamped.
// Producer trajectories/extremes must validate this margin before visual acceptance.
const VIEW_GAP_MIN_M:f64=-0.008;
const VIEW_GAP_MAX_M:f64=0.080;

#[derive(Clone,Copy,Debug,PartialEq)]
struct MotionPose {head_m:f64,string_m:[f64;3],phase:ActionPhase}
impl MotionPose {fn gap_m(&self)->f64 {self.string_m[0]-self.head_m}}
#[derive(Clone,Copy,Debug,PartialEq)]
struct Provenance {lower_step:u64,upper_step:u64,alpha:f64,cursor_s:f64}
#[derive(Clone,Copy,Debug,PartialEq)]
struct SampledPose {draw:MotionPose,source:Provenance,meta:ActionStroke}
#[derive(Clone,Copy)]
struct ContactCue {pose:MotionPose,pose_step:u64,solve_step:u64,wire:usize,meta:ActionStroke}
#[derive(Clone,Copy,Debug,PartialEq)]
enum RenderState {Neutral,Buffering,AwaitingBracket,Unavailable(Option<SampledPose>),Live(SampledPose),Paused(SampledPose)}
#[derive(Clone,Copy,PartialEq,Eq)]
enum Part {None,Felt,Hammer,Shank,String,Gap}
fn selected_part(name:&str)->Part {
    match name {
        "Felt hardness"|"Felt damping"=>Part::Felt,
        "Hammer mass"=>Part::Hammer,
        "Shank springiness"=>Part::Shank,
        "Strike point"=>Part::String,
        "Let-off distance"=>Part::Gap,
        _=>Part::None,
    }
}

// Test-only LAST-FRAME scope/provenance/color tags. Geometry is read from FullOutput,
// not echoed here as a predicted head/reference bounding-box oracle.
#[cfg(test)]
#[derive(Clone)]
pub(in crate::editor) struct PaintTrace {
    pub panel:Rect,pub clip:Rect,pub status:&'static str,
    pub identity:Option<(u64,i16,u64)>,
    pub bracket:Option<(u64,u64,f64,f64)>,
    pub cue_steps:Option<(u64,u64,u64)>,
    pub felt_color:Color32,pub reference_color:Color32,pub cue_color:Color32,
}

pub(in crate::editor) struct ActionIllustration {
    shapes:Vec<Shape>,
    felt:Vec<Pos2>,
    molding:Vec<Pos2>,
    shank:Vec<Pos2>,
    trail:Vec<(u64,Pos2)>,
    generation:Option<u64>,
    last_step:u64,
    last_sequence:u64,
    anchor_engine_s:f64,
    anchor_ui_s:f64,
    progress_ui_s:f64,
    suspended:bool,
    initial_fill:bool,
    initial_step:u64,
    owner_unavailable:bool,
    frozen:Option<SampledPose>,
}
impl Default for ActionIllustration {
    fn default()->Self {
        // Short striking crown, rounded shoulders and tapered heel; SAME crown anchor.
        // Authored cutaway only: no observed shape change or physical deformation.
        let mut felt=vec![pos2(392.0,100.0),pos2(424.0,100.0)];
        cubic(&mut felt,pos2(439.0,100.0),pos2(449.0,108.0),pos2(450.0,124.0),CURVE_TOLERANCE);
        cubic(&mut felt,pos2(448.0,144.0),pos2(445.0,175.0),pos2(435.0,198.0),CURVE_TOLERANCE);
        cubic(&mut felt,pos2(424.0,200.0),pos2(408.0,199.0),pos2(401.0,185.0),CURVE_TOLERANCE);
        cubic(&mut felt,pos2(389.0,165.0),pos2(382.0,143.0),pos2(382.0,120.0),CURVE_TOLERANCE);
        cubic(&mut felt,pos2(382.0,108.0),pos2(383.0,100.0),pos2(392.0,100.0),CURVE_TOLERANCE);
        felt.pop();
        let mut molding=vec![pos2(415.0,124.0)];
        cubic(&mut molding,pos2(429.0,123.0),pos2(441.0,135.0),pos2(441.0,151.0),CURVE_TOLERANCE);
        cubic(&mut molding,pos2(441.0,166.0),pos2(438.0,184.0),pos2(431.0,190.0),CURVE_TOLERANCE);
        cubic(&mut molding,pos2(418.0,190.0),pos2(408.0,178.0),pos2(404.0,165.0),CURVE_TOLERANCE);
        cubic(&mut molding,pos2(399.0,150.0),pos2(402.0,130.0),pos2(415.0,124.0),CURVE_TOLERANCE);
        molding.pop();
        let shank=vec![pos2(440.0,164.0),pos2(664.0,221.0),pos2(661.0,230.0),pos2(442.0,178.0)];
        let mut drawing=Drawing::default();
        drawing.surface(shank.clone(),pos2(550.0,196.0),WOOD,Stroke::new(0.9,INK));
        drawing.surface(felt.clone(),pos2(418.0,150.0),KEY,Stroke::new(1.0,INK));
        // Explicit authored cutaway: the LONG wood molding is visible inside the felt wrap.
        drawing.surface(molding.clone(),pos2(420.0,160.0),WOOD,Stroke::new(1.2,FRAME));
        let mut seam=vec![pos2(385.0,120.0)];
        cubic(&mut seam,pos2(385.0,150.0),pos2(398.0,179.0),pos2(411.0,192.0),CURVE_TOLERANCE);
        drawing.line(seam,FRAME,1.0);
        drawing.shapes.push(Shape::circle_stroke(pos2(442.0,171.0),4.0,Stroke::new(0.9,INK)));
        Self {shapes:drawing.shapes,felt,molding,shank,trail:Vec::with_capacity(8),generation:None,last_step:0,last_sequence:0,
            anchor_engine_s:0.0,anchor_ui_s:0.0,progress_ui_s:0.0,
            suspended:true,initial_fill:false,initial_step:0,owner_unavailable:false,frozen:None}
    }
}
impl ActionIllustration {
    fn invalidate(&mut self) {
        self.generation=None;self.frozen=None;self.suspended=true;self.initial_fill=false;
        self.owner_unavailable=false;self.trail.clear();
    }
    /// generation and this owned Snapshot were copied under the SAME Display lock.
    /// The clock follows a generation, NOT the latest focus/stroke; no history copy/cache.
    pub(in crate::editor) fn receive(&mut self,generation:u64,trace:&ActionTrace,now:f64) {
        let first=trace.poses().next();
        if !trace.valid || !now.is_finite() || !trace.internal_dt_s.is_finite() || trace.internal_dt_s<=0.0
            || trace.sample_stride_steps==0 || first.is_none() || trace.current_stroke().is_none() {
            self.invalidate();return;
        }
        let changed=self.generation!=Some(generation);
        if changed {
            self.invalidate();self.generation=Some(generation);
            self.anchor_engine_s=trace.latest_step as f64*trace.internal_dt_s-LOOKBEHIND_S;
            self.anchor_ui_s=now;self.progress_ui_s=now;
            self.initial_step=first.unwrap().pose.pose_step;self.initial_fill=true;self.suspended=false;
        } else if trace.latest_step<self.last_step || now<self.progress_ui_s {
            self.suspended=true;return;
        }
        let distance=first.unwrap().sequence.wrapping_sub(self.last_sequence);
        let gap=!changed && distance>1 && distance<(1u64<<63);
        let fresh=changed || trace.latest_step>self.last_step;
        if !changed && now-self.progress_ui_s>STALE_S {self.suspended=true;}
        if gap {self.suspended=true;}
        else if fresh && self.suspended {
            let candidate=trace.latest_step as f64*trace.internal_dt_s-LOOKBEHIND_S;
            let forward=self.frozen.is_none_or(|shown|candidate>=shown.source.cursor_s);
            match Self::sample(trace,candidate) {
                Ok(Some(_)) if forward=>{
                    self.anchor_engine_s=candidate;self.anchor_ui_s=now;
                    self.suspended=false;self.initial_fill=false;self.owner_unavailable=false;
                },
                Err(_)=>self.owner_unavailable=true,
                _=>{},
            }
        }
        if fresh {self.progress_ui_s=now;}
        self.last_step=trace.latest_step;self.last_sequence=trace.pose_sequence;
    }
    fn measured(meta:&ActionStroke,pose:&ActionPose)->Option<MotionPose> {
        if !(0..128).contains(&meta.midi) || !(1..=3).contains(&meta.string_count)
            || !meta.hammer_mass_kg.is_finite() || meta.hammer_mass_kg<=0.0
            || !meta.let_off_requested_m.is_finite() || !meta.check_distance_m.is_finite()
            || !meta.rest_distance_m.is_finite() {return None;}
        let head=match pose.phase {
            ActionPhase::Flight=>pose.eta_m,
            ActionPhase::Returning|ActionPhase::Checked|ActionPhase::Rest=>{
                let origin=&meta.return_origin;
                if !origin.valid || !origin.reference_observed || pose.pose_step<origin.transition_pose_step {return None;}
                origin.origin_m-pose.hy_m
            },
            ActionPhase::Invalid=>return None,
        };
        (head.is_finite() && pose.string_along_m.iter().take(meta.string_count as usize).all(|x|x.is_finite()))
            .then_some(MotionPose {head_m:head,string_m:pose.string_along_m,phase:pose.phase})
    }
    fn exact(trace:&ActionTrace,record:&ActionPoseRecord,cursor:f64)->Result<Option<SampledPose>,u64> {
        let meta=trace.stroke(record.stroke_sequence).ok_or(record.stroke_sequence)?;
        Ok(Self::measured(meta,&record.pose).map(|draw|SampledPose {draw,meta:*meta,source:Provenance {
            lower_step:record.pose.pose_step,upper_step:record.pose.pose_step,alpha:0.0,cursor_s:cursor}}))
    }
    /// Chronological GLOBAL capture groups FIRST; owner lookup only AFTER winner selection.
    /// Exact same-time boundary chooses the LAST global capture, including a missing owner.
    fn sample(trace:&ActionTrace,cursor:f64)->Result<Option<SampledPose>,u64> {
        let mut records=trace.poses().peekable();
        let mut before:Option<&ActionPoseRecord>=None;
        while let Some(first)=records.next() {
            let step=first.pose.pose_step;
            let time=step as f64*trace.internal_dt_s;
            if !time.is_finite() {return Ok(None);}
            let mut last=first;
            let mut upper=before.filter(|old|old.stroke_sequence==first.stroke_sequence).map(|_|first);
            while records.peek().is_some_and(|record|record.pose.pose_step==step) {
                let record=records.next().unwrap();
                if before.is_some_and(|old|old.stroke_sequence==record.stroke_sequence) {upper=Some(record);}
                last=record;
            }
            if time==cursor {return Self::exact(trace,last,cursor);}
            if time<cursor {before=Some(last);continue;}
            let Some(lower)=before else {return Ok(None);};
            let lower_time=lower.pose.pose_step as f64*trace.internal_dt_s;
            if lower_time==cursor {return Self::exact(trace,lower,cursor);}
            let meta=trace.stroke(lower.stroke_sequence).ok_or(lower.stroke_sequence)?;
            let Some(upper)=upper else {return Ok(None);}; // No cross-stroke bracket/tween.
            let a=Self::measured(meta,&lower.pose);let b=Self::measured(meta,&upper.pose);
            let (Some(a),Some(b))=(a,b) else {return Ok(None);};
            let span=time-lower_time;
            if span<=0.0 || span>2.0*trace.sample_stride_steps as f64*trace.internal_dt_s+1e-12 {return Ok(None);}
            // The OLD terminal endpoint at S is upper ONLY while cursor<S.
            let alpha=if a.phase==b.phase {(cursor-lower_time)/span} else {0.0};
            let draw=MotionPose {head_m:a.head_m+(b.head_m-a.head_m)*alpha,
                string_m:std::array::from_fn(|i|a.string_m[i]+(b.string_m[i]-a.string_m[i])*alpha),phase:a.phase};
            return Ok(Some(SampledPose {draw,meta:*meta,source:Provenance {lower_step:lower.pose.pose_step,
                upper_step:if alpha==0.0 {lower.pose.pose_step} else {upper.pose.pose_step},alpha,cursor_s:cursor}}));
        }
        Ok(None) // No extrapolation from ended coverage or latest point.
    }
    fn render_state(&mut self,trace:&ActionTrace,now:f64)->RenderState {
        if self.generation.is_none() || !trace.valid {return RenderState::Neutral;}
        if !now.is_finite() || now<self.anchor_ui_s || now-self.progress_ui_s>STALE_S {self.suspended=true;}
        if !self.suspended {
            let cursor=self.anchor_engine_s+(now-self.anchor_ui_s);
            let first=trace.poses().next().unwrap().pose.pose_step;
            if self.initial_fill && cursor<self.initial_step as f64*trace.internal_dt_s && first==self.initial_step {return RenderState::Buffering;}
            if self.frozen.is_some_and(|shown|cursor<shown.source.cursor_s) {self.suspended=true;}
            else {
                match Self::sample(trace,cursor) {
                    Ok(Some(sample))=>{
                        self.initial_fill=false;self.owner_unavailable=false;self.frozen=Some(sample);
                        return RenderState::Live(sample);
                    },
                    Err(_)=>{self.owner_unavailable=true;self.suspended=true;},
                    Ok(None)=>{self.owner_unavailable=false;self.suspended=true;},
                }
            }
        }
        if self.owner_unavailable {RenderState::Unavailable(self.frozen)}
        else {self.frozen.map_or(RenderState::AwaitingBracket,RenderState::Paused)}
    }
    fn recent_contact(trace:&ActionTrace,cursor:f64)->Option<ContactCue> {
        let mut chosen:Option<ContactCue>=None;
        for contact in trace.contacts() {
            let Some(meta)=trace.stroke(contact.stroke_sequence) else {continue;}; // Never substitute latest metadata.
            for (pose,solve_step) in [(&contact.entry_pose,contact.entry_solve_step),(&contact.peak_pose,contact.peak_solve_step)] {
                let time=pose.pose_step as f64*trace.internal_dt_s;
                if !pose.force_observed() || pose.pose_step!=solve_step.saturating_add(1)
                    || time>cursor || cursor-time>CONTACT_CUE_S {continue;}
                let Some(wire)=pose.force_n.iter().take(meta.string_count as usize).position(|f|f.is_finite() && *f>0.0) else {continue;};
                let Some(measured)=Self::measured(meta,pose) else {continue;};
                if chosen.is_none_or(|old|pose.pose_step>old.pose_step) {
                    chosen=Some(ContactCue {pose:measured,pose_step:pose.pose_step,solve_step,wire,meta:*meta});
                }
            }
        }
        chosen
    }
    pub(in crate::editor) fn paint(&mut self,ui:&mut Ui,trace:&ActionTrace,active:&str,height:f32)->Response {
        let now=ui.ctx().input(|input|input.time);
        let state=self.render_state(trace,now);
        let (rect,response)=ui.allocate_exact_size(vec2(ui.available_width(),height),Sense::hover());
        let painter=ui.painter_at(rect); let part=selected_part(active);
        let moving=matches!(state,RenderState::Live(_));
        let shown=match state {RenderState::Live(shown)|RenderState::Paused(shown)=>Some(shown),RenderState::Unavailable(shown)=>shown,RenderState::Neutral|RenderState::Buffering|RenderState::AwaitingBracket=>None};
        let pose=shown.map(|shown|shown.draw);
        let scale=((rect.width()-145.0).max(1.0)/295.0).min(height*0.27/132.0);
        let pixels_per_m=((height-40.0-132.0*scale).max(1.0) as f64)/(VIEW_GAP_MAX_M-VIEW_GAP_MIN_M);
        let axis_y=rect.top()+16.0+(-VIEW_GAP_MIN_M*pixels_per_m) as f32;
        let crown_x=rect.left()+rect.width()*0.34;
        // Head remains in the MODEL AXIS. Moving string coordinates never fake head movement.
        let crown_y=pose.map_or(rect.top()+52.0,|pose|axis_y-(pose.head_m*pixels_per_m) as f32);
        let transform=TSTransform::new(vec2(crown_x,crown_y)-CROWN.to_vec2()*scale,scale);
        if let Some(pose)=pose {
            for i in 0..shown.unwrap().meta.string_count as usize {
                let y=axis_y-(pose.string_m[i]*pixels_per_m) as f32;
                if y.is_finite() {
                    let half=35.0-5.0*i as f32;
                    painter.line_segment([pos2(crown_x-half,y),pos2(crown_x+half,y)],
                        Stroke::new(1.0,if part==Part::String {EDGE} else {INK}));
                }
            }
            let y=axis_y-(pose.string_m[0]*pixels_per_m) as f32;
            if y.is_finite() {painter.text(pos2(crown_x-35.0,y-3.0),Align2::LEFT_BOTTOM,
                "Measured local string references",FontId::proportional(10.0),MUTED);}
        } else {
            painter.line_segment([pos2(rect.left()+12.0,axis_y),pos2(rect.right()-12.0,axis_y)],Stroke::new(1.0,INK));
            painter.text(pos2(rect.left()+12.0,axis_y-3.0),Align2::LEFT_BOTTOM,
                "Neutral string guide",FontId::proportional(10.0),MUTED);
        }
        let cue=if moving {Self::recent_contact(trace,shown.unwrap().source.cursor_s)} else {None};
        self.trail.clear();
        if moving {
            let cursor=shown.unwrap().source.cursor_s;
            for record in trace.poses() {
                let time=record.pose.pose_step as f64*trace.internal_dt_s;
                if time>cursor || cursor-time>0.020 {continue;}
                if record.stroke_sequence!=shown.unwrap().meta.stroke_sequence {continue;}
                let Some(owner)=trace.stroke(record.stroke_sequence) else {continue;};
                if let Some(observed)=Self::measured(owner,&record.pose) {
                    let y=axis_y-(observed.head_m*pixels_per_m) as f32;
                    if y.is_finite() {
                        if self.trail.len()==8 {self.trail.remove(0);}
                        self.trail.push((record.pose.pose_step,pos2(crown_x-12.0,y)));
                    }
                }
            }
            // Historical contact may predate retained routine history: draw its
            // ghost/marker separately, never connect across an unobserved interval.
            self.trail.sort_by_key(|point|point.0); self.trail.dedup_by_key(|point|point.0);
            if self.trail.len()>1 {painter.add(Shape::line(self.trail.iter().map(|point|point.1).collect(),
                Stroke::new(0.8,FRAME.gamma_multiply(0.45))));}
        }
        if crown_y.is_finite() {
            for shape in &self.shapes {let mut shape=shape.clone();shape.transform(transform);painter.add(shape);}
            let outline=match part {Part::Felt=>Some(&self.felt),Part::Hammer=>Some(&self.molding),Part::Shank=>Some(&self.shank),_=>None};
            if let Some(outline)=outline {painter.add(Shape::closed_line(outline.iter().map(|&p|transform.mul_pos(p)).collect(),Stroke::new(1.2,EDGE)));}
            for (i,(label,point)) in [("Felt",pos2(446.0,124.0)),("Wood molding",pos2(431.0,158.0)),("Wooden shank",pos2(592.0,205.0))].into_iter().enumerate() {
                let label_x=(transform.mul_pos(pos2(666.0,219.0)).x+20.0).min(rect.right()-112.0);
                let at=pos2(label_x,rect.top()+65.0+25.0*i as f32); let endpoint=transform.mul_pos(point);
                if rect.contains(endpoint) {painter.line_segment([endpoint,pos2(at.x-5.0,at.y)],Stroke::new(0.6,FRAME));}
                painter.text(at,Align2::LEFT_CENTER,label,FontId::proportional(11.0),INK);
            }
        }
        if let Some(cue)=cue.filter(|cue|cue.pose_step==cue.solve_step.saturating_add(1)) {
            // Clearly HISTORICAL outline at the actual event head position, not a live-head hold.
            let y=axis_y-(cue.pose.head_m*pixels_per_m) as f32;
            let reference_y=axis_y-(cue.pose.string_m[cue.wire]*pixels_per_m) as f32;
            if y.is_finite() && reference_y.is_finite() {
                let event_transform=TSTransform::new(vec2(crown_x,y)-CROWN.to_vec2()*scale,scale);
                painter.add(Shape::closed_line(self.felt.iter().map(|&p|event_transform.mul_pos(p)).collect(),
                    Stroke::new(0.9,EDGE.gamma_multiply(0.45))));
                painter.line_segment([pos2(crown_x-8.0,reference_y),pos2(crown_x+8.0,reference_y)],Stroke::new(2.0,EDGE));
                let at=pos2(crown_x+52.0*scale+10.0,reference_y+7.0);
                let label=painter.layout(format!("EARLIER · recorded contact {}",note_name(cue.meta.midi as i32)),
                    FontId::proportional(11.0),EDGE,(rect.right()-at.x-12.0).min(132.0).max(1.0));
                painter.galley(at,label,EDGE);
            }
        }
        let status=match state {
            RenderState::Neutral=>"Waiting for simulation samples · neutral diagram",
            RenderState::Buffering=>"Buffering measured stroke · 60 ms lookbehind",
            RenderState::AwaitingBracket=>"Paused · waiting for actual measured bracket",
            RenderState::Unavailable(_)=>"Unavailable recorded stroke metadata",
            RenderState::Paused(_)=>"Paused · last displayed measured bracket",
            RenderState::Live(sample)=>match sample.draw.phase {
                ActionPhase::Flight=>"Measured hammer motion · 60 ms lookbehind",
                ActionPhase::Returning=>"Measured return · 60 ms lookbehind",
                ActionPhase::Checked=>"Measured backcheck · 60 ms lookbehind",
                ActionPhase::Rest=>"Measured rest · 60 ms lookbehind",
                ActionPhase::Invalid=>"Waiting for simulation",
            },
        };
        painter.text(pos2(rect.left()+12.0,rect.bottom()-4.0),Align2::LEFT_BOTTOM,status,FontId::proportional(12.0),INK);
        if shown.is_some() && crown_y.is_finite() {
            let label=match state {
                RenderState::Paused(_)=>"NOW · paused measured bracket",
                RenderState::Unavailable(_)=>"LAST · unavailable metadata",
                RenderState::Live(sample)=>match sample.draw.phase {
                    ActionPhase::Flight=>"NOW · measured hammer motion",
                    ActionPhase::Returning=>"NOW · measured return",
                    ActionPhase::Checked=>"NOW · measured backcheck",
                    ActionPhase::Rest=>"NOW · measured rest",
                    ActionPhase::Invalid=>"Waiting for simulation",
                },
                _=>"Waiting for simulation",
            };
            // Bound only the caption layout, never the measured pose/model-axis projection.
            painter.text(pos2(crown_x-35.0,(crown_y+132.0*scale+12.0).min(rect.bottom()-48.0)),
                Align2::LEFT_TOP,label,FontId::proportional(11.0),INK);
        }
        if pose.is_some() {
            painter.text(pos2(rect.left()+12.0,rect.bottom()-19.0),Align2::LEFT_BOTTOM,
                format!("{} · {:.1} g · effective let-off {:.2} mm",note_name(shown.unwrap().meta.midi as i32),shown.unwrap().meta.hammer_mass_kg*1000.0,shown.unwrap().meta.let_off_effective_m()*1000.0),
                FontId::proportional(10.0),if part==Part::Gap {EDGE} else {MUTED});
        }
        if let Some(pose)=pose {
            painter.text(pos2(rect.right()-12.0,rect.bottom()-19.0),Align2::RIGHT_BOTTOM,
                format!("Gap {:+.1} mm",pose.gap_m()*1000.0),FontId::proportional(11.0),INK);
            // Scene direction is the actual MODEL AXIS/crown projection, not
            // head-versus-moving-string clearance. Report that clearance separately.
            let downward_axis=-pose.head_m;
            if downward_axis<VIEW_GAP_MIN_M || downward_axis>VIEW_GAP_MAX_M || !crown_y.is_finite() || !rect.contains(pos2(crown_x,crown_y)) {
                let top=downward_axis<VIEW_GAP_MIN_M || (crown_y.is_finite() && crown_y<rect.top());
                let y=if top {rect.top()+5.0} else {rect.bottom()-32.0};
                painter.text(pos2(crown_x,y),if top {Align2::CENTER_TOP} else {Align2::CENTER_BOTTOM},
                    format!("{} Head outside scene: {:+.3e} m axis",if top {"↑"} else {"↓"},pose.head_m),FontId::proportional(10.0),EDGE);
            }
        }
        #[cfg(test)]
        {
            let snapshot=PaintTrace {panel:rect,clip:painter.clip_rect(),status,
                identity:shown.map(|sample|(self.generation.unwrap(),sample.meta.midi,sample.meta.stroke_sequence)),
                bracket:shown.map(|shown|(shown.source.lower_step,shown.source.upper_step,shown.source.alpha,shown.source.cursor_s)),
                cue_steps:cue.map(|cue|(cue.meta.stroke_sequence,cue.pose_step,cue.solve_step)),
                felt_color:KEY,reference_color:if part==Part::String {EDGE} else {INK},cue_color:EDGE};
            ui.ctx().data_mut(|data|data.insert_temp(Id::new("action:paint"),snapshot));
        }
        response.on_hover_text("Authored hammer section; head and local string references use the SAME measured bracket. Model begins at let-off, not a lever upswing. Paused pose retains last displayed bracket provenance. Recorded contact is a historical solved interval/endpoint, not current compression. Wood/shank remain authored anatomy. Configured60ms lookbehind is not measured A/V latency; fixed −8…80mm envelope is provisional.")
    }
}

#[cfg(test)]
#[path = "action_consumer_tests.rs"]
mod consumer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_anatomy_is_cached_finite_and_bounded_without_simulation_claims() {
        let art=ActionIllustration::default();
        assert!(art.generation.is_none() && art.frozen.is_none());
        let vertices:usize=art.shapes.iter().map(|shape|match shape {Shape::Mesh(mesh)=>mesh.vertices.len(),_=>0}).sum();
        assert!(vertices<=512,"cached anatomy surface vertex budget: {vertices}");
        for shape in &art.shapes {let r=shape.visual_bounding_rect(); assert!(r.min.is_finite() && r.max.is_finite());}
    }
    #[test]
    fn cached_anatomy_uses_valid_mesh_indices_and_existing_fill_kernels() {
        let art=ActionIllustration::default();
        for shape in &art.shapes {
            if let Shape::Mesh(mesh)=shape {
                assert!(mesh.indices.iter().all(|&i|(i as usize)<mesh.vertices.len()));
                let contour:Vec<_>=mesh.vertices[1..].iter().map(|vertex|vertex.pos).collect();
                let sign=signed_area(&contour).signum() as f32; let kernel=mesh.vertices[0].pos;
                for (&a,&b) in contour.iter().zip(contour.iter().cycle().skip(1)).take(contour.len()) {
                    let edge=b-a; let toward=kernel-a;
                    assert!((edge.x*toward.y-edge.y*toward.x)*sign>=-0.1,"authored fill kernel outside contour");
                }
            }
        }
    }
}
