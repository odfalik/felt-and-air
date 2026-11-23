//! Read-only ONE-focus observations retained across strokes. No physical forces/timing change.
//! Previously recorded motion survives retarget; future unfocused voices are NOT observed.
//! A solve at S applies over [S,S+1]; its attached pose is observed at endpoint S+1.
//! Contact episodes aggregate ANY positive solved wire force, not individual wire edges.

pub const ACTION_POSES: usize = 128;
pub const ACTION_CONTACTS: usize = 16;
pub const ACTION_STROKES: usize = 16;
pub const FORCE_OBSERVED: u16 = 1;
pub const KEY_DOWN: u16 = 2;
/// A real shank mass coordinate exists AND is being advanced in Flight.
pub const SHANK_OBSERVED: u16 = 4;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionPhase {
    #[default]
    Invalid,
    Flight,
    Returning,
    Checked,
    Rest,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionPose {
    pub pose_step: u64,
    /// Meaningful only with FORCE_OBSERVED. NOT an instantaneous endpoint force.
    pub force_solve_step: u64,
    /// Metres along strike; head valid in Flight, shank also requires SHANK_OBSERVED.
    /// Velocities are discrete per-step increments / dt, not unstaggered endpoint estimates.
    pub eta_m: f64,
    pub eta_velocity_mps: f64,
    pub xi_m: f64,
    pub xi_velocity_mps: f64,
    /// Metres below the return reference / metres per second downward.
    pub hy_m: f64,
    pub hv_mps: f64,
    /// Metres key-down / metres per second key-down.
    pub ky_m: f64,
    pub kv_mps: f64,
    /// Actual weighted strike-axis coordinates, including both polarizations.
    pub string_along_m: [f64; 3],
    pub force_n: [f64; 3],
    pub flags: u16,
    pub key_phase: u8,
    /// Flight takes precedence over key return; never infer motion from key_phase.
    pub phase: ActionPhase,
}

impl ActionPose {
    pub fn force_observed(&self) -> bool { self.flags & FORCE_OBSERVED != 0 }
    pub fn key_down(&self) -> bool { self.flags & KEY_DOWN != 0 }
    pub fn shank_observed(&self) -> bool { self.flags & SHANK_OBSERVED != 0 }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionPoseRecord {
    pub sequence: u64,
    pub stroke_sequence: u64,
    pub pose: ActionPose,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionContact {
    pub sequence: u64,
    pub stroke_sequence: u64,
    pub entry_solve_step: u64,
    pub last_positive_solve_step: u64,
    /// Undefined unless exit_force_observed; use observed_exit_solve_step().
    pub exit_solve_step: u64,
    pub peak_solve_step: u64,
    pub peak_total_force_n: f64,
    pub entry_pose: ActionPose,
    pub peak_pose: ActionPose,
    pub exit_pose: ActionPose,
    pub open: bool,
    /// False means termination without a new observed zero-force solve, not a measured exit.
    pub exit_force_observed: bool,
    /// Force observation stopped without a measured zero-force exit. Pose sampling may continue.
    pub observation_ended: bool,
}

impl ActionContact {
    pub fn observed_exit_solve_step(&self)->Option<u64> {
        (!self.open && self.exit_force_observed).then_some(self.exit_solve_step)
    }
    /// Also valid for implicit termination without a measured zero-force solve.
    pub fn termination_pose_step(&self)->Option<u64> {
        (!self.open).then_some(self.exit_pose.pose_step)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionReturnOrigin {
    pub transition_pose_step: u64,
    pub eta_at_transition_m: f64,
    pub hy_seed_m: f64,
    pub zeta_at_transition_m: f64,
    /// Telemetry basis only: eta_next + hy_seed, never a physical-state correction.
    pub origin_m: f64,
    pub along0_after_advance_m: f64,
    pub hy_before_action_m: f64,
    pub valid: bool,
    pub reference_observed: bool,
}


#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionStroke {
    pub stroke_sequence: u64,
    pub onset_step: u64,
    pub last_observed_pose_step: u64,
    /// Meaningful when !observing: the actual pre-retarget endpoint was recorded here.
    pub observation_end_step: u64,
    pub let_off_requested_m: f64,
    pub check_distance_m: f64,
    pub rest_distance_m: f64,
    pub hammer_mass_kg: f64,
    pub return_origin: ActionReturnOrigin,
    pub midi: i16,
    pub string_count: u8,
    pub observing: bool,
}
impl ActionStroke {
    pub fn let_off_effective_m(&self)->f64 {self.let_off_requested_m.max(1e-5)}
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
pub struct ActionTrace {
    pub stroke_sequence: u64,
    pub latest_step: u64,
    pub pose_sequence: u64,
    pub contact_sequence: u64,
    pub pose_overwrites: u64,
    pub contact_overwrites: u64,
    pub metadata_overwrites: u64,
    pub internal_dt_s: f64,
    pub sample_stride_steps: u32,
    pose_head:u16, pose_len:u16,
    contact_head:u16, contact_len:u16,
    stroke_head:u16, stroke_len:u16,
    pub valid:bool,
    strokes:[ActionStroke;ACTION_STROKES],
    poses:[ActionPoseRecord;ACTION_POSES],
    contacts:[ActionContact;ACTION_CONTACTS],
}
impl Default for ActionTrace {
    fn default()->Self {
        Self {stroke_sequence:0,latest_step:0,pose_sequence:0,contact_sequence:0,
            pose_overwrites:0,contact_overwrites:0,metadata_overwrites:0,internal_dt_s:0.0,
            sample_stride_steps:1,pose_head:0,pose_len:0,contact_head:0,contact_len:0,
            stroke_head:0,stroke_len:0,valid:false,
            strokes:[ActionStroke::default();ACTION_STROKES],
            poses:[ActionPoseRecord::default();ACTION_POSES],
            contacts:[ActionContact::default();ACTION_CONTACTS]}
    }
}
impl std::fmt::Debug for ActionTrace {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("ActionTrace").field("valid",&self.valid)
            .field("current_stroke",&self.stroke_sequence).field("latest_step",&self.latest_step)
            .field("poses",&self.pose_len).field("contacts",&self.contact_len).field("metadata",&self.stroke_len)
            .field("pose_overwrites",&self.pose_overwrites).field("contact_overwrites",&self.contact_overwrites)
            .field("metadata_overwrites",&self.metadata_overwrites).finish_non_exhaustive()
    }
}
impl ActionTrace {
    pub fn poses(&self) -> impl Iterator<Item = &ActionPoseRecord> {
        (0..self.pose_len as usize).map(move |i| &self.poses[(self.pose_head as usize+i)%ACTION_POSES])
    }
    pub fn contacts(&self) -> impl Iterator<Item = &ActionContact> {
        (0..self.contact_len as usize).map(move |i| &self.contacts[(self.contact_head as usize+i)%ACTION_CONTACTS])
    }
    pub fn strokes(&self)->impl Iterator<Item=&ActionStroke> {
        (0..self.stroke_len as usize).map(move |i|&self.strokes[(self.stroke_head as usize+i)%ACTION_STROKES])
    }
    pub fn stroke(&self,id:u64)->Option<&ActionStroke> {
        self.strokes().find(|s|s.stroke_sequence==id)
    }
    pub fn current_stroke(&self)->Option<&ActionStroke> {
        if !self.valid || self.stroke_len==0 {return None;}
        let i=(self.stroke_head as usize+self.stroke_len as usize-1)%ACTION_STROKES;
        let meta=&self.strokes[i];
        (meta.stroke_sequence==self.stroke_sequence).then_some(meta)
    }
    pub fn unresolved_pose_count(&self)->usize {
        self.poses().filter(|r|self.stroke(r.stroke_sequence).is_none()).count()
    }
    pub fn unresolved_contact_count(&self)->usize {
        self.contacts().filter(|r|self.stroke(r.stroke_sequence).is_none()).count()
    }
    fn current_mut(&mut self)->&mut ActionStroke {
        let i=(self.stroke_head as usize+self.stroke_len as usize-1)%ACTION_STROKES;
        &mut self.strokes[i]
    }
    fn open_contact(&self)->Option<usize> {
        if self.contact_len==0 {return None;}
        let i=(self.contact_head as usize+self.contact_len as usize-1)%ACTION_CONTACTS;
        (self.contacts[i].open && self.contacts[i].stroke_sequence==self.stroke_sequence).then_some(i)
    }
    fn push_pose(&mut self, pose: ActionPose) {
        let i=(self.pose_head as usize+self.pose_len as usize)%ACTION_POSES;
        if self.pose_len as usize == ACTION_POSES {
            self.pose_head=(self.pose_head+1)%ACTION_POSES as u16;
            self.pose_overwrites=self.pose_overwrites.wrapping_add(1);
        } else { self.pose_len+=1; }
        self.pose_sequence=self.pose_sequence.wrapping_add(1);
        self.poses[i]=ActionPoseRecord { sequence:self.pose_sequence, stroke_sequence:self.stroke_sequence, pose };
        self.current_mut().last_observed_pose_step=pose.pose_step;
    }
}

// Source repr(C) calculation, to be confirmed/rejected by a FUTURE compiler gate.
// Fixed totals remain128 poses /16 contacts;16 metadata slots fit the32KiB cap.
const _:()=assert!(std::mem::size_of::<ActionPose>()==136);
const _:()=assert!(std::mem::size_of::<ActionPoseRecord>()==152);
const _:()=assert!(std::mem::size_of::<ActionContact>()==472);
const _:()=assert!(std::mem::size_of::<ActionReturnOrigin>()==64);
const _:()=assert!(std::mem::size_of::<ActionStroke>()==136);
const _:()=assert!(std::mem::size_of::<ActionTrace>()==29272);
const _:()=assert!(std::mem::size_of::<ActionTrace>()<=32*1024);

#[derive(Clone, Copy, Default)]
pub(super) struct SolvedForce {
    pub step: u64,
    pub wires: [f64; 3],
    pub total: f64,
    pub observed: bool,
}
pub(super) struct ActionObserver {
    pub trace: ActionTrace,
    focus: Option<usize>,
    pending: SolvedForce,
    next_sample: u64,
    transition_pending: bool,
    #[cfg(test)]
    pub enabled: bool,
}
impl ActionObserver {
    pub fn new(k: f64) -> Self {
        let mut trace=ActionTrace::default();
        trace.internal_dt_s=k;
        trace.sample_stride_steps=((1.0/k/1000.0).ceil() as u32).max(1);
        Self { trace, focus:None, pending:SolvedForce::default(), next_sample:0,
            transition_pending:false, #[cfg(test)] enabled:true }
    }
    pub fn focus(&self) -> Option<usize> {
        #[cfg(test)]
        if !self.enabled { return None; }
        self.focus
    }
    /// MUST be called with the actual previous-focus endpoint BEFORE any note mutation.
    pub fn end_focus(&mut self,pose:ActionPose) {
        if self.focus().is_none() {return;}
        self.trace.push_pose(pose);
        let meta=self.trace.current_mut();
        meta.observing=false;meta.observation_end_step=pose.pose_step;
        if let Some(i)=self.trace.open_contact() {
            let c=&mut self.trace.contacts[i];
            c.open=false;c.exit_force_observed=false;c.exit_solve_step=0;
            c.exit_pose=pose;c.observation_ended=true;
        }
        self.focus=None;self.pending=SolvedForce::default();self.transition_pending=false;
    }
    pub fn begin(&mut self,ni:usize,midi:i32,count:usize,step:u64,
        let_off:f64,check:f64,rest:f64,mass:f64) {
        #[cfg(test)] if !self.enabled {return;}
        debug_assert!(self.focus.is_none(),"previous observation must end before note mutation");
        self.focus=Some(ni);
        let t=&mut self.trace;
        let i=(t.stroke_head as usize+t.stroke_len as usize)%ACTION_STROKES;
        if t.stroke_len as usize==ACTION_STROKES {
            t.stroke_head=(t.stroke_head+1)%ACTION_STROKES as u16;
            t.metadata_overwrites=t.metadata_overwrites.wrapping_add(1);
        } else {t.stroke_len+=1;}
        t.stroke_sequence=t.stroke_sequence.wrapping_add(1);
        t.strokes[i]=ActionStroke {stroke_sequence:t.stroke_sequence,onset_step:step,
            last_observed_pose_step:step,let_off_requested_m:let_off,check_distance_m:check,
            rest_distance_m:rest,hammer_mass_kg:mass,midi:midi as i16,string_count:count as u8,
            observing:true,..ActionStroke::default()};
        t.latest_step=step;t.valid=true;
        // Keep existing records/counters; reused metadata is matched by full stroke ID.
        self.pending=SolvedForce::default();self.transition_pending=false;self.next_sample=step;
    }
    pub fn force(&mut self, ni:usize, step:u64, wires:[f64;3], total:f64) {
        if self.focus()==Some(ni) { self.pending=SolvedForce { step,wires,total,observed:true }; }
    }
    pub fn returning(&mut self,ni:usize,step:u64,eta:f64,hy:f64,zeta:f64) {
        if self.focus()!=Some(ni) {return;}
        self.trace.current_mut().return_origin=ActionReturnOrigin {transition_pose_step:step,
            eta_at_transition_m:eta,hy_seed_m:hy,zeta_at_transition_m:zeta,origin_m:eta+hy,
            valid:true,..ActionReturnOrigin::default()};
        self.transition_pending=true;
    }
    pub fn needs_reference(&self,ni:usize)->bool {
        self.focus()==Some(ni) && self.trace.current_stroke().is_some_and(|s|
            s.return_origin.valid && !s.return_origin.reference_observed)
    }
    pub fn return_reference(&mut self,along0:f64,hy:f64) {
        let o=&mut self.trace.current_mut().return_origin;
        o.along0_after_advance_m=along0;o.hy_before_action_m=hy;o.reference_observed=true;
    }
    pub fn force_observation(&self) -> SolvedForce { self.pending }
    pub fn needs_pose(&self, step:u64) -> bool {
        if self.focus().is_none() { return false; }
        let open=self.trace.open_contact();
        step>=self.next_sample || self.transition_pending || match open {
            Some(i)=>self.pending.total<=0.0 || self.pending.total>self.trace.contacts[i].peak_total_force_n,
            None=>self.pending.total>0.0,
        }
    }
    pub fn finish_step(&mut self, step:u64, pose:Option<ActionPose>) {
        if self.focus().is_none() { return; }
        self.trace.latest_step=step;
        if let Some(pose)=pose {
            if step>=self.next_sample || self.transition_pending {
                self.trace.push_pose(pose);
                self.next_sample=step.saturating_add(self.trace.sample_stride_steps as u64);
            }
            self.trace.current_mut().last_observed_pose_step=pose.pose_step;
            if self.pending.total>0.0 {
                let i=if let Some(i)=self.trace.open_contact() { i } else {
                    let t=&mut self.trace;
                    let i=(t.contact_head as usize+t.contact_len as usize)%ACTION_CONTACTS;
                    if t.contact_len as usize==ACTION_CONTACTS {
                        t.contact_head=(t.contact_head+1)%ACTION_CONTACTS as u16;
                        t.contact_overwrites=t.contact_overwrites.wrapping_add(1);
                    } else { t.contact_len+=1; }
                    t.contact_sequence=t.contact_sequence.wrapping_add(1);
                    t.contacts[i]=ActionContact { sequence:t.contact_sequence, stroke_sequence:t.stroke_sequence, entry_solve_step:self.pending.step,
                        last_positive_solve_step:self.pending.step, peak_solve_step:self.pending.step,
                        peak_total_force_n:self.pending.total, entry_pose:pose, peak_pose:pose,
                        open:true, ..ActionContact::default() };
                    i
                };
                let c=&mut self.trace.contacts[i];
                c.last_positive_solve_step=self.pending.step;
                if self.pending.total>c.peak_total_force_n {
                    c.peak_total_force_n=self.pending.total; c.peak_solve_step=self.pending.step; c.peak_pose=pose;
                }
            } else if let Some(i)=self.trace.open_contact() {
                let c=&mut self.trace.contacts[i];
                c.open=false; c.exit_force_observed=self.pending.observed;
                c.exit_solve_step=self.pending.step; c.exit_pose=pose;
                // This ends FORCE observation only; current metadata still observes poses.
                c.observation_ended=!self.pending.observed;
            }
        } else if self.pending.total>0.0 {
            if let Some(i)=self.trace.open_contact() { self.trace.contacts[i].last_positive_solve_step=self.pending.step; }
        }
        self.pending=SolvedForce::default(); self.transition_pending=false;
    }
}
const _:()=assert!(std::mem::size_of::<ActionObserver>()<=32*1024);
