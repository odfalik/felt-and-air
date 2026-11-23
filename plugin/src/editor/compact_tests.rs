//! TEST-ONLY compact reachability gates using the parent's ACTUAL editor Fixture.
//! Apply on an isolated before/after branch. No native resize/input/occlusion claim.
use super::*;
use std::cell::RefCell;

const SIZES: [Vec2; 4] = [vec2(800.0, 600.0), vec2(1000.0, 650.0), vec2(1280.0, 720.0), vec2(1280.0, 860.0)];
const ROUNDING: f32 = 1.0; // logical-point tolerance, not partial-control visibility.

fn fits_x(rect: Rect, visible: Rect) -> bool {
    rect.is_finite() && visible.is_finite() && rect.width() > 0.0
        && visible.width() > 0.0 && rect.left() >= visible.left() - ROUNDING
        && rect.right() <= visible.right() + ROUNDING
}
fn fits(rect: Rect, visible: Rect) -> bool {
    fits_x(rect, visible) && rect.height() > 0.0 && visible.height() > 0.0
        && rect.top() >= visible.top() - ROUNDING && rect.bottom() <= visible.bottom() + ROUNDING
}
fn category(f: &mut Fixture, tab: usize) {
    f.state.tab = tab;
    f.state.active_param = PARAMS.iter().position(|p| p.tab == tab).unwrap();
    f.state.frame = u64::MAX;
    f.settle();
}

#[test]
fn actual_compact_keyboard_and_control_widths_fit_without_category_anchor_drift() {
    let failures = RefCell::new(Vec::new());
    both_editor_modes(|| {
        for size in SIZES {
            let mut f = Fixture::new(size);
            let mut anchor = None;
            for tab in 0..guide::TABS.len() {
                category(&mut f, tab);
                let label = format!("api={:?} size={size:?} category={}", f.host.api, guide::TABS[tab]);
                let keyboard = f.ctx.data(|d| d.get_temp::<Rect>(Id::new("editor:keyboard")));
                if let Some(rect) = keyboard {
                    println!("COMPACT actual keyboard {label} rect={rect:?}");
                    if !rect.is_finite() || rect.height() != 90.0 || !f.screen().contains_rect(rect) {
                        failures.borrow_mut().push(format!("{label}: whole 90pt keyboard outside screen: {rect:?}"));
                    }
                    if let Some(previous) = anchor {
                        if rect != previous { failures.borrow_mut().push(format!("{label}: category anchor moved from {previous:?} to {rect:?}")); }
                    } else { anchor = Some(rect); }
                } else { failures.borrow_mut().push(format!("{label}: no fresh actual keyboard response")); }
                // Header rails/resets are outside the sidebar; do not intersect them
                // with its viewport or silently omit the header's compact-width risk.
                for (kind, name) in [("control", "Volume"), ("reset", "Volume"), ("reset", "Board grid")] {
                    match f.trace(kind, name) {
                        Some((rect, clip, _)) if fits(rect, clip.intersect(f.screen())) => {}
                        Some((rect, clip, _)) => failures.borrow_mut().push(format!("{label}: header {kind} {name} clipped: rect={rect:?}, visible={:?}", clip.intersect(f.screen()))),
                        None => failures.borrow_mut().push(format!("{label}: header {kind} {name} has no fresh response")),
                    }
                }
                let (viewport, _, _) = f.sidebar();
                for kind in ["control", "reset"] {
                    let mut bad = Vec::new();
                    for p in PARAMS.iter().filter(|p| p.tab == tab) {
                        match f.trace(kind, p.name) {
                            Some((rect, clip, _)) if fits_x(rect, clip.intersect(viewport).intersect(f.screen())) => {}
                            Some((rect, clip, _)) => bad.push(format!("{} rect={rect:?} visible={:?}", p.name, clip.intersect(viewport).intersect(f.screen()))),
                            None => bad.push(format!("{} missing actual response", p.name)),
                        }
                    }
                    if !bad.is_empty() { failures.borrow_mut().push(format!("{label}: horizontally clipped {kind}: {}", bad.join("; "))); }
                }
                f.host.assert_no_edits();
            }
        }
    });
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "Actual compact reachability failures (not native claims):\n{}", failures.join("\n"));
}

#[test]
fn actual_compact_last_control_and_reset_are_reachable_at_real_scroll_end() {
    let failures = RefCell::new(Vec::new());
    both_editor_modes(|| {
        for size in SIZES {
            let mut f = Fixture::new(size);
            for tab in 0..guide::TABS.len() {
                category(&mut f, tab);
                let label = format!("api={:?} size={size:?} category={}", f.host.api, guide::TABS[tab]);
                // Determine the LAST rendered response from actual geometry, not a copied
                // parameter ordering assumption (Room switches are drawn first).
                let last = PARAMS.iter().filter(|p| p.tab == tab)
                    .filter_map(|p| f.trace("control", p.name).map(|(rect, _, _)| (p.name, rect.bottom())))
                    .max_by(|a, b| a.1.total_cmp(&b.1)).map(|p| p.0);
                let Some(last) = last else { failures.borrow_mut().push(format!("{label}: no actual parameter responses")); continue; };
                for _ in 0..12 {
                    let (viewport, content, offset) = f.sidebar();
                    let max_offset = (content.y - viewport.height()).max(0.0);
                    if offset.y >= max_offset - ROUNDING { break; }
                    let visible = viewport.intersect(f.screen());
                    if visible.width() <= 0.0 || visible.height() <= 0.0 {
                        failures.borrow_mut().push(format!("{label}: no on-screen sidebar wheel target {viewport:?}"));
                        break;
                    }
                    f.wheel(visible.center(), MouseWheelUnit::Point, 1200.0);
                    f.host.assert_no_edits();
                }
                let (viewport, content, offset) = f.sidebar();
                println!("COMPACT actual end {label} last={last} viewport={viewport:?} content={content:?} offset={offset:?}");
                for kind in ["control", "reset"] {
                    match f.trace(kind, last) {
                        Some((rect, clip, _)) if fits(rect, clip.intersect(viewport).intersect(f.screen())) => {}
                        Some((rect, clip, _)) => failures.borrow_mut().push(format!("{label}: final {kind} {last} not fully reachable: rect={rect:?}, visible={:?}", clip.intersect(viewport).intersect(f.screen()))),
                        None => failures.borrow_mut().push(format!("{label}: final {kind} {last} has no fresh response")),
                    }
                }
                if offset.y > (content.y - viewport.height()).max(0.0) + ROUNDING {
                    failures.borrow_mut().push(format!("{label}: offset outside actual content range"));
                }
                f.host.assert_no_edits();
            }
        }
    });
    let failures = failures.into_inner();
    assert!(failures.is_empty(), "Actual compact final-control failures (not native claims):\n{}", failures.join("\n"));
}

#[test]
fn actual_compact_header_and_footer_widgets_are_fully_reachable() {
    let failures=RefCell::new(Vec::new());
    both_editor_modes(|| {
        for size in SIZES {
            let mut f=Fixture::new(size);
            for tab in 0..guide::TABS.len() {
                category(&mut f,tab);
                let label=format!("api={:?} size={size:?} category={}",f.host.api,guide::TABS[tab]);
                let widgets=f.ctx.data(|data|data.get_temp::<Vec<(String,Rect,Rect)>>(Id::new("compact:widgets")))
                    .expect("fresh actual header/footer traces from State.show");
                let mut required=vec!["header:title","header:preset","header:save","header:demo",
                    "header:play","header:elapsed","header:grid","header:diagnostics",
                    "footer:sustain","footer:soft","footer:sostenuto","footer:settings",
                    "footer:keyboard","footer:help","category:0","category:1","category:2","category:3"];
                if STANDALONE.load(Ordering::Relaxed) {required.push("header:sound");}
                for name in required {
                    let observed:Vec<_>=widgets.iter().filter(|(key,_,_)|key==name).collect();
                    if observed.len()!=1 {
                        failures.borrow_mut().push(format!("{label}: {name} expected one fresh response, got {}",observed.len()));
                    }
                }
                for (name,rect,clip) in &widgets {
                    let visible=clip.intersect(f.screen());
                    if !fits(*rect,visible) {
                        failures.borrow_mut().push(format!("{label}: full widget {name} clipped: rect={rect:?}, visible={visible:?}"));
                    }
                }
                // Both header factory resets and the actual Volume rail require
                // complete X AND Y containment, including wrapped header rows.
                for (kind,name) in [("control","Volume"),("reset","Volume"),("reset","Board grid")] {
                    match f.trace(kind,name) {
                        Some((rect,clip,_)) if fits(rect,clip.intersect(f.screen()))=>{},
                        Some((rect,clip,_))=>failures.borrow_mut().push(format!("{label}: full header {kind} {name} clipped: {rect:?} in {clip:?}")),
                        None=>failures.borrow_mut().push(format!("{label}: no fresh header {kind} {name} response")),
                    }
                }
                f.host.assert_no_edits();
            }
        }
    });
    let failures=failures.into_inner();
    assert!(failures.is_empty(),"Actual compact header/footer reachability failures (not native claims):\n{}",failures.join("\n"));
}
