//! Opt-in OFFLINE engine→actual State.show trajectories. SOURCE ONLY until approved.
use super::*;
#[derive(serde::Serialize,serde::Deserialize)]
#[serde(untagged)]
enum InputRecord {
    Single {boundary:String,ui_pts_s:f64,output_frame:usize,key:String,pressed:bool},
    Dense {boundary:String,output_frame:usize,engine_output_pts_s:f64,midi:i32,velocity:i32,release:f64},
}
fn real_inputs_roundtrip(events:&[InputRecord],epoch:Option<f64>) {
    let restored:Vec<InputRecord>=serde_json::from_slice(&serde_json::to_vec(events).unwrap()).unwrap();
    assert_eq!(events.len(),restored.len());
    for (a,b) in events.iter().zip(&restored) {
        match (a,b) {
            (InputRecord::Single {boundary:a,ui_pts_s:x,output_frame:p,key:k,pressed:z},
             InputRecord::Single {boundary:b,ui_pts_s:y,output_frame:q,key:l,pressed:w})=>{
                assert_eq!((a,p,k,z),(b,q,l,w));assert_eq!(x.to_bits(),y.to_bits());
            },
            (InputRecord::Dense {boundary:a,output_frame:p,engine_output_pts_s:x,midi:m,velocity:v,release:r},
             InputRecord::Dense {boundary:b,output_frame:q,engine_output_pts_s:y,midi:n,velocity:u,release:s})=>{
                assert_eq!((a,p,m,v),(b,q,n,u));assert_eq!(x.to_bits(),y.to_bits());assert_eq!(r.to_bits(),s.to_bits());
            },
            _=>panic!("actual input boundary changed"),
        }
    }
    let restored:Option<f64>=serde_json::from_slice(&serde_json::to_vec(&epoch).unwrap()).unwrap();
    assert_eq!(epoch.map(f64::to_bits),restored.map(f64::to_bits));
}
#[derive(serde::Serialize,serde::Deserialize)]
struct Logical {logical_size:[f32;2],pixels_per_point:f32}
fn logical_roundtrip(logical:&Logical) {
    let restored:Logical=serde_json::from_slice(&serde_json::to_vec(logical).unwrap()).unwrap();
    assert!(same_f32(&logical.logical_size,&restored.logical_size));
    assert_eq!(logical.pixels_per_point.to_bits(),restored.pixels_per_point.to_bits());
}
#[derive(serde::Serialize)]
pub(super) struct StaticDocument<'a> {
    pub format:&'static str,pub revision:&'a str,pub mode:&'static str,pub category_index:usize,
    pub category_name:&'a str,pub viewport_preset:&'a str,pub logical_size:[i32;2],
    pub pixels_per_point:f32,pub frames:&'a [GpuFrame],pub limits:&'static str,
}
#[derive(serde::Serialize)]
struct Counts {visible:usize,history:u64,physics_frames:usize}
#[derive(serde::Serialize)]
struct TextureHistory {recorded_before_first_frame:bool,rgba_bytes:usize}
#[derive(serde::Serialize)]
struct Limits {visible:usize,history:usize,texture_bytes:usize,mesh_bytes_per_frame:usize,json_bytes:usize,total_bytes:usize}
#[derive(serde::Serialize)]
struct Document<'a> {
    format:&'static str,revision:&'a str,label:&'static str,scenario:&'a str,viewport:&'a str,
    #[serde(flatten)]logical:Logical,input_boundary:&'static str,time_policy:&'static str,
    ui_clock_hz:usize,engine_sample_rate:usize,engine_epoch_ui_s:Option<f64>,clock_boundary:&'static str,
    events:&'a [InputRecord],frames:&'a [Frame],texture_history:TextureHistory,counts:Counts,limits:Limits,
}

const HISTORY_CAP:usize=256;
const VISIBLE_CAP:usize=120;
const TEXTURE_BYTE_CAP:usize=8*1024*1024;
const REPLAY_MESH_BYTE_CAP:usize=8*1024*1024;
const JSON_CAP:usize=64*1024*1024;
const TOTAL_CAP:usize=512*1024*1024;
#[derive(serde::Serialize,serde::Deserialize)]
pub(super) struct Upload {id:u64,size:[usize;2],pos:Option<[usize;2]>,mag:String,min:String,wrap:String,rgba:Vec<[u8;4]>}
#[derive(serde::Serialize,serde::Deserialize)]
pub(super) struct Mesh {
    texture:u64,clip:[f32;4],
    #[serde(serialize_with="serialize_vertex_rows")]
    vertices:Vec<[f32;8]>,indices:Vec<u32>,
}
// Wire encoding only: memory/Deserialize stay f32; no color quantization.
struct BorrowedVertex<'a>(&'a [f32;8]);
impl serde::Serialize for BorrowedVertex<'_> {
    fn serialize<S:serde::Serializer>(&self,serializer:S)->Result<S::Ok,S::Error> {
        use serde::ser::SerializeTuple;
        let mut rgba=[0u8;4];
        // Validate ALL colors before emitting any vertex component, including
        // positive-zero identity: -0.0/fractional/out-of-range/NaN must fail.
        for (i,color) in rgba.iter_mut().enumerate() {
            let original=self.0[i+4];let exact=original as u8;
            if original.to_bits()!=(exact as f32).to_bits() {
                return Err(serde::ser::Error::custom("vertex RGBA must retain exact Color32 u8 bits"));
            }
            *color=exact;
        }
        let mut tuple=serializer.serialize_tuple(8)?;
        for component in &self.0[..4] {tuple.serialize_element(component)?;}
        for color in &rgba {tuple.serialize_element(color)?;}
        tuple.end()
    }
}
fn serialize_vertex_rows<S:serde::Serializer>(rows:&[[f32;8]],serializer:S)->Result<S::Ok,S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq=serializer.serialize_seq(Some(rows.len()))?;
    for row in rows {seq.serialize_element(&BorrowedVertex(row))?;}
    seq.end()
}
#[derive(serde::Serialize,serde::Deserialize)]
pub(super) struct GpuFrame {set:Vec<Upload>,meshes:Vec<Mesh>,free:Vec<u64>}
#[derive(serde::Serialize,serde::Deserialize,PartialEq)]
struct Scope {status:String,identity:Option<(u64,i16,u64)>,bracket:Option<(u64,u64,f64,f64)>,
    cue:Option<(u64,u64,u64)>,panel:[f32;4],clip:[f32;4]}
#[derive(serde::Serialize,serde::Deserialize)]
struct Frame {#[serde(flatten)]gpu:GpuFrame,index:u64,ui_pts_s:f64,visible:bool,
    processed_output_frames:usize,published_step:u64,internal_dt_s:f64,
    model_label:Option<String>,action:Option<Scope>}
#[derive(Default)]
struct Counter {bytes:usize}
impl std::io::Write for Counter {
    fn write(&mut self,bytes:&[u8])->std::io::Result<usize> {
        self.bytes=self.bytes.checked_add(bytes.len()).ok_or_else(||std::io::Error::other("count overflow"))?;Ok(bytes.len())
    }
    fn flush(&mut self)->std::io::Result<()> {Ok(())}
}
fn byte_count(value:&impl serde::Serialize)->usize {
    let mut count=Counter::default();serde_json::to_writer(&mut count,value).unwrap();count.bytes
}
pub(super) fn serialize_replay_frame(ctx:&Context,delta:&epaint::textures::TexturesDelta,output:Option<&FullOutput>)->GpuFrame {
    let id=|texture:TextureId|match texture {TextureId::Managed(id)=>id,TextureId::User(_)=>panic!("unsupported user texture")};
    let set=delta.set.iter().map(|(texture,delta)| {
        assert!(delta.options.mipmap_mode.is_none());
        assert_eq!(delta.options.wrap_mode,epaint::textures::TextureWrapMode::ClampToEdge);
        Upload {id:id(*texture),size:delta.image.size(),pos:delta.pos,
            mag:format!("{:?}",delta.options.magnification),min:format!("{:?}",delta.options.minification),
            wrap:format!("{:?}",delta.options.wrap_mode),
            rgba:match &delta.image {
                ImageData::Color(image)=>image.pixels.iter().map(|c|c.to_array()).collect(),
                ImageData::Font(image)=>image.srgba_pixels(None).map(|c|c.to_array()).collect(),
            }}
    }).collect();
    let mut meshes=Vec::new();let mut raw=0usize;
    if let Some(output)=output {
        assert_eq!(output.pixels_per_point,1.0);
        for primitive in ctx.tessellate(output.shapes.clone(),output.pixels_per_point) {
            assert!(primitive.clip_rect.is_finite());
            let mesh=match primitive.primitive {epaint::Primitive::Mesh(mesh)=>mesh,epaint::Primitive::Callback(_)=>panic!("unsupported native callback")};
            raw=raw.checked_add(mesh.vertices.len().checked_mul(32).unwrap()).and_then(|n|n.checked_add(mesh.indices.len().checked_mul(4)?)).unwrap();
            assert!(raw<=REPLAY_MESH_BYTE_CAP);
            let vertices:Vec<[f32;8]>=mesh.vertices.iter().map(|v| {
                assert!(v.pos.is_finite() && v.uv.is_finite());let c=v.color.to_array();
                [v.pos.x,v.pos.y,v.uv.x,v.uv.y,c[0] as f32,c[1] as f32,c[2] as f32,c[3] as f32]
            }).collect();
            assert!(mesh.indices.iter().all(|&i|(i as usize)<vertices.len()));
            let r=primitive.clip_rect;
            meshes.push(Mesh {texture:id(mesh.texture_id),clip:[r.min.x,r.min.y,r.max.x,r.max.y],vertices,indices:mesh.indices});
        }
        assert!(!meshes.is_empty());
    }
    GpuFrame {set,meshes,free:delta.free.iter().copied().map(id).collect()}
}
fn same_f32(a:&[f32],b:&[f32])->bool {a.len()==b.len() && a.iter().zip(b).all(|(a,b)|a.to_bits()==b.to_bits())}
fn same_gpu(a:&GpuFrame,b:&GpuFrame) {
    assert!(a.free==b.free,"actual free-command equality mismatch");assert_eq!(a.set.len(),b.set.len());assert_eq!(a.meshes.len(),b.meshes.len());
    for (a,b) in a.set.iter().zip(&b.set) {
        assert!((a.id,a.size,a.pos,&a.mag,&a.min,&a.wrap,&a.rgba)==(b.id,b.size,b.pos,&b.mag,&b.min,&b.wrap,&b.rgba),
            "actual upload/options/RGBA exact equality mismatch");
    }
    for (a,b) in a.meshes.iter().zip(&b.meshes) {
        assert_eq!(a.texture,b.texture);assert!(a.indices==b.indices,"actual mesh index exact equality mismatch");
        assert!(same_f32(&a.clip,&b.clip),"actual f32 CPU clip signed-bit roundtrip");
        assert_eq!(a.vertices.len(),b.vertices.len());
        for (a,b) in a.vertices.iter().zip(&b.vertices) {assert!(same_f32(a,b),"ALL actual f32 GPU components signed-bit roundtrip");}
    }
}
pub(super) fn real_gpu_roundtrip(frame:&GpuFrame) {
    let bytes=serde_json::to_vec(frame).unwrap();
    let restored:GpuFrame=serde_json::from_slice(&bytes).unwrap();same_gpu(frame,&restored);
}
fn real_frame_roundtrip(frame:&Frame) {
    let bytes=serde_json::to_vec(frame).unwrap();let restored:Frame=serde_json::from_slice(&bytes).unwrap();
    same_gpu(&frame.gpu,&restored.gpu);
    assert_eq!((frame.index,frame.visible,frame.processed_output_frames,frame.published_step,&frame.model_label),
        (restored.index,restored.visible,restored.processed_output_frames,restored.published_step,&restored.model_label));
    assert_eq!(frame.ui_pts_s.to_bits(),restored.ui_pts_s.to_bits());
    assert_eq!(frame.internal_dt_s.to_bits(),restored.internal_dt_s.to_bits());
    match (&frame.action,&restored.action) {
        (Some(a),Some(b))=>{
            assert_eq!((&a.status,a.identity,a.cue),(&b.status,b.identity,b.cue));
            assert!(same_f32(&a.panel,&b.panel) && same_f32(&a.clip,&b.clip));
            match (a.bracket,b.bracket) {
                (Some(a),Some(b))=>{assert_eq!((a.0,a.1),(b.0,b.1));assert_eq!(a.2.to_bits(),b.2.to_bits());assert_eq!(a.3.to_bits(),b.3.to_bits());},
                (None,None)=>{},_=>panic!("actual bracket presence changed"),
            }
        },
        (None,None)=>{},_=>panic!("actual scope presence changed"),
    }
}
#[derive(Default)]
pub(super) struct Capture {frames:Vec<Frame>,texture_bytes:usize,json_bytes:usize,visible:bool,
    visible_frames:usize,physics_frames:usize,engine_epoch_ui_s:Option<f64>}
impl Capture {
    pub(super) fn push(&mut self,ctx:&Context,output:&FullOutput,state:&State,index:u64) {
        assert_eq!(self.frames.len() as u64,index);assert!(self.frames.len()<HISTORY_CAP);
        assert!(output.textures_delta.set.len()<=1024 && output.textures_delta.free.len()<=1024);
        if self.visible {self.visible_frames+=1;assert!(self.visible_frames<=VISIBLE_CAP);}
        let bytes=output.textures_delta.set.iter().try_fold(0usize,|n,(_,d)| {let [w,h]=d.image.size();n.checked_add(w.checked_mul(h)?.checked_mul(4)?)}).unwrap();
        self.texture_bytes=self.texture_bytes.checked_add(bytes).unwrap();assert!(self.texture_bytes<=TEXTURE_BYTE_CAP);
        let gpu=serialize_replay_frame(ctx,&output.textures_delta,Some(output));
        let paint=ctx.data(|d|d.get_temp::<crate::editor::illustration::action::PaintTrace>(Id::new("action:paint")));
        fn model_label(shape:&Shape,labels:&mut Vec<String>) {
            match shape {Shape::Text(t) if t.galley.job.text.contains(" · effective let-off ")=>labels.push(t.galley.job.text.clone()),
                Shape::Vec(shapes)=>for s in shapes {model_label(s,labels);},_=>{}}
        }
        let mut labels=Vec::new();for s in &output.shapes {model_label(&s.shape,&mut labels);}assert!(labels.len()<=1);
        let action=paint.map(|p|Scope {status:p.status.into(),identity:p.identity,bracket:p.bracket,cue:p.cue_steps,
            panel:[p.panel.min.x,p.panel.min.y,p.panel.max.x,p.panel.max.y],clip:[p.clip.min.x,p.clip.min.y,p.clip.max.x,p.clip.max.y]});
        let frame=Frame {gpu,index,ui_pts_s:index as f64/60.0,visible:self.visible,processed_output_frames:self.physics_frames,
            published_step:state.snap.action.latest_step,internal_dt_s:state.snap.action.internal_dt_s,model_label:labels.pop(),action};
        let current=byte_count(&frame);self.json_bytes=self.json_bytes.checked_add(current).unwrap();
        let vertices:usize=frame.gpu.meshes.iter().map(|m|m.vertices.len()).sum();
        let indices:usize=frame.gpu.meshes.iter().map(|m|m.indices.len()).sum();
        eprintln!("ACTION_EXPORT_BUDGET index={} history={} visible={} meshes={} vertices={} indices={} raw_mesh={} rgba_current={} rgba_total={} json_current={} json_total={} json_cap={}",
            index,self.frames.len()+1,self.visible_frames,frame.gpu.meshes.len(),vertices,indices,
            vertices*32+indices*4,bytes,self.texture_bytes,current,self.json_bytes,JSON_CAP);
        assert!(self.json_bytes<=JSON_CAP,"typed frame JSON cap; no omission/fallback");
        real_frame_roundtrip(&frame);self.frames.push(frame);
    }
}
// Fixed selector-only UI export: existing Capture owns every GPU frame.
#[derive(serde::Serialize,serde::Deserialize,PartialEq)]
pub(super) struct SelectorMetadata {
    pub case:String,pub selector:String,pub surface:String,pub target_index:u64,
    pub setup_keys:Vec<String>,pub demo_titles:Vec<String>,pub selected_setup:String,
    pub selected_demo:usize,pub selected_demo_title:String,pub full_label:String,
    pub response_id_hex:String,pub response_rect:[f32;4],pub response_clip:[f32;4],
    pub row_id_hex:Option<String>,pub row_rect:Option<[f32;4]>,pub row_clip:Option<[f32;4]>,
    pub galley_text:String,pub galley_rect:[f32;4],pub galley_clip:[f32;4],
    pub selected_elided:bool,pub popup_open:bool,pub full_glyphs:bool,
}
#[derive(serde::Serialize)]
struct SelectorUiFrame<'a> {#[serde(flatten)]gpu:&'a GpuFrame,index:u64,ui_pts_s:f64}
#[derive(serde::Deserialize)]
struct SelectorUiRestored {#[serde(flatten)]gpu:GpuFrame,index:u64,ui_pts_s:f64}
struct SelectorUiFrames<'a>(&'a [Frame]);
impl serde::Serialize for SelectorUiFrames<'_> {
    fn serialize<S:serde::Serializer>(&self,serializer:S)->Result<S::Ok,S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq=serializer.serialize_seq(Some(self.0.len()))?;
        for frame in self.0 {seq.serialize_element(&SelectorUiFrame {gpu:&frame.gpu,index:frame.index,ui_pts_s:frame.ui_pts_s})?;}
        seq.end()
    }
}
#[derive(serde::Serialize)]
struct SelectorDocument<'a> {
    format:&'static str,revision:&'a str,mode:&'static str,#[serde(flatten)]logical:Logical,
    ui_clock_hz:usize,time_policy:&'static str,claim_boundary:&'static str,engine_playback_proof:bool,
    metadata:&'a SelectorMetadata,frames:SelectorUiFrames<'a>,texture_history:TextureHistory,
    counts:Counts,limits:Limits,
}
pub(super) struct SelectorSummary {pub frames:usize,pub target_index:u64,pub target_pts:f64,pub bytes:usize,pub texture_bytes:usize}
impl Capture {
    pub(super) fn write_selector_sequence(&self,revision:&str,logical_size:[f32;2],metadata:&SelectorMetadata,
        output:&mut impl std::io::Write)->SelectorSummary {
        assert!(revision.len()==40 && revision.bytes().all(|b|b.is_ascii_hexdigit()),"selector revision binding");
        assert!(same_f32(&logical_size,&[800.0,600.0]),"fixed selector800 logical size");
        assert!(!self.frames.is_empty() && self.frames.len()<=160 && self.frames.len()<=HISTORY_CAP,"all selector UI draw cap");
        assert!(self.physics_frames==0 && self.engine_epoch_ui_s.is_none() && !self.visible,"UI-only selector capture, no faux engine epoch");
        let last=self.frames.last().unwrap();assert_eq!(metadata.target_index,last.index,"metadata must be same LAST actual captured frame");
        let cases=[("preset-closed","preset","closed"),("demo-closed","demo","closed"),
            ("preset-hover","preset","hover"),("demo-hover","demo","hover"),
            ("preset-popup","preset","popup"),("demo-popup","demo","popup")];
        assert!(cases.iter().any(|&(a,b,c)|metadata.case==a && metadata.selector==b && metadata.surface==c),"fixed six selector cases");
        assert!(metadata.setup_keys.len()==2 && metadata.demo_titles.len()==2 && metadata.selected_demo<2,"actual two-item catalog");
        assert!(metadata.selected_demo_title==metadata.demo_titles[metadata.selected_demo],"actual selected demo title/index");
        if metadata.selector=="preset" {assert!(metadata.selected_setup==metadata.setup_keys[1],"actual selected full setup key");}
        else {assert_eq!(metadata.selected_demo,1,"actual selected demo index");}
        let name=if metadata.selector=="preset" {&metadata.selected_setup} else {&metadata.selected_demo_title};
        let expected=if metadata.surface=="popup" && metadata.selector=="preset" {format!("Saved · {name}")} else {name.clone()};
        assert!(metadata.full_label==expected && metadata.galley_text==expected,"actual FULL target job text");
        assert!(metadata.setup_keys.iter().chain(&metadata.demo_titles).all(|s|s.len()<=512) && metadata.full_label.len()<=512,"fixed catalog text bounds");
        assert_eq!(metadata.row_id_hex.is_some(),metadata.surface=="popup","actual row identity presence");
        assert_eq!(metadata.row_rect.is_some(),metadata.surface=="popup");assert_eq!(metadata.row_clip.is_some(),metadata.surface=="popup");
        let is_hex=|s:&str|s.len()==16 && s.bytes().all(|b|b.is_ascii_hexdigit());
        assert!(is_hex(&metadata.response_id_hex) && metadata.row_id_hex.as_ref().is_none_or(|s|is_hex(s)),"exact selector Id hex");
        assert!(metadata.response_rect.iter().chain(&metadata.response_clip).chain(&metadata.galley_rect)
            .chain(&metadata.galley_clip).all(|v|v.is_finite()),"actual finite selector scope");
        assert!(metadata.row_rect.as_ref().is_none_or(|a|a.iter().all(|v|v.is_finite())) &&
            metadata.row_clip.as_ref().is_none_or(|a|a.iter().all(|v|v.is_finite())),"actual finite row scope");
        assert!(metadata.surface=="closed" && metadata.selected_elided && !metadata.popup_open && !metadata.full_glyphs ||
            metadata.surface=="hover" && metadata.selected_elided && !metadata.popup_open && metadata.full_glyphs ||
            metadata.surface=="popup" && metadata.popup_open && metadata.full_glyphs,"actual surface/elision semantics");
        let restored:SelectorMetadata=serde_json::from_slice(&serde_json::to_vec(metadata).unwrap()).unwrap();
        assert!(metadata==&restored,"ALL selector strings/identity/flags exact roundtrip");
        for (a,b) in [(&metadata.response_rect,&restored.response_rect),(&metadata.response_clip,&restored.response_clip),
            (&metadata.galley_rect,&restored.galley_rect),(&metadata.galley_clip,&restored.galley_clip)] {
            assert!(same_f32(a,b),"ALL selector scope f32 signed bits");
        }
        for (a,b) in [(&metadata.row_rect,&restored.row_rect),(&metadata.row_clip,&restored.row_clip)] {
            match (a,b) {(Some(a),Some(b))=>assert!(same_f32(a,b),"ALL row f32 signed bits"),(None,None)=>{},_=>panic!("row presence changed")}
        }
        for (index,frame) in self.frames.iter().enumerate() {
            assert_eq!(frame.index,index as u64,"ALL actual contiguous UI draws");
            assert_eq!(frame.ui_pts_s.to_bits(),(index as f64/60.0).to_bits(),"ALL original UI PTS bits");
            let view=SelectorUiFrame {gpu:&frame.gpu,index:frame.index,ui_pts_s:frame.ui_pts_s};
            let restored:SelectorUiRestored=serde_json::from_slice(&serde_json::to_vec(&view).unwrap()).unwrap();
            same_gpu(&frame.gpu,&restored.gpu);assert_eq!(frame.index,restored.index);
            assert_eq!(frame.ui_pts_s.to_bits(),restored.ui_pts_s.to_bits(),"emitted UI PTS exact bits");
        }
        let logical=Logical {logical_size,pixels_per_point:1.0};logical_roundtrip(&logical);
        let document=SelectorDocument {format:"actual-egui-selector-sequence-v1",revision,mode:"Standalone",logical,
            ui_clock_hz:60,time_policy:"EVERY actual RawInput/UI draw and PTS; UNTIMED raster target, no copied holds",
            claim_boundary:"UNTIMED actual testcatalog UI raster; NOT native/window/engine/audio/video proof",
            engine_playback_proof:false,metadata,frames:SelectorUiFrames(&self.frames),
            texture_history:TextureHistory {recorded_before_first_frame:true,rgba_bytes:self.texture_bytes},
            counts:Counts {visible:self.frames.len(),history:self.frames.len() as u64,physics_frames:0},
            limits:Limits {visible:160,history:HISTORY_CAP,texture_bytes:TEXTURE_BYTE_CAP,
                mesh_bytes_per_frame:REPLAY_MESH_BYTE_CAP,json_bytes:JSON_CAP,total_bytes:TOTAL_CAP}};
        let bytes=byte_count(&document);assert!(bytes<=JSON_CAP,"whole selector JSON cap");
        let mut writer=Writer {inner:output,bytes:0,remaining_total:JSON_CAP};
        serde_json::to_writer(&mut writer,&document).expect("complete bounded typed selector sequence");
        std::io::Write::flush(&mut writer).unwrap();assert_eq!(writer.bytes,bytes,"whole selector output count");
        SelectorSummary {frames:self.frames.len(),target_index:last.index,target_pts:last.ui_pts_s,bytes,texture_bytes:self.texture_bytes}
    }
}

fn action_header(f:&Fixture)->Rect {
    let headers=f.ctx.data(|data|data.get_temp::<Vec<(String,Rect,Rect)>>(Id::new("compact:widgets"))).unwrap();
    let found:Vec<_>=headers.iter().filter(|(name,_,_)|name=="category:1").collect();
    assert_eq!(found.len(),1);found[0].1
}
fn prepare(size:Vec2)->(crate::PhysicalPiano,Fixture) {
    let mut p=crate::PhysicalPiano::with_demos(||Arc::new(Vec::new()));
    assert!(p.initialize_engine(48000.0,512));
    assert!(p.shared.display.lock().unwrap().view.is_some());
    let mut f=Fixture::from_shared(size,p.params.clone(),p.shared.clone(),p.demos.clone());
    f.replay=Some(Capture::default()); // before settle/click/any Context.run
    f.settle();f.click_at(action_header(&f).center());f.settle();
    let paint=f.ctx.data(|data|data.get_temp::<crate::editor::illustration::action::PaintTrace>(Id::new("action:paint"))).unwrap();
    f.click_at(paint.panel.left_top()+vec2(5.0,5.0));f.settle();
    assert!(p.shared.inputs.is_empty());f.host.assert_no_edits();
    f.replay.as_mut().unwrap().visible=true;(p,f)
}
fn process(p:&mut crate::PhysicalPiano,f:&mut Fixture,mut count:usize,until_publish:&mut usize,cap:usize) {
    let mut left=[0.0;512];let mut right=[0.0;512];
    while count>0 {
        let n=count.min(512).min(*until_publish);
        p.built.as_mut().unwrap().piano.process(&mut left[..n],&mut right[..n]);
        assert!(left[..n].iter().chain(&right[..n]).all(|v|v.is_finite()));
        let replay=f.replay.as_mut().unwrap();replay.physics_frames+=n;
        assert!(replay.physics_frames<=cap);
        count-=n;*until_publish-=n;
        if *until_publish==0 {assert!(p.try_publish_snapshot());*until_publish=2048;}
    }
}
fn pause(p:&crate::PhysicalPiano,f:&mut Fixture) {
    let publications=p.shared.display_frames.load(Ordering::Acquire);
    for _ in 0..60 {f.frame(Vec::new());} // ACTUAL static State.show draws, not copied holds
    assert_eq!(p.shared.display_frames.load(Ordering::Acquire),publications);
    f.host.assert_no_edits();assert!(p.shared.inputs.is_empty());
}
fn single(p:&mut crate::PhysicalPiano,f:&mut Fixture)->Vec<InputRecord> {
    let mut events=Vec::new();
    for pressed in [true,false] {
        events.push(InputRecord::Single {boundary:"actual RawInput key→State.show queue→production drain".into(),
            ui_pts_s:f.frame as f64/60.0,output_frame:0,key:"A".into(),pressed});
        f.frame(vec![Event::Key {key:Key::A,physical_key:None,pressed,repeat:false,modifiers:Modifiers::default()}]);
        assert!(!p.shared.inputs.is_empty());p.drain_editor_inputs();assert!(p.shared.inputs.is_empty());
        assert_eq!(p.shared.held[60].load(Ordering::Relaxed)&FROM_UI,if pressed {FROM_UI} else {0});
    }
    f.replay.as_mut().unwrap().engine_epoch_ui_s=Some((f.frame-1) as f64/60.0);
    let mut until_publish=2048;
    for _ in 0..14 {process(p,f,800,&mut until_publish,12000);f.frame(Vec::new());}
    assert_eq!(f.replay.as_ref().unwrap().physics_frames,11200);pause(p,f);events
}
fn dense(p:&mut crate::PhysicalPiano,f:&mut Fixture)->Vec<InputRecord> {
    f.replay.as_mut().unwrap().engine_epoch_ui_s=Some((f.frame-1) as f64/60.0);
    let mut events=Vec::new();let (mut position,mut note,mut ui_tick,mut until_publish)=(0usize,0usize,1usize,2048usize);
    while position<8192 {
        if note<8 && position==note*960 {
            // Genuine real-model/production-input calls at20ms engine boundaries;
            // NOT invented RawInput keys or silently retimed60Hz key frames.
            let midi=if note%2==0 {60} else {62};
            p.input(Input::NoteOn {midi,velocity:80,from:FROM_UI});
            p.input(Input::NoteOff {midi,from:FROM_UI,release:1.0});
            events.push(InputRecord::Dense {boundary:"scripted production-input, NOT UI key delivery".into(),
                output_frame:position,engine_output_pts_s:position as f64/48000.0,
                midi,velocity:80,release:1.0});note+=1;
        }
        let next_note=if note<8 {note*960} else {8192};
        let next=next_note.min(ui_tick*800).min(8192);
        assert!(next>position);process(p,f,next-position,&mut until_publish,8192);position=next;
        if position==ui_tick*800 {f.frame(Vec::new());ui_tick+=1;}
    }
    assert_eq!(note,8);assert_eq!(position,8192);pause(p,f);events
}
fn verify_capture(capture:&Capture,scenario:&str) {
    let visible:Vec<_>=capture.frames.iter().filter(|f|f.visible).collect();
    let measured=|f:&&Frame|f.action.as_ref().is_some_and(|p|p.status.starts_with("Measured "));
    assert!(visible.iter().any(measured));
    assert!(visible.iter().any(|f|f.action.as_ref().is_some_and(|p|p.status.starts_with("Paused "))));
    if scenario=="single" {
        assert!(visible.iter().any(|f|f.action.as_ref().is_some_and(|p|p.cue.is_some())));
        assert!(visible.iter().any(|f|f.action.as_ref().is_some_and(|p|p.status.starts_with("Measured return"))));
    } else {
        let mut ids=Vec::new();let mut labels=Vec::new();
        for frame in visible {
            if frame.processed_output_frames<6720 && frame.action.as_ref().is_some_and(|p|p.status.starts_with("Measured ")) {
                let (_,midi,id)=frame.action.as_ref().unwrap().identity.unwrap();
                if !ids.contains(&id) {ids.push(id);}
                let name=note_name(midi as i32);
                assert!(frame.model_label.as_ref().is_some_and(|label|label.starts_with(&format!("{name} · "))));
                if !labels.contains(&midi) {labels.push(midi);}
            }
        }
        assert!(ids.len()>=2 && labels.len()>=2);
    }
}
struct Writer<W> {inner:W,bytes:usize,remaining_total:usize}
impl<W:std::io::Write> std::io::Write for Writer<W> {
    fn write(&mut self,buffer:&[u8])->std::io::Result<usize> {
        if buffer.len()>JSON_CAP.saturating_sub(self.bytes).min(self.remaining_total) {
            return Err(std::io::Error::other("hard trajectory JSON/total cap"));
        }
        let n=self.inner.write(buffer)?;self.bytes+=n;self.remaining_total-=n;Ok(n)
    }
    fn flush(&mut self)->std::io::Result<()> {self.inner.flush()}
}
#[test]
#[ignore="explicit approved four-sequence OFFLINE-engine export only; NOT runtime authority"]
fn export_four_actual_action_trajectories() {
    use std::os::unix::fs::OpenOptionsExt;
    let directory=std::env::var_os("FELT_ACTION_REPLAY_OUTPUT_DIR").expect("explicit private scratch directory");
    let directory=std::path::Path::new(&directory);assert!(directory.is_absolute());
    use std::os::unix::fs::MetadataExt;
    // POSIX effective UID query only; no child process, ambient UID or new dependency.
    extern "C" {fn geteuid()->u32;}
    let uid=unsafe {geteuid()};
    let metadata=std::fs::symlink_metadata(directory).expect("private output directory metadata");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.uid(),uid,"output directory must be owned by this process user");
    assert_eq!(metadata.mode()&0o7777,0o700,"private output directory must already be0700");
    assert_eq!(std::fs::canonicalize(directory).unwrap(),directory,"canonical path, no symlink/relative components");
    assert!(std::fs::read_dir(directory).unwrap().next().is_none(),"fresh-empty output directory before ANY writes");
    let directory_identity=(metadata.dev(),metadata.ino());
    let revision=std::env::var("FELT_ACTION_REPLAY_REV").expect("exact approved export source revision");
    assert!(revision.len()==40 && revision.bytes().all(|b|b.is_ascii_hexdigit()));
    let _mode=EditorModeGuard::new(true);let mut total=0usize;let mut physics=0usize;let mut builds=0usize;
    for (viewport,size) in [("1280x860",vec2(1280.0,860.0)),("800x600",vec2(800.0,600.0))] {
        for scenario in ["single","dense"] {
            let (mut p,mut f)=prepare(size);builds+=1;
            let events=if scenario=="single" {single(&mut p,&mut f)} else {dense(&mut p,&mut f)};
            let capture=f.replay.take().unwrap();verify_capture(&capture,scenario);real_inputs_roundtrip(&events,capture.engine_epoch_ui_s);
            assert_eq!(capture.frames.len() as u64,f.frame);
            physics+=capture.physics_frames;assert!(physics<=40384 && builds<=4);
            let logical=Logical {logical_size:[size.x,size.y],pixels_per_point:1.0};logical_roundtrip(&logical);
            let document=Document {format:"actual-egui-trajectory-v1",revision:&revision,
                label:"OFFLINE ENGINE→actual State.show replay; NOT native interaction/audio",
                scenario,viewport,logical,
                input_boundary:if scenario=="single" {"actual RawInput key→queue→production drain"} else {"scripted20ms production-input; separate60Hz State.show"},
                time_policy:"original UI PTS1x; NO resampling/time-stretch/copied holds",
                ui_clock_hz:60,engine_sample_rate:48000,engine_epoch_ui_s:capture.engine_epoch_ui_s,
                clock_boundary:"declared offline-engine/UI epochs, NOT measured AV latency or native scheduling",
                events:&events,frames:&capture.frames,
                texture_history:TextureHistory {recorded_before_first_frame:true,rgba_bytes:capture.texture_bytes},
                counts:Counts {visible:capture.visible_frames,history:f.frame,physics_frames:capture.physics_frames},
                limits:Limits {visible:VISIBLE_CAP,history:HISTORY_CAP,texture_bytes:TEXTURE_BYTE_CAP,
                    mesh_bytes_per_frame:REPLAY_MESH_BYTE_CAP,json_bytes:JSON_CAP,total_bytes:TOTAL_CAP}};
            let document_bytes=byte_count(&document);
            eprintln!("ACTION_EXPORT_DOCUMENT history={} visible={} physics={} rgba={} json_bytes={} json_cap={} batch_before={} batch_cap={}",
                f.frame,capture.visible_frames,capture.physics_frames,capture.texture_bytes,document_bytes,JSON_CAP,total,TOTAL_CAP);
            assert!(document_bytes<=JSON_CAP && document_bytes<=TOTAL_CAP-total);
            let current=std::fs::symlink_metadata(directory).unwrap();
            assert!(current.is_dir() && !current.file_type().is_symlink());
            assert_eq!((current.dev(),current.ino()),directory_identity,"owned output directory must not be replaced");
            assert_eq!(current.uid(),uid);assert_eq!(current.mode()&0o7777,0o700);
            assert_eq!(std::fs::canonicalize(directory).unwrap(),directory);
            let path=directory.join(format!("action-{scenario}-{viewport}.json"));
            let file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).expect("NEW private export, no overwrite");
            let mut writer=Writer {inner:std::io::BufWriter::new(file),bytes:0,remaining_total:TOTAL_CAP-total};
            serde_json::to_writer(&mut writer,&document).expect("complete bounded actual trajectory");
            std::io::Write::flush(&mut writer).unwrap();total+=writer.bytes;assert!(total<=TOTAL_CAP);
        }
    }
    assert_eq!(builds,4);assert_eq!(physics,38784);
}
