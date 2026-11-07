//! Visual-only motion for pane layout changes (split, close, move, swap, zoom).
//!
//! Layout always jumps to its final state, so panes keep their cached frames and
//! nothing re-lays out per frame. Each pane gets a spring-driven "presented" rect
//! that travels from where it was drawn toward its final bounds, and an overlay
//! painted over the panes fakes the card being there: background covers the part
//! of the final bounds the card hasn't reached, and flat card colour fills where it
//! hangs past them.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

use gpui::App;
use gpui::Bounds;
use gpui::BoxShadow;
use gpui::ContentMask;
use gpui::Corners;
use gpui::Decay;
use gpui::DispatchPhase;
use gpui::EntityId;
use gpui::Hsla;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::MouseUpEvent;
use gpui::Pixels;
use gpui::Styled;
use gpui::canvas;
use gpui::fill;
use gpui::hsla;
use gpui::motion;
use gpui::point;
use gpui::px;
use smallvec::SmallVec;
use ui::prelude::ActiveTheme;

use crate::CARD_RADIUS;

const MOTION: Decay = motion::GLIDE;
/// A new pane grows from this fraction of its final size, about its center.
const EXPAND_FROM: f32 = 0.85;
/// Decay never reaches its target; half a logical pixel is a device pixel on Retina.
const SETTLE_EPSILON: f32 = 0.5;
const MAX_FRAME: Duration = Duration::from_millis(33);
const MIN_PIECE: Pixels = px(0.5);
/// A raised card's shadow is at full strength until it is this close to landing.
const LIFT_DISTANCE: f32 = 32.;
const SHADOW_BLUR: Pixels = px(16.);
const SHADOW_OFFSET_Y: Pixels = px(6.);
/// Dark themes put dark cards next to dark cards, so the shadow has to be strong to read.
const SHADOW_ALPHA: f32 = 0.45;

#[derive(Clone, Copy, Debug)]
struct Track {
    id: EntityId,
    target: Bounds<Pixels>,
    /// Presented left, top, right, bottom.
    edges: [f32; 4],
    /// Painted above other cards (new or moved panes), with a shadow.
    raised: bool,
    /// Height of the transparent tab strip above the rounded card body.
    strip: Pixels,
}

impl Track {
    fn new(id: EntityId, from: Bounds<Pixels>, target: Bounds<Pixels>, strip: Pixels, raised: bool) -> Self {
        Self { id, target, edges: edges(from), raised, strip }
    }

    fn presented(&self) -> Bounds<Pixels> {
        let [left, top, right, bottom] = self.edges.map(px);
        Bounds::from_corners(point(left, top), point(right.max(left), bottom.max(top)))
    }

    /// The presented card body: below the tab strip, rounded.
    fn body(&self) -> Bounds<Pixels> {
        let presented = self.presented();
        let top = (presented.top() + self.strip).min(presented.bottom());
        Bounds::from_corners(point(presented.left(), top), presented.bottom_right())
    }

    /// How lifted the card looks: 1 while far from its target, easing to 0 as it
    /// lands, so its shadow fades instead of popping off on settle.
    fn lift(&self) -> f32 {
        let distance = self.edges.iter().zip(edges(self.target)).map(|(edge, target)| (edge - target).abs());
        (distance.fold(0., f32::max) / LIFT_DISTANCE).min(1.)
    }

    /// Advances by the frame's shared `MOTION.remaining` factor.
    fn step(&mut self, remaining: f32) -> bool {
        let mut active = false;
        for (edge, target) in self.edges.iter_mut().zip(edges(self.target)) {
            *edge = target + (*edge - target) * remaining;
            if (*edge - target).abs() <= SETTLE_EPSILON {
                *edge = target;
            } else {
                active = true;
            }
        }
        if !active {
            self.raised = false;
        }
        active
    }
}

fn edges(bounds: Bounds<Pixels>) -> [f32; 4] {
    [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()].map(f32::from)
}

fn scaled_about_center(bounds: Bounds<Pixels>, scale: f32) -> Bounds<Pixels> {
    let size = gpui::size(bounds.size.width * scale, bounds.size.height * scale);
    Bounds::new(bounds.center() - point(size.width / 2., size.height / 2.), size)
}

/// `rect` minus `hole`, as up to four non-overlapping pieces.
fn subtract(rect: Bounds<Pixels>, hole: Bounds<Pixels>) -> SmallVec<[Bounds<Pixels>; 4]> {
    let mut pieces = SmallVec::new();
    let hole = rect.intersect(&hole);
    if hole.size.width < MIN_PIECE || hole.size.height < MIN_PIECE {
        pieces.push(rect);
        return pieces;
    }
    let (left, top, right, bottom) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    for piece in [
        Bounds::from_corners(point(left, top), point(right, hole.top())),
        Bounds::from_corners(point(left, hole.bottom()), point(right, bottom)),
        Bounds::from_corners(point(left, hole.top()), point(hole.left(), hole.bottom())),
        Bounds::from_corners(point(hole.right(), hole.top()), point(right, hole.bottom())),
    ] {
        if piece.size.width >= MIN_PIECE && piece.size.height >= MIN_PIECE {
            pieces.push(piece);
        }
    }
    pieces
}

/// What to paint over the panes this frame.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct PaneOverlay {
    /// Background over final bounds the card hasn't reached yet.
    pub covers: Vec<Bounds<Pixels>>,
    /// Cards presented outside their final bounds, bottom to top.
    pub ghosts: Vec<Ghost>,
}

/// A card's silhouette (tab strip in backdrop, rounded body in card colour) and
/// its shadow, each painted only inside its visible pieces.
#[derive(Debug, PartialEq)]
pub(crate) struct Ghost {
    pub presented: Bounds<Pixels>,
    pub body: Bounds<Pixels>,
    /// Where the silhouette shows: outside the card's final bounds, not occluded.
    pub pieces: SmallVec<[Bounds<Pixels>; 4]>,
    /// Where the shadow shows: outside the card, not occluded. Empty unless raised.
    pub shadow: SmallVec<[Bounds<Pixels>; 4]>,
    pub lift: f32,
}

#[derive(Default)]
pub struct PaneMotion {
    recorded: Vec<(EntityId, Bounds<Pixels>, Pixels)>,
    tracks: Vec<Track>,
    region: Option<Bounds<Pixels>>,
    /// The last frame showed one pane zoomed in place of the group.
    zoomed: bool,
    last_frame: Option<Instant>,
    /// The pointer is down: divider drags and other direct manipulation never animate.
    pub(crate) pointer_pressed: bool,
}

impl PaneMotion {
    /// Records a pane's final bounds for this frame, in paint order.
    pub(crate) fn record(&mut self, id: EntityId, bounds: Bounds<Pixels>, strip: Pixels) {
        self.recorded.push((id, bounds, strip));
    }

    /// Makes the next frame snap, for when the group is shown again (mode switch):
    /// only layout changes made while it is visible animate.
    pub(crate) const fn snap_next(&mut self) {
        self.region = None;
    }

    /// Consumes this frame's recorded panes and advances to `now`. Returns whether
    /// anything is still moving. A region change (window resize, dock toggle) or a
    /// pressed pointer snaps everything; so does the first frame. Zooming in or out
    /// (`zoomed` flips) animates even though the region changes with it.
    pub(crate) fn update(&mut self, region: Bounds<Pixels>, zoomed: bool, now: Instant) -> bool {
        let zoom_changed = self.zoomed != zoomed;
        self.zoomed = zoomed;
        let snap = self.region.is_none() || (self.region != Some(region) && !zoom_changed) || self.pointer_pressed;
        self.region = Some(region);
        let unchanged = self.recorded.len() == self.tracks.len()
            && self
                .recorded
                .iter()
                .zip(&self.tracks)
                .all(|((id, bounds, _), track)| *id == track.id && *bounds == track.target);
        self.recorded_into_tracks(unchanged, snap, zoom_changed);

        // Settled tracks sit exactly on their targets, so a settled, unchanged
        // layout (the common redraw) has nothing to step.
        if unchanged && self.last_frame.is_none() {
            return false;
        }
        let elapsed = self
            .last_frame
            .and_then(|last| now.checked_duration_since(last))
            .unwrap_or_default()
            .min(MAX_FRAME)
            .as_secs_f32();
        let remaining = MOTION.remaining(elapsed);

        let mut active = false;
        for track in &mut self.tracks {
            active |= track.step(remaining);
        }
        self.last_frame = active.then_some(now);
        active
    }

    /// Reconciles this frame's recorded panes with the tracks, keeping the
    /// recording buffer's capacity. On a zoom change the zoomed pane travels raised
    /// between its rects, and panes it covered or uncovered just vanish or appear.
    fn recorded_into_tracks(&mut self, unchanged: bool, snap: bool, zoom_changed: bool) {
        if unchanged {
            for ((_, _, strip), track) in self.recorded.drain(..).zip(&mut self.tracks) {
                track.strip = strip;
                if snap {
                    *track = Track::new(track.id, track.target, track.target, strip, false);
                }
            }
            return;
        }

        let mut previous = std::mem::take(&mut self.tracks);
        let mut removed: Vec<Track> = Vec::new();
        previous.retain(|track| {
            let keep = self.recorded.iter().any(|(id, _, _)| *id == track.id);
            if !keep {
                removed.push(*track);
            }
            keep
        });
        let mut removed = removed.into_iter();

        self.tracks = self
            .recorded
            .drain(..)
            .map(|(id, target, strip)| {
                if snap {
                    return Track::new(id, target, target, strip, false);
                }
                if let Some(track) = previous.iter().find(|track| track.id == id) {
                    let raised = track.raised || (zoom_changed && track.target != target);
                    return Track { target, raised, strip, ..*track };
                }
                if zoom_changed {
                    return Track::new(id, target, target, strip, false);
                }
                // A pane that appears as another disappears is that pane moving (e.g.
                // dragging a pane's last tab into a split); otherwise it is new.
                match removed.next() {
                    Some(source) => Track::new(id, source.presented(), target, strip, true),
                    None => Track::new(id, scaled_about_center(target, EXPAND_FROM), target, strip, true),
                }
            })
            .collect();
    }

    pub(crate) fn overlay(&self) -> PaneOverlay {
        let mut overlay = PaneOverlay::default();
        for track in &self.tracks {
            overlay.covers.extend(subtract(track.target, track.presented()));
        }
        let order: SmallVec<[&Track; 8]> = self
            .tracks
            .iter()
            .filter(|track| !track.raised)
            .chain(self.tracks.iter().filter(|track| track.raised))
            .collect();
        for (ix, track) in order.iter().enumerate() {
            let above = &order[ix + 1..];
            let presented = track.presented();
            let body = track.body();
            let pieces = occlude(subtract(presented, track.target), above);
            let lift = if track.raised { track.lift() } else { 0. };
            let shadow = if lift > 0. {
                let reach = (body + point(px(0.), SHADOW_OFFSET_Y)).dilate(SHADOW_BLUR * 2.);
                occlude(subtract(reach, presented), above)
            } else {
                SmallVec::new()
            };
            if !pieces.is_empty() || !shadow.is_empty() {
                overlay.ghosts.push(Ghost { presented, body, pieces, shadow, lift });
            }
        }
        overlay
    }
}

/// `pieces` minus every card in `above`; stops once nothing is left.
fn occlude(mut pieces: SmallVec<[Bounds<Pixels>; 4]>, above: &[&Track]) -> SmallVec<[Bounds<Pixels>; 4]> {
    for track in above {
        if pieces.is_empty() {
            break;
        }
        let hole = track.presented();
        pieces = pieces.into_iter().flat_map(|piece| subtract(piece, hole)).collect();
    }
    pieces
}

/// Records the pane `id`'s laid-out bounds and tab strip height; place it
/// absolutely over the pane.
pub(crate) fn record_bounds(motion: Rc<RefCell<PaneMotion>>, id: EntityId, strip: Pixels) -> impl IntoElement {
    canvas(move |bounds, _, _| motion.borrow_mut().record(id, bounds, strip), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// Paints `motion`'s overlay after the group's panes (which record their bounds
/// during prepaint), keeping frames coming while anything moves. `backdrop` is
/// what shows between the group's cards; `zoomed` says one pane is shown zoomed.
pub(crate) fn render_overlay(
    motion: Rc<RefCell<PaneMotion>>,
    backdrop: Hsla,
    zoomed: bool,
    cx: &App,
) -> impl IntoElement {
    let card = cx.theme().colors().editor_background;
    canvas(
        move |region, window, _| {
            let mut state = motion.borrow_mut();
            let overlay = if state.update(region, zoomed, Instant::now()) {
                window.request_animation_frame();
                state.overlay()
            } else {
                PaneOverlay::default()
            };
            (overlay, motion.clone())
        },
        move |_, (overlay, motion), window, _| {
            for bounds in overlay.covers {
                window.paint_quad(fill(bounds, backdrop));
            }
            let radii = Corners::all(CARD_RADIUS);
            for ghost in overlay.ghosts {
                let shadow = [BoxShadow::new(px(0.), SHADOW_OFFSET_Y, hsla(0., 0., 0., SHADOW_ALPHA * ghost.lift))
                    .blur_radius(SHADOW_BLUR)];
                for piece in ghost.shadow {
                    window.with_content_mask(Some(ContentMask { bounds: piece }), |window| {
                        window.paint_drop_shadows(ghost.body, radii, &shadow);
                    });
                }
                for piece in ghost.pieces {
                    window.with_content_mask(Some(ContentMask { bounds: piece }), |window| {
                        window.paint_quad(fill(ghost.presented, backdrop));
                        window.paint_quad(fill(ghost.body, card).corner_radii(radii));
                    });
                }
            }
            window.on_mouse_event({
                let motion = motion.clone();
                move |event: &MouseDownEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        motion.borrow_mut().pointer_pressed = true;
                    }
                }
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
                if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                    motion.borrow_mut().pointer_pressed = false;
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    fn id(n: u64) -> EntityId {
        EntityId::from(n)
    }

    fn area(pieces: &[Bounds<Pixels>]) -> f32 {
        pieces.iter().map(|b| f32::from(b.size.width) * f32::from(b.size.height)).sum()
    }

    fn ghost_area(overlay: &PaneOverlay) -> f32 {
        overlay.ghosts.iter().map(|ghost| area(&ghost.pieces)).sum()
    }

    const REGION: Bounds<Pixels> = Bounds {
        origin: gpui::Point { x: px(0.), y: px(0.) },
        size: gpui::Size { width: px(1000.), height: px(600.) },
    };

    fn frame(motion: &mut PaneMotion, panes: &[(u64, Bounds<Pixels>)], now: Instant) -> bool {
        for (n, bounds) in panes {
            motion.record(id(*n), *bounds, px(0.));
        }
        motion.update(REGION, false, now)
    }

    /// Runs frames at 120 Hz until settled, returning the frame count.
    fn settle(motion: &mut PaneMotion, panes: &[(u64, Bounds<Pixels>)], mut now: Instant) -> usize {
        for frames in 1..240 {
            now += Duration::from_millis(8);
            if !frame(motion, panes, now) {
                return frames;
            }
        }
        panic!("pane motion never settled");
    }

    #[test]
    fn subtract_splits_into_disjoint_pieces() {
        let pieces = subtract(rect(0., 0., 100., 100.), rect(25., 25., 50., 50.));
        assert_eq!(pieces.len(), 4);
        assert_eq!(area(&pieces), 100. * 100. - 50. * 50.);

        assert_eq!(
            subtract(rect(0., 0., 100., 100.), rect(200., 0., 10., 10.)).as_slice(),
            &[rect(0., 0., 100., 100.)]
        );
        assert!(subtract(rect(10., 10., 10., 10.), rect(0., 0., 100., 100.)).is_empty());

        let pieces = subtract(rect(0., 0., 100., 100.), rect(50., -10., 100., 200.));
        assert_eq!(pieces.as_slice(), &[rect(0., 0., 50., 100.)]);
    }

    #[test]
    fn first_frame_and_region_change_snap() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        assert!(!frame(&mut motion, &[(1, rect(0., 0., 1000., 600.))], now));
        assert_eq!(motion.overlay(), PaneOverlay::default());

        motion.record(id(1), rect(0., 0., 500., 600.), px(0.));
        motion.record(id(2), rect(500., 0., 500., 600.), px(0.));
        assert!(!motion.update(rect(0., 0., 900., 600.), false, now));
        assert_eq!(motion.overlay(), PaneOverlay::default());
    }

    #[test]
    fn pressed_pointer_snaps() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))], now);
        motion.pointer_pressed = true;
        assert!(!frame(&mut motion, &[(1, rect(0., 0., 600., 600.)), (2, rect(600., 0., 400., 600.))], now));
    }

    #[test]
    fn split_slides_existing_and_expands_new() {
        let mut motion = PaneMotion::default();
        let start = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 1000., 600.))], start);

        let split = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        assert!(frame(&mut motion, &split, start));
        // Existing pane is still drawn full width: it hangs over the new pane's area.
        assert_eq!(motion.tracks[0].presented(), rect(0., 0., 1000., 600.));
        // The new pane starts scaled down about its center and is raised.
        assert_eq!(motion.tracks[1].presented(), scaled_about_center(rect(500., 0., 500., 600.), EXPAND_FROM));
        assert!(motion.tracks[1].raised);

        let overlay = motion.overlay();
        // Covers: the new pane's unrevealed ring.
        assert!(!overlay.covers.is_empty());
        // The existing pane's ghost leaves a hole where the raised new pane shows.
        let new_area = 500. * EXPAND_FROM * 600. * EXPAND_FROM;
        assert!((ghost_area(&overlay) - (500. * 600. - new_area)).abs() < 1.);

        assert!(settle(&mut motion, &split, start) > 5);
        assert_eq!(motion.overlay(), PaneOverlay::default());
        assert!(!motion.tracks[1].raised);
    }

    #[test]
    fn motion_decelerates_without_overshoot() {
        let mut motion = PaneMotion::default();
        let mut now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 1000., 600.))], now);
        let split = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        frame(&mut motion, &split, now);
        let (mut width, mut previous_step) = (motion.tracks[0].presented().size.width, px(f32::INFINITY));
        loop {
            now += Duration::from_millis(8);
            let active = frame(&mut motion, &split, now);
            let next = motion.tracks[0].presented().size.width;
            assert!(next >= px(500.), "overshot to {next:?}");
            // Only the final snap onto the target may jump (by under SETTLE_EPSILON).
            assert!(next == px(500.) || width - next <= previous_step, "sped up: {width:?} -> {next:?}");
            previous_step = width - next;
            width = next;
            if !active {
                break;
            }
        }
        assert_eq!(width, px(500.));
    }

    #[test]
    fn pane_replacing_removed_one_moves_from_it() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))], now);
        // Pane 1 disappears as pane 3 appears on the right: 3 travels from 1's rect.
        let moved = [(2, rect(0., 0., 500., 600.)), (3, rect(500., 0., 500., 600.))];
        frame(&mut motion, &moved, now);
        assert_eq!(motion.tracks[1].presented(), rect(0., 0., 500., 600.));
        assert!(motion.tracks[1].raised);
        settle(&mut motion, &moved, now);
        assert_eq!(motion.tracks[1].presented(), rect(500., 0., 500., 600.));
    }

    #[test]
    fn unchanged_settled_frames_skip_work_and_keep_capacity() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        let panes = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        frame(&mut motion, &panes, now);
        let capacity = motion.recorded.capacity();
        for _ in 0..3 {
            assert!(!frame(&mut motion, &panes, now));
        }
        assert!(motion.recorded.is_empty());
        assert_eq!(motion.recorded.capacity(), capacity);
        assert_eq!(motion.overlay(), PaneOverlay::default());
    }

    #[test]
    fn active_motion_with_unchanged_targets_keeps_moving() {
        let mut motion = PaneMotion::default();
        let mut now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 1000., 600.))], now);
        let split = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        frame(&mut motion, &split, now);
        let before = motion.tracks[0].presented();
        now += Duration::from_millis(16);
        assert!(frame(&mut motion, &split, now));
        assert_ne!(motion.tracks[0].presented(), before);
    }

    #[test]
    fn changed_target_after_settling_animates() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        let a = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        frame(&mut motion, &a, now);
        assert!(!frame(&mut motion, &a, now));
        let b = [(1, rect(0., 0., 600., 600.)), (2, rect(600., 0., 400., 600.))];
        assert!(frame(&mut motion, &b, now));
        assert_eq!(motion.tracks[0].presented(), rect(0., 0., 500., 600.));
    }

    #[test]
    fn region_change_snaps_active_motion() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 1000., 600.))], now);
        let split = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        assert!(frame(&mut motion, &split, now));
        for (n, bounds) in &split {
            motion.record(id(*n), *bounds, px(0.));
        }
        assert!(!motion.update(rect(0., 0., 900., 600.), false, now));
        assert_eq!(motion.overlay(), PaneOverlay::default());
    }

    #[test]
    fn zoom_in_grows_pane_and_drops_others_despite_region_change() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))], now);
        let zoom_region = rect(-50., 0., 1100., 600.);
        motion.record(id(2), zoom_region, px(0.));
        assert!(motion.update(zoom_region, true, now));
        assert_eq!(motion.tracks.len(), 1);
        assert_eq!(motion.tracks[0].presented(), rect(500., 0., 500., 600.));
        assert!(motion.tracks[0].raised);
        // The unreached zoomed area shows backdrop, not the vanished pane.
        assert!((area(&motion.overlay().covers) - (1100. * 600. - 500. * 600.)).abs() < 1.);
    }

    #[test]
    fn zoom_out_reveals_others_in_place_under_shrinking_pane() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        let zoom_region = rect(0., 0., 1000., 600.);
        motion.record(id(2), zoom_region, px(0.));
        motion.update(zoom_region, true, now);
        let panes = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        assert!(frame(&mut motion, &panes, now));
        assert_eq!(motion.tracks[0].presented(), rect(0., 0., 500., 600.));
        assert!(!motion.tracks[0].raised);
        assert_eq!(motion.tracks[1].presented(), zoom_region);
        assert!(motion.tracks[1].raised);
        // The shrinking pane's ghost covers pane 1, which is already in place below it.
        assert!((ghost_area(&motion.overlay()) - 500. * 600.).abs() < 1.);
    }

    #[test]
    fn snap_next_skips_changes_made_while_hidden() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))], now);
        motion.snap_next();
        assert!(!frame(&mut motion, &[(2, rect(0., 0., 1000., 600.))], now));
        assert_eq!(motion.overlay(), PaneOverlay::default());
    }

    #[test]
    fn fully_occluded_ghost_is_dropped() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        frame(&mut motion, &[(1, rect(0., 0., 1000., 600.))], now);
        // Pane 1 shrinks to the left half while new pane 2 is raised over the right half.
        let split = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        frame(&mut motion, &split, now);
        motion.tracks[1].edges = edges(rect(500., 0., 500., 600.));
        assert!(motion.overlay().ghosts.is_empty());
    }

    #[test]
    fn retarget_keeps_presented_position() {
        let mut motion = PaneMotion::default();
        let mut now = Instant::now();
        let a = [(1, rect(0., 0., 500., 600.)), (2, rect(500., 0., 500., 600.))];
        frame(&mut motion, &a, now);
        let b = [(2, rect(0., 0., 500., 600.)), (1, rect(500., 0., 500., 600.))];
        frame(&mut motion, &b, now);
        now += Duration::from_millis(40);
        frame(&mut motion, &b, now);
        let mid = motion.tracks.iter().find(|t| t.id == id(1)).unwrap().presented();
        frame(&mut motion, &a, now);
        assert_eq!(motion.tracks.iter().find(|t| t.id == id(1)).unwrap().presented(), mid);
    }

    #[test]
    fn ghost_body_sits_below_tab_strip() {
        let track = Track::new(id(1), rect(0., 0., 400., 300.), rect(0., 0., 200., 300.), px(40.), true);
        assert_eq!(track.body(), rect(0., 40., 400., 260.));
        let short = Track::new(id(1), rect(0., 0., 400., 20.), rect(0., 0., 400., 20.), px(40.), false);
        assert_eq!(short.body().size.height, px(0.));
    }

    #[test]
    fn lift_fades_as_card_lands() {
        let far = Track::new(id(1), rect(0., 0., 1000., 600.), rect(0., 0., 500., 600.), px(0.), true);
        assert_eq!(far.lift(), 1.);
        let near = Track::new(id(1), rect(0., 0., 516., 600.), rect(0., 0., 500., 600.), px(0.), true);
        assert!((near.lift() - 0.5).abs() < 1e-4);
        let landed = Track::new(id(1), rect(0., 0., 500., 600.), rect(0., 0., 500., 600.), px(0.), true);
        assert_eq!(landed.lift(), 0.);
    }

    #[test]
    fn only_raised_cards_cast_shadow_outside_themselves() {
        let mut motion = PaneMotion::default();
        let now = Instant::now();
        let zoom_region = rect(0., 0., 1000., 600.);
        motion.record(id(2), zoom_region, px(30.));
        motion.update(zoom_region, true, now);
        // Unzoom: pane 2 shrinks raised over pane 1, which snaps in place below it.
        motion.record(id(1), rect(0., 0., 500., 600.), px(30.));
        motion.record(id(2), rect(500., 0., 500., 600.), px(30.));
        motion.update(REGION, false, now);
        motion.tracks[1].edges = edges(rect(300., 0., 700., 600.));
        let overlay = motion.overlay();
        assert_eq!(overlay.ghosts.len(), 1);
        let ghost = &overlay.ghosts[0];
        assert_eq!(ghost.body, rect(300., 30., 700., 570.));
        assert_eq!(ghost.pieces.as_slice(), &[rect(300., 0., 200., 600.)]);
        assert_eq!(ghost.lift, 1.);
        assert!(!ghost.shadow.is_empty());
        for piece in &ghost.shadow {
            assert!(
                piece.intersect(&ghost.presented).size.width <= px(0.)
                    || piece.intersect(&ghost.presented).size.height <= px(0.)
            );
        }
    }
}
