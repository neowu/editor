use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::mem;
use std::ops::Range;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use slotmap::DefaultKey;
use slotmap::SecondaryMap;
use smallvec::SmallVec;
use taffy::TaffyTree;
use taffy::TraversePartialTree as _;
use taffy::compute_leaf_layout;
use taffy::geometry::Point as TaffyPoint;
use taffy::geometry::Rect as TaffyRect;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::LengthPercentageAuto;
use taffy::prelude::max_content;
use taffy::prelude::min_content;
use taffy::style;
use taffy::style::AvailableSpace as TaffyAvailableSpace;
use taffy::style::Dimension;
use taffy::style::LengthPercentage;
use taffy::tree::NodeId;

use crate::AbsoluteLength;
use crate::App;
use crate::Bounds;
use crate::DefiniteLength;
use crate::Edges;
use crate::GridTemplate;
use crate::Length;
use crate::Pixels;
use crate::Point;
use crate::Size;
use crate::Style;
// lint: expect(module_cycle)
use crate::Window;
use crate::size;
use crate::util::ceil_to_device_pixel;
use crate::util::round_half_toward_zero;
use crate::util::round_stroke_to_device_pixel;
use crate::util::round_to_device_pixel;

#[rustfmt::skip]
pub(crate) type MeasureFn =
    // lint: expect(module_cycle)
    dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>;

/// What a measure function kept between frames shares with the element it
/// measures for (see [`Window::request_retained_measured_layout`]).
pub(crate) trait MeasureState: 'static {
    /// Whether measuring writes to the state, so that what a measurement
    /// leaves behind depends on the ones before it.
    const RESETS: bool = false;

    /// Forgets what earlier measurements left behind.
    fn reset(&self) {}
}

fn reset_measure_state<S: MeasureState>(state: &dyn Any) {
    if let Some(state) = state.downcast_ref::<S>() {
        state.reset();
    }
}

struct RetainedMeasure {
    state: Rc<dyn Any>,
    reset: Option<fn(&dyn Any)>,
}

enum NodeMeasure<M> {
    None,
    /// The node keeps the measure function it has, and the sizes cached from it.
    Keep,
    Replace(M, Option<RetainedMeasure>),
}

/// Layout nodes are kept from one frame to the next, so Taffy's layout cache
/// skips the subtrees whose styles, children and measure functions didn't
/// change. `M` is the measure function, a parameter so tests can run without
/// a window.
// lint: expect(module_cycle)
pub(crate) struct TaffyLayoutEngine<M = Box<MeasureFn>> {
    taffy: TaffyTree,
    nodes: SecondaryMap<DefaultKey, Node<M>>,
    scopes: FxHashMap<u64, LayoutScope>,
    /// Evicted nodes, detached from the tree. A request that misses reuses one
    /// before allocating, and those still unused at the end of the frame after
    /// the one they were evicted in are removed.
    free_nodes: Vec<NodeId>,
    /// Stretched nodes requested this frame that weren't stretched again yet.
    stretch_pending: SmallVec<[NodeId; 2]>,
    /// How many nodes kept a stateful measure function this frame; see `Node::measure_kept`.
    kept_measures: usize,
    frame: u64,
    scratch: Vec<NodeId>,
}

/// Layout nodes requested by elements under one element id path, in request order.
///
/// Elements without ids are distinguished only by their order within the
/// nearest ancestor with one. Which node a request reuses only affects how
/// much of Taffy's cache survives: a reused node's style and children are
/// always brought up to date.
#[derive(Default)]
struct LayoutScope {
    /// How many of `nodes` were requested this frame.
    requested: usize,
    /// The last frame any of `nodes` was requested in.
    requested_frame: u64,
    nodes: SmallVec<[NodeId; 4]>,
}

/// How many frames the nodes under an id path outlive the last request under
/// it. A cached view requests nothing while it is clean, and renders again
/// with the same tree more often than not: its nodes are still there, and its
/// layout is skipped (see `compute`). Frames are only drawn while something
/// changes, so this is a couple of seconds of animation and much longer
/// otherwise.
const UNREQUESTED_FRAMES: u64 = 240;

struct Node<M> {
    /// What the node's Taffy style was converted from, compared before converting again.
    style: LayoutStyle,
    measure: Option<(M, Option<RetainedMeasure>)>,
    /// Whether the Taffy style has `stretch_auto_size_to_fill` applied on top of `style`.
    stretched: bool,
    /// Whether the measure function was kept this frame with the state earlier frames left in it.
    measure_kept: bool,
    /// The last frame the node was requested in.
    requested_frame: u64,
    /// The last frame the node was computed as a root in, and the space it was last given.
    computed_frame: u64,
    computed_space: Size<AvailableSpace>,
    /// Whether the node was computed as a root in more than one space in that
    /// frame. What its measure functions left then depends on the earlier
    /// spaces too, so it doesn't stand for a layout in the last one alone.
    computed_spaces_differ: bool,
    /// Whether the node was computed as a root in the frame it was last requested in before this one.
    was_root: bool,
    /// The frame `bounds` and `outer_origin` are for.
    bounds_frame: u64,
    bounds: Bounds<Pixels>,
    /// Unrounded absolute border-box top-left coordinate in device pixels.
    outer_origin: Point<f32>,
}

const EXPECT_MESSAGE: &str = "we should avoid taffy layout errors by construction if possible";

fn node_key(id: NodeId) -> DefaultKey {
    id.into()
}

impl TaffyLayoutEngine {
    pub(crate) fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        // lint: expect(module_cycle)
        window: &mut Window,
        cx: &mut App,
    ) {
        // lint: expect(module_cycle)
        let scale_factor = window.scale_factor();
        self.compute(id, available_space, scale_factor, |measure, known_dimensions, available_space| {
            measure(known_dimensions, available_space, window, cx)
        });
    }
}

impl<M> TaffyLayoutEngine<M> {
    pub(crate) fn new() -> Self {
        let mut taffy = TaffyTree::new();
        taffy.disable_rounding();
        TaffyLayoutEngine {
            taffy,
            nodes: SecondaryMap::new(),
            scopes: FxHashMap::default(),
            free_nodes: Vec::new(),
            stretch_pending: SmallVec::new(),
            kept_measures: 0,
            frame: 1,
            scratch: Vec::new(),
        }
    }

    /// Evicts every node. Cached sizes are in device pixels and a style can
    /// convert the same at two scales, so nothing is kept across a scale
    /// factor change.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Ends the frame, evicting the nodes that went unrequested: at once under
    /// an id path that had other requests (the tree there changed), and after
    /// `UNREQUESTED_FRAMES` under one that had none.
    pub(crate) fn clear(&mut self) {
        self.unstretch_pending();
        self.kept_measures = 0;

        for id in self.free_nodes.drain(..) {
            self.nodes.remove(node_key(id));
            self.taffy.remove(id).expect(EXPECT_MESSAGE);
        }

        let frame = self.frame;
        let free_nodes = &mut self.free_nodes;
        let nodes = &mut self.nodes;
        let taffy = &mut self.taffy;
        self.scopes.retain(|_, scope| {
            let requested = mem::take(&mut scope.requested);
            if requested > 0 {
                scope.requested_frame = frame;
                free_nodes.extend(scope.nodes.drain(requested..));
                return true;
            }
            if scope.requested_frame + 1 == frame {
                // A measure function can hold on to anything, so one that
                // isn't kept on purpose is dropped as soon as it is unused.
                for &id in &scope.nodes {
                    let node = &mut nodes[node_key(id)];
                    if matches!(node.measure, Some((_, None))) {
                        node.measure = None;
                        taffy.mark_dirty(id).expect(EXPECT_MESSAGE);
                    }
                }
            }
            if scope.requested_frame + UNREQUESTED_FRAMES >= frame {
                return true;
            }
            free_nodes.extend(scope.nodes.drain(..));
            false
        });

        for &id in &self.free_nodes {
            self.nodes[node_key(id)].measure = None;
            if let Some(parent) = self.taffy.parent(id) {
                self.taffy.remove_child(parent, id).expect(EXPECT_MESSAGE);
            }
            if self.taffy.child_count(id) > 0 {
                self.taffy.set_children(id, &[]).expect(EXPECT_MESSAGE);
            }
        }
        self.frame += 1;
    }

    /// `scope` is the hash of the element id path the layout is requested under.
    pub(crate) fn request_layout(
        &mut self,
        scope: u64,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        self.request_node(scope, style, rem_size, scale_factor, children, NodeMeasure::None)
    }

    /// A measure function can measure anything, so the sizes Taffy cached from
    /// the node's previous one are dropped.
    pub(crate) fn request_measured_layout(
        &mut self,
        scope: u64,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: M,
    ) -> LayoutId {
        self.request_node(scope, style, rem_size, scale_factor, &[], NodeMeasure::Replace(measure, None))
    }

    /// Keeps the node's measure function, and the sizes cached from it, when
    /// `retain` accepts the state it was requested with last frame. Otherwise
    /// the node gets the state and measure function from `new`. A function
    /// whose state resets is still measured again, from empty state, unless
    /// nothing in its tree changed.
    pub(crate) fn request_retained_measured_layout<S: MeasureState>(
        &mut self,
        scope: u64,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        retain: impl FnOnce(&S) -> bool,
        new: impl FnOnce() -> (Rc<S>, M),
    ) -> (LayoutId, Rc<S>) {
        if let Some(state) = self.retained_measure_state(scope).and_then(|state| Rc::clone(state).downcast::<S>().ok())
            && retain(&state)
        {
            let id = self.request_node(scope, style, rem_size, scale_factor, &[], NodeMeasure::Keep);
            return (id, state);
        }
        let (state, measure) = new();
        let retained = RetainedMeasure {
            state: Rc::clone(&state) as Rc<dyn Any>,
            reset: S::RESETS.then_some(reset_measure_state::<S> as fn(&dyn Any)),
        };
        let measure = NodeMeasure::Replace(measure, Some(retained));
        (self.request_node(scope, style, rem_size, scale_factor, &[], measure), state)
    }

    /// The state kept with the measure function of the node the next request under `scope` reuses.
    fn retained_measure_state(&self, scope: u64) -> Option<&Rc<dyn Any>> {
        let scope = self.scopes.get(&scope)?;
        let id = *scope.nodes.get(scope.requested)?;
        let (_, retained) = self.nodes[node_key(id)].measure.as_ref()?;
        Some(&retained.as_ref()?.state)
    }

    fn request_node(
        &mut self,
        scope: u64,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
        measure: NodeMeasure<M>,
    ) -> LayoutId {
        // This is safe because LayoutId is repr(transparent) to taffy::tree::NodeId.
        let children = LayoutId::to_taffy_slice(children);
        let taffy = &mut self.taffy;
        let frame = self.frame;

        let scope = self.scopes.entry(scope).or_default();
        let ordinal = scope.requested;
        scope.requested += 1;
        let id = if let Some(&id) = scope.nodes.get(ordinal) {
            let node = &mut self.nodes[node_key(id)];
            node.was_root = node.computed_frame == node.requested_frame;
            if !node.style.matches(style, rem_size, scale_factor) {
                node.style = LayoutStyle::new(style, rem_size, scale_factor);
                node.stretched = false;
                let taffy_style = node.style.to_taffy();
                if taffy.style(id).expect(EXPECT_MESSAGE) != &taffy_style {
                    taffy.set_style(id, taffy_style).expect(EXPECT_MESSAGE);
                }
            } else if node.stretched {
                // Only a root is stretched, and this node may not be one any more.
                self.stretch_pending.push(id);
            }
            if !taffy.child_ids(id).eq(children.iter().copied()) {
                taffy.set_children(id, children).expect(EXPECT_MESSAGE);
            }
            id
        } else {
            let style = LayoutStyle::new(style, rem_size, scale_factor);
            let taffy_style = style.to_taffy();
            let id = if let Some(id) = self.free_nodes.pop() {
                taffy.set_style(id, taffy_style).expect(EXPECT_MESSAGE);
                let node = &mut self.nodes[node_key(id)];
                node.style = style;
                node.stretched = false;
                node.was_root = false;
                id
            } else {
                let id = taffy.new_leaf(taffy_style).expect(EXPECT_MESSAGE);
                self.nodes.insert(
                    node_key(id),
                    Node {
                        style,
                        measure: None,
                        stretched: false,
                        measure_kept: false,
                        requested_frame: 0,
                        computed_frame: 0,
                        computed_space: Size::default(),
                        computed_spaces_differ: false,
                        was_root: false,
                        bounds_frame: 0,
                        bounds: Bounds::default(),
                        outer_origin: Point::default(),
                    },
                );
                id
            };
            if !children.is_empty() {
                // Unlike `new_with_children`, this detaches children from their previous parents.
                taffy.set_children(id, children).expect(EXPECT_MESSAGE);
            }
            scope.nodes.push(id);
            id
        };

        let node = &mut self.nodes[node_key(id)];
        node.requested_frame = frame;
        node.measure_kept = false;
        match measure {
            NodeMeasure::None => {
                if node.measure.take().is_some() {
                    taffy.mark_dirty(id).expect(EXPECT_MESSAGE);
                }
            }
            NodeMeasure::Replace(measure, retained) => {
                node.measure = Some((measure, retained));
                taffy.mark_dirty(id).expect(EXPECT_MESSAGE);
            }
            NodeMeasure::Keep => {
                if let Some((_, Some(RetainedMeasure { state, reset: Some(reset) }))) = &node.measure {
                    // A node Taffy has nothing cached for is measured again this frame.
                    if taffy.dirty(id).expect(EXPECT_MESSAGE) {
                        reset(&**state);
                    } else {
                        node.measure_kept = true;
                        self.kept_measures += 1;
                    }
                }
            }
        }
        id.into()
    }

    /// Treats any `auto` dimension of the given node's style as filling `size`.
    ///
    /// This is applied to window roots before layout so they behave like the
    /// root element on the web, which stretches to fill the initial containing
    /// block (the viewport) unless given an explicit size. Explicitly styled
    /// dimensions are preserved.
    pub(crate) fn stretch_auto_size_to_fill(&mut self, id: LayoutId, size: Size<Pixels>, scale_factor: f32) {
        self.stretch_pending.retain(|pending| *pending != id.0);
        let node = &mut self.nodes[node_key(id.0)];
        let stretch_width = node.style.size.width == Length::Auto;
        let stretch_height = node.style.size.height == Length::Auto;
        if !stretch_width && !stretch_height {
            return;
        }
        let mut style = node.style.to_taffy();
        if stretch_width {
            style.size.width = Dimension::length(round_to_device_pixel(size.width.0, scale_factor));
        }
        if stretch_height {
            style.size.height = Dimension::length(round_to_device_pixel(size.height.0, scale_factor));
        }
        node.stretched = true;
        // A root stretched to the same size as last frame keeps its cached layout.
        if self.taffy.style(id.0).expect(EXPECT_MESSAGE) != &style {
            self.taffy.set_style(id.0, style).expect(EXPECT_MESSAGE);
        }
    }

    /// Restores the styles of the nodes that were stretched last frame and not in this one.
    fn unstretch_pending(&mut self) {
        for id in self.stretch_pending.drain(..) {
            let node = &mut self.nodes[node_key(id)];
            node.stretched = false;
            self.taffy.set_style(id, node.style.to_taffy()).expect(EXPECT_MESSAGE);
        }
    }

    fn compute(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        scale_factor: f32,
        mut measure: impl FnMut(&mut M, Size<Option<Pixels>>, Size<AvailableSpace>) -> Size<Pixels>,
    ) {
        self.unstretch_pending();
        let frame = self.frame;

        // A node computed as a root may still be attached to a parent from an
        // earlier frame that no element requested in this one. Its layout must
        // not be relative to that parent.
        if let Some(parent) = self.taffy.parent(id.0)
            && self.nodes[node_key(parent)].requested_frame != frame
        {
            self.taffy.remove_child(parent, id.0).expect(EXPECT_MESSAGE);
        }

        let transform = |v: AvailableSpace| match v {
            AvailableSpace::Definite(pixels) => AvailableSpace::Definite(Pixels(pixels.0 * scale_factor)),
            AvailableSpace::MinContent => AvailableSpace::MinContent,
            AvailableSpace::MaxContent => AvailableSpace::MaxContent,
        };
        let available_space = size(transform(available_space.width), transform(available_space.height));

        let node = &mut self.nodes[node_key(id.0)];
        let computed_frame = mem::replace(&mut node.computed_frame, frame);
        let computed_space = mem::replace(&mut node.computed_space, available_space);
        let computed_spaces_differ = mem::take(&mut node.computed_spaces_differ);
        if computed_frame == frame {
            node.computed_spaces_differ = computed_spaces_differ || computed_space != available_space;
            // Computed again within the frame: the bounds read since are stale.
            let stack = &mut self.scratch;
            stack.push(id.0);
            while let Some(id) = stack.pop() {
                self.nodes[node_key(id)].bounds_frame = 0;
                stack.extend(self.taffy.child_ids(id));
            }
        } else if node.was_root
            && !computed_spaces_differ
            && computed_space == available_space
            && !self.taffy.dirty(id.0).expect(EXPECT_MESSAGE)
        {
            // Nothing below this root changed since it was last requested and
            // laid out, as a root in this space only: every node still has that layout.
            return;
        }

        // A measure function leaves in its state what the element paints, and
        // what it leaves depends on the sizes Taffy asked it for, in order.
        // Taffy asks again for the ones it has not cached, so a kept function
        // would run on top of state from earlier frames while a new one starts
        // empty. State is only kept across a tree that is skipped as a whole.
        if self.kept_measures > 0 {
            let stack = &mut self.scratch;
            stack.push(id.0);
            while let Some(id) = stack.pop() {
                let node = &mut self.nodes[node_key(id)];
                if mem::take(&mut node.measure_kept) {
                    if let Some((_, Some(RetainedMeasure { state, reset: Some(reset) }))) = &node.measure {
                        reset(&**state);
                    }
                    self.taffy.mark_dirty(id).expect(EXPECT_MESSAGE);
                }
                stack.extend(self.taffy.child_ids(id));
            }
        }

        let nodes = &mut self.nodes;
        self.taffy
            .compute_layout_with_measure(id.into(), available_space.into(), |inputs, id, _, style| {
                compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known_dimensions, available_space| {
                        let Some((node_measure, _)) = &mut nodes[node_key(id)].measure else {
                            return TaffySize::default();
                        };

                        let known_dimensions = Size {
                            width: known_dimensions.width.map(|e| Pixels(e / scale_factor)),
                            height: known_dimensions.height.map(|e| Pixels(e / scale_factor)),
                        };

                        let available_space: Size<AvailableSpace> = available_space.into();
                        let untransform = |ev: AvailableSpace| match ev {
                            AvailableSpace::Definite(pixels) => {
                                AvailableSpace::Definite(Pixels(pixels.0 / scale_factor))
                            }
                            AvailableSpace::MinContent => AvailableSpace::MinContent,
                            AvailableSpace::MaxContent => AvailableSpace::MaxContent,
                        };
                        let available_space =
                            size(untransform(available_space.width), untransform(available_space.height));

                        let measured_size = measure(node_measure, known_dimensions, available_space);
                        snap_measured_size_to_device_pixels(measured_size, scale_factor).into()
                    },
                )
            })
            .expect(EXPECT_MESSAGE);
    }

    // Pixel snapping
    //
    // Painting primitives at non-integer pixel coordinates produces blurry
    // output. Pixel snapping converts layout coordinates into integer
    // device-pixel coordinates so painted edges land exactly on physical
    // pixel boundaries.
    //
    // Non-integer coordinates can arise for several reasons, including:
    //   - flex distribution, percentages, centering, and text measurement
    //     can produce fractional element sizes and positions;
    //   - at fractional scale factors (for example 125% or 150%), integer
    //     logical-pixel values can map to non-integer device-pixel values.
    //
    // We pixel-snap by rounding in device-pixel space, after multiplying
    // by `scale_factor`, so that snapping targets physical pixels. Bounds
    // are divided by `scale_factor` before being returned to GPUI.
    //
    // Midpoints are rounded toward zero. This is a stylistic choice: a
    // 1-logical-pixel line at 150% scale should render as 1 dp rather than
    // 2 dp.
    //
    // Pixel snapping is done in two phases:
    //
    //  1. Pre-layout metric snapping. Before Taffy computes layout, all
    //     authored absolute lengths are rounded in `to_taffy`. This
    //     includes borders, padding, gaps, and explicit sizes.
    //     Custom-measured leaf nodes have their measured sizes rounded up
    //     to integer device-pixel lengths.
    //
    //  2. Post-layout edge snapping. After Taffy resolves the tree, layout
    //     relationships such as flex shares, grid tracks, percentages, and
    //     centering can produce new fractional edge positions. Boxes now
    //     have edges in absolute coordinates, and snapping must decide
    //     where those edges land on the device-pixel grid.
    //
    // Ideally, post-layout snapping would satisfy:
    //
    //  - Edge closure. Two raw layout edges at the same absolute position
    //    should snap to the same pixel column.
    //  - Translation stability. A component's internal geometry should not
    //    change when it moves to a new absolute position.
    //
    // These goals are in tension because rounding is not associative.
    // The simple local schemes make different tradeoffs:
    //
    //  - Absolute edge rounding gives each window coordinate one answer,
    //    so coincident edges always close globally. But a span's snapped
    //    length is `round(far) - round(near)`, which may change by 1 dp
    //    as its absolute origin moves.
    //
    //  - Parent-relative edge rounding rounds each child inside its
    //    parent's coordinate space. This guarantees translation stability,
    //    but a shared edge reached through different parents can
    //    accumulate different rounding, causing non-closure between
    //    cousins.
    //
    //  - Length rounding rounds each width, height, and thickness
    //    independently and then places boxes from those rounded lengths.
    //    Sizes stay stable under translation, but neighboring boxes derive
    //    their shared boundary from different sources, so closure is not
    //    guaranteed.
    //
    // We apply absolute edge rounding for each element's outer box in
    // post-layout rounding to preserve closure. Border and padding widths
    // are not touched by post-layout rounding; they keep their pre-layout
    // rounded value so that they remain stable under translation.
    //
    // This gives both closure and translation stability in the case that
    // all local metrics are integer device-pixel lengths. Pre-layout
    // rounding covers that in most cases. The exception is metrics
    // resolved by layout relationships, such as percentages. Outer box
    // edges will still close globally, and painted border widths are still
    // snapped independently, but the raw content-box origin can carry a
    // 1dp residual into descendants.

    pub(crate) fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        let frame = self.frame;
        let node = &self.nodes[node_key(id.0)];
        if node.bounds_frame == frame {
            return node.bounds;
        }

        let layout = self.taffy.layout(id.into()).expect(EXPECT_MESSAGE);
        let layout_location = layout.location;
        let layout_size = layout.size;

        let outer_origin = match self.taffy.parent(id.0) {
            Some(parent_id) => {
                self.layout_bounds(parent_id.into(), scale_factor);
                self.nodes[node_key(parent_id)].outer_origin + Point::from(layout_location)
            }
            None => Point::from(layout_location),
        };

        let absolute_far = outer_origin + Point::from(Size::from(layout_size));
        let snapped_bounds =
            Bounds::from_corners(outer_origin.map(round_half_toward_zero), absolute_far.map(round_half_toward_zero));

        let bounds = (snapped_bounds / scale_factor).map(Pixels);
        let node = &mut self.nodes[node_key(id.0)];
        node.bounds_frame = frame;
        node.bounds = bounds;
        node.outer_origin = outer_origin;
        bounds
    }
}

/// A unique identifier for a layout node, generated when requesting a layout from Taffy.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct LayoutId(NodeId);

impl LayoutId {
    const fn to_taffy_slice(node_ids: &[Self]) -> &[NodeId] {
        // SAFETY: LayoutId is repr(transparent) to taffy::tree::NodeId.
        unsafe { std::slice::from_raw_parts(node_ids.as_ptr().cast(), node_ids.len()) }
    }
}

impl Hash for LayoutId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        u64::from(self.0).hash(state);
    }
}

impl From<NodeId> for LayoutId {
    fn from(node_id: NodeId) -> Self {
        Self(node_id)
    }
}

impl From<LayoutId> for NodeId {
    fn from(layout_id: LayoutId) -> NodeId {
        layout_id.0
    }
}

fn snap_measured_size_to_device_pixels(size: Size<Pixels>, scale_factor: f32) -> Size<f32> {
    size.map(|d| ceil_to_device_pixel(d.0.max(0.0), scale_factor))
}

fn border_widths_to_taffy(
    widths: &Edges<AbsoluteLength>,
    rem_size: Pixels,
    scale_factor: f32,
) -> TaffyRect<LengthPercentage> {
    let snap = |w: &AbsoluteLength| {
        LengthPercentage::length(round_stroke_to_device_pixel(w.to_pixels(rem_size).0, scale_factor))
    };
    TaffyRect {
        top: snap(&widths.top),
        right: snap(&widths.right),
        bottom: snap(&widths.bottom),
        left: snap(&widths.left),
    }
}

trait ToTaffy<Output> {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Output;
}

// Conversion reads the copied fields, not `Style`, so a field can't affect
// layout without also being compared to decide whether to convert again.
macro_rules! layout_style {
    ($($field:ident: $field_type:ty),* $(,)?) => {
        /// The fields of a [`Style`] that affect layout, and what their lengths are resolved against.
        #[derive(Clone, Debug)]
        struct LayoutStyle {
            rem_size: Pixels,
            scale_factor: f32,
            $($field: $field_type,)*
        }

        impl LayoutStyle {
            fn new(style: &Style, rem_size: Pixels, scale_factor: f32) -> Self {
                Self {
                    rem_size,
                    scale_factor,
                    $($field: Clone::clone(&style.$field),)*
                }
            }

            fn matches(&self, style: &Style, rem_size: Pixels, scale_factor: f32) -> bool {
                self.rem_size == rem_size
                    && self.scale_factor == scale_factor
                    $(&& self.$field == style.$field)*
            }
        }
    };
}

layout_style! {
    display: crate::Display,
    overflow: Point<crate::Overflow>,
    scrollbar_width: AbsoluteLength,
    position: crate::Position,
    inset: Edges<Length>,
    size: Size<Length>,
    min_size: Size<Length>,
    max_size: Size<Length>,
    aspect_ratio: Option<f32>,
    margin: Edges<Length>,
    padding: Edges<DefiniteLength>,
    border_widths: Edges<AbsoluteLength>,
    align_items: Option<crate::AlignItems>,
    align_self: Option<crate::AlignSelf>,
    align_content: Option<crate::AlignContent>,
    justify_content: Option<crate::JustifyContent>,
    gap: Size<DefiniteLength>,
    flex_direction: crate::FlexDirection,
    flex_wrap: crate::FlexWrap,
    flex_basis: Length,
    flex_grow: f32,
    flex_shrink: f32,
    grid_rows: Option<GridTemplate>,
    grid_cols: Option<GridTemplate>,
    grid_location: Option<crate::GridLocation>,
}

impl LayoutStyle {
    fn to_taffy(&self) -> style::Style {
        use taffy::style_helpers::fr;
        use taffy::style_helpers::length;
        use taffy::style_helpers::minmax;
        use taffy::style_helpers::repeat;

        let rem_size = self.rem_size;
        let scale_factor = self.scale_factor;

        fn to_grid_line(placement: &Range<crate::GridPlacement>) -> taffy::Line<taffy::GridPlacement> {
            taffy::Line { start: placement.start.into(), end: placement.end.into() }
        }

        fn to_grid_repeat<T: style::CheapCloneStr>(unit: Option<GridTemplate>) -> Vec<taffy::GridTemplateComponent<T>> {
            unit.map_or_default(|template| {
                match template.min_size {
                    // grid-template-*: repeat(<number>, minmax(0, 1fr));
                    crate::GridTemplateMinSize::Zero => {
                        vec![repeat(template.repeat, vec![minmax(length(0.0_f32), fr(1.0_f32))])]
                    }
                    // grid-template-*: repeat(<number>, minmax(min-content, 1fr));
                    crate::GridTemplateMinSize::MinContent => {
                        vec![repeat(template.repeat, vec![minmax(min_content(), fr(1.0_f32))])]
                    }
                    // grid-template-*: repeat(<number>, minmax(0, max-content))
                    crate::GridTemplateMinSize::MaxContent => {
                        vec![repeat(template.repeat, vec![minmax(length(0.0_f32), max_content())])]
                    }
                }
            })
        }

        style::Style {
            display: self.display.into(),
            overflow: self.overflow.into(),
            scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
            position: self.position.into(),
            inset: self.inset.to_taffy(rem_size, scale_factor),
            size: self.size.to_taffy(rem_size, scale_factor),
            min_size: self.min_size.to_taffy(rem_size, scale_factor),
            max_size: self.max_size.to_taffy(rem_size, scale_factor),
            aspect_ratio: self.aspect_ratio,
            margin: self.margin.to_taffy(rem_size, scale_factor),
            padding: self.padding.to_taffy(rem_size, scale_factor),
            border: border_widths_to_taffy(&self.border_widths, rem_size, scale_factor),
            align_items: self.align_items.map_or(style::AlignItems::NORMAL, Into::into),
            align_self: self.align_self.map(Into::into),
            align_content: self.align_content.map_or(style::AlignContent::NORMAL, Into::into),
            justify_content: self.justify_content.map_or(style::JustifyContent::NORMAL, Into::into),
            gap: self.gap.to_taffy(rem_size, scale_factor),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            grid_template_rows: to_grid_repeat(self.grid_rows),
            grid_template_columns: to_grid_repeat(self.grid_cols),
            grid_row: self.grid_location.as_ref().map_or_default(|location| to_grid_line(&location.row)),
            grid_column: self.grid_location.as_ref().map_or_default(|location| to_grid_line(&location.column)),
            ..Default::default()
        }
    }
}

impl ToTaffy<f32> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> f32 {
        round_to_device_pixel(self.to_pixels(rem_size).0, scale_factor)
    }
}

impl ToTaffy<LengthPercentageAuto> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> LengthPercentageAuto {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => LengthPercentageAuto::auto(),
        }
    }
}

impl ToTaffy<Dimension> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Dimension {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => Dimension::auto(),
        }
    }
}

impl ToTaffy<LengthPercentage> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> LengthPercentage {
        match self {
            DefiniteLength::Absolute(length) => length.to_taffy(rem_size, scale_factor),
            DefiniteLength::Fraction(fraction) => LengthPercentage::percent(*fraction),
        }
    }
}

impl ToTaffy<LengthPercentageAuto> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> LengthPercentageAuto {
        match self {
            DefiniteLength::Absolute(length) => length.to_taffy(rem_size, scale_factor),
            DefiniteLength::Fraction(fraction) => LengthPercentageAuto::percent(*fraction),
        }
    }
}

impl ToTaffy<Dimension> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Dimension {
        match self {
            DefiniteLength::Absolute(length) => length.to_taffy(rem_size, scale_factor),
            DefiniteLength::Fraction(fraction) => Dimension::percent(*fraction),
        }
    }
}

impl ToTaffy<LengthPercentage> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> LengthPercentage {
        LengthPercentage::length(self.to_taffy(rem_size, scale_factor))
    }
}

impl ToTaffy<LengthPercentageAuto> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> LengthPercentageAuto {
        LengthPercentageAuto::length(self.to_taffy(rem_size, scale_factor))
    }
}

impl ToTaffy<Dimension> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Dimension {
        Dimension::length(self.to_taffy(rem_size, scale_factor))
    }
}

impl<T, T2> From<TaffyPoint<T>> for Point<T2>
where
    T: Into<T2>,
    T2: Clone + Debug + Default + PartialEq,
{
    fn from(point: TaffyPoint<T>) -> Point<T2> {
        Point { x: point.x.into(), y: point.y.into() }
    }
}

impl<T, T2> From<Point<T>> for TaffyPoint<T2>
where
    T: Into<T2> + Clone + Debug + Default + PartialEq,
{
    fn from(val: Point<T>) -> Self {
        TaffyPoint { x: val.x.into(), y: val.y.into() }
    }
}

impl<T, U> ToTaffy<TaffySize<U>> for Size<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffySize<U> {
        TaffySize {
            width: self.width.to_taffy(rem_size, scale_factor),
            height: self.height.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> ToTaffy<TaffyRect<U>> for Edges<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffyRect<U> {
        TaffyRect {
            top: self.top.to_taffy(rem_size, scale_factor),
            right: self.right.to_taffy(rem_size, scale_factor),
            bottom: self.bottom.to_taffy(rem_size, scale_factor),
            left: self.left.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> From<TaffySize<T>> for Size<U>
where
    T: Into<U>,
    U: Clone + Debug + Default + PartialEq,
{
    fn from(taffy_size: TaffySize<T>) -> Self {
        Size { width: taffy_size.width.into(), height: taffy_size.height.into() }
    }
}

impl<T, U> From<Size<T>> for TaffySize<U>
where
    T: Into<U> + Clone + Debug + Default + PartialEq,
{
    fn from(size: Size<T>) -> Self {
        TaffySize { width: size.width.into(), height: size.height.into() }
    }
}

/// The space available for an element to be laid out in.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub enum AvailableSpace {
    /// The amount of space available is the specified number of pixels.
    Definite(Pixels),
    /// The amount of space available is indefinite and the node should be laid out under a min-content constraint.
    #[default]
    MinContent,
    /// The amount of space available is indefinite and the node should be laid out under a max-content constraint.
    MaxContent,
}

impl AvailableSpace {
    /// Returns a `Size` with both width and height set to `AvailableSpace::MinContent`.
    ///
    /// This function is useful when you want to create a `Size` with the minimum content constraints
    /// for both dimensions.
    pub const fn min_size() -> Size<Self> {
        Size { width: Self::MinContent, height: Self::MinContent }
    }
}

impl From<AvailableSpace> for TaffyAvailableSpace {
    fn from(space: AvailableSpace) -> TaffyAvailableSpace {
        match space {
            AvailableSpace::Definite(Pixels(value)) => TaffyAvailableSpace::Definite(value),
            AvailableSpace::MinContent => TaffyAvailableSpace::MinContent,
            AvailableSpace::MaxContent => TaffyAvailableSpace::MaxContent,
        }
    }
}

impl From<TaffyAvailableSpace> for AvailableSpace {
    fn from(space: TaffyAvailableSpace) -> AvailableSpace {
        match space {
            TaffyAvailableSpace::Definite(value) => AvailableSpace::Definite(Pixels(value)),
            TaffyAvailableSpace::MinContent => AvailableSpace::MinContent,
            TaffyAvailableSpace::MaxContent => AvailableSpace::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(pixels: Pixels) -> Self {
        AvailableSpace::Definite(pixels)
    }
}

impl From<Size<Pixels>> for Size<AvailableSpace> {
    fn from(size: Size<Pixels>) -> Self {
        Size { width: AvailableSpace::Definite(size.width), height: AvailableSpace::Definite(size.height) }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use taffy::style;

    use super::AvailableSpace;
    use super::LayoutId;
    use super::LayoutStyle;
    use super::MeasureState;
    use super::TaffyLayoutEngine;
    use super::UNREQUESTED_FRAMES;
    use crate::AlignItems;
    use crate::Bounds;
    use crate::Display;
    use crate::ElementId;
    use crate::FlexDirection;
    use crate::FlexWrap;
    use crate::JustifyContent;
    use crate::Pixels;
    use crate::Position;
    use crate::Size;
    use crate::Style;
    use crate::extend_path_hash;
    use crate::point;
    use crate::px;
    use crate::relative;
    use crate::size;

    type TestMeasure = Box<dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>) -> Size<Pixels>>;
    type Engine = TaffyLayoutEngine<TestMeasure>;

    const REM: Pixels = px(16.);
    const VIEWPORT: Size<Pixels> = size(px(300.), px(200.));

    /// Text of a given width that wraps into 10px lines. Like the text
    /// element's measure function, it keeps its last layout for the element to
    /// paint and answers an unconstrained probe from it.
    struct Wrapped {
        width: f32,
        layout: Cell<Option<(Option<Pixels>, Size<Pixels>)>>,
        calls: Rc<Cell<usize>>,
    }

    impl MeasureState for Wrapped {
        const RESETS: bool = true;

        fn reset(&self) {
            self.layout.set(None);
        }
    }

    fn wrapped_measure(state: Rc<Wrapped>) -> TestMeasure {
        Box::new(move |known_dimensions, available_space| {
            state.calls.set(state.calls.get() + 1);
            let wrap_width = known_dimensions.width.or(match available_space.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            if let Some((last_wrap_width, size)) = state.layout.get()
                && (wrap_width.is_none() || wrap_width == last_wrap_width)
            {
                return size;
            }
            let width = wrap_width.map_or(state.width, |wrap_width| state.width.min(wrap_width.0));
            let lines = (state.width / width.max(1.)).ceil().max(1.);
            let size = size(px(width), px(10. * lines));
            state.layout.set(Some((wrap_width, size)));
            size
        })
    }

    #[derive(Clone)]
    enum Kind {
        Container(Vec<Spec>),
        Measured(Size<Pixels>),
        Text(f32),
    }

    #[derive(Clone)]
    struct Spec {
        id: Option<u64>,
        style: Style,
        kind: Kind,
    }

    fn container(style: Style, children: Vec<Spec>) -> Spec {
        Spec { id: None, style, kind: Kind::Container(children) }
    }

    fn text(width: f32) -> Spec {
        Spec { id: None, style: Style::default(), kind: Kind::Text(width) }
    }

    fn sized(width: f32, height: f32) -> Style {
        Style { size: size(px(width).into(), px(height).into()), ..Style::default() }
    }

    fn column() -> Style {
        Style { flex_direction: FlexDirection::Column, ..Style::default() }
    }

    /// What a frame leaves behind: the bounds of every node and the layout
    /// every text would paint. Nodes below a `display: none` node are left
    /// out: elements don't draw their children there, and Taffy only visits
    /// them when something above them is laid out again.
    #[derive(Debug, PartialEq)]
    struct Drawn {
        bounds: Vec<Bounds<Pixels>>,
        texts: Vec<Option<(Option<Pixels>, Size<Pixels>)>>,
    }

    #[derive(Default)]
    struct Requested {
        ids: Vec<LayoutId>,
        texts: Vec<Rc<Wrapped>>,
    }

    #[derive(Clone, Copy)]
    struct Parent {
        scope: u64,
        hidden: bool,
    }

    struct Harness {
        engine: Engine,
        calls: Rc<Cell<usize>>,
    }

    impl Harness {
        fn new() -> Self {
            Self { engine: Engine::new(), calls: Rc::default() }
        }

        fn request(&mut self, spec: &Spec, parent: Parent, rem_size: Pixels, requested: &mut Requested) -> LayoutId {
            let scope = spec.id.map_or(parent.scope, |id| extend_path_hash(parent.scope, &ElementId::Integer(id)));
            let id = match &spec.kind {
                Kind::Container(children) => {
                    let this = Parent { scope, hidden: parent.hidden || spec.style.display == Display::None };
                    let children: Vec<_> =
                        children.iter().map(|child| self.request(child, this, rem_size, requested)).collect();
                    self.engine.request_layout(scope, &spec.style, rem_size, 1., &children)
                }
                Kind::Measured(measured) => {
                    let measured = *measured;
                    self.engine.request_measured_layout(
                        scope,
                        &spec.style,
                        rem_size,
                        1.,
                        Box::new(move |_, _| measured),
                    )
                }
                Kind::Text(width) => {
                    let calls = &self.calls;
                    let (id, state) = self.engine.request_retained_measured_layout(
                        scope,
                        &spec.style,
                        rem_size,
                        1.,
                        |kept: &Wrapped| kept.width == *width,
                        || {
                            let state =
                                Rc::new(Wrapped { width: *width, layout: Cell::new(None), calls: Rc::clone(calls) });
                            (Rc::clone(&state), wrapped_measure(state))
                        },
                    );
                    if !parent.hidden {
                        requested.texts.push(state);
                    }
                    id
                }
            };
            if !parent.hidden {
                requested.ids.push(id);
            }
            id
        }

        /// Draws `roots` as one frame, each under the id path it is paired
        /// with: path 0 fills the viewport like a window root, the others are
        /// laid out in `space` like views and popovers.
        fn frame(&mut self, roots: &[(u64, &Spec)], space: Size<AvailableSpace>, rem_size: Pixels) -> Drawn {
            let mut requested = Requested::default();
            for &(scope, root) in roots {
                let id = self.request(root, Parent { scope, hidden: false }, rem_size, &mut requested);
                if scope == 0 {
                    self.engine.stretch_auto_size_to_fill(id, VIEWPORT, 1.);
                    self.compute(id, VIEWPORT.into());
                } else {
                    self.compute(id, space);
                }
            }
            let drawn = Drawn {
                bounds: requested.ids.iter().map(|id| self.engine.layout_bounds(*id, 1.)).collect(),
                texts: requested.texts.iter().map(|text| text.layout.get()).collect(),
            };
            self.engine.clear();
            drawn
        }

        fn compute(&mut self, id: LayoutId, space: Size<AvailableSpace>) {
            self.engine.compute(id, space, 1., |measure, known_dimensions, available_space| {
                measure(known_dimensions, available_space)
            });
        }

        fn draw(&mut self, root: &Spec) -> Drawn {
            self.frame(&[(0, root)], AvailableSpace::min_size(), REM)
        }

        /// The measure calls made by `draw`.
        fn calls_in(&mut self, draw: impl FnOnce(&mut Self) -> Drawn) -> (usize, Drawn) {
            let before = self.calls.get();
            let drawn = draw(self);
            (self.calls.get() - before, drawn)
        }
    }

    /// Draws each frame with an engine that keeps its nodes and with one that
    /// starts empty, and checks that they leave the same bounds and text layouts.
    fn assert_frames_match_rebuilt(frames: &[Spec]) {
        let mut kept = Harness::new();
        for (ix, frame) in frames.iter().enumerate() {
            assert_eq!(kept.draw(frame), Harness::new().draw(frame), "frame {ix}");
        }
    }

    #[test]
    fn unchanged_frame_is_not_measured_or_computed_again() {
        let root = container(column(), vec![container(sized(80., 40.), vec![text(200.)]), text(50.)]);
        let mut harness = Harness::new();
        let (calls, first) = harness.calls_in(|harness| harness.draw(&root));
        assert!(calls > 0);
        // The 200px text wraps into three lines in the 80px container.
        assert_eq!(first.texts[0], Some((Some(px(80.)), size(px(80.), px(30.)))));

        let (calls, second) = harness.calls_in(|harness| harness.draw(&root));
        assert_eq!(calls, 0);
        assert_eq!(first, second);
        assert_eq!(second, Harness::new().draw(&root));
    }

    #[test]
    fn changed_tree_is_measured_again_and_unchanged_trees_are_not() {
        let tree = |width| container(column(), vec![text(width), text(50.)]);
        let frame = |harness: &mut Harness, width| {
            let trees = [(0, &tree(150.)), (1, &tree(width)), (2, &tree(70.))];
            harness.frame(&trees, size(px(100.), px(100.)).into(), REM)
        };
        let mut harness = Harness::new();
        let (first_calls, _) = harness.calls_in(|harness| frame(harness, 120.));

        // Only the second tree changed. All of its text is measured again:
        // the unchanged one too, since its container was laid out again.
        let (calls, drawn) = harness.calls_in(|harness| frame(harness, 90.));
        assert_eq!(drawn, frame(&mut Harness::new(), 90.));
        assert!(calls > 0 && calls < first_calls, "{calls} calls, {first_calls} in the first frame");
        let second_tree_only = Harness::new().calls_in(|harness| harness.draw(&tree(90.))).0;
        assert!(calls <= second_tree_only);
    }

    #[test]
    fn kept_text_is_laid_out_again_when_its_container_changes() {
        // The text keeps its measure function in every frame, while the width
        // it wraps at changes, returns to one Taffy still has a size cached
        // for, and stops constraining it at all.
        let frame = |width: Option<f32>| {
            let mut style = Style::default();
            if let Some(width) = width {
                style.size.width = px(width).into();
            }
            container(column(), vec![container(style, vec![text(200.)])])
        };
        assert_frames_match_rebuilt(&[
            frame(Some(80.)),
            frame(Some(120.)),
            frame(Some(80.)),
            frame(Some(120.)),
            frame(None),
            frame(Some(80.)),
            frame(None),
        ]);
    }

    #[test]
    fn plain_measure_function_is_called_every_frame() {
        let frame = |height| {
            let measured = Spec { id: None, style: Style::default(), kind: Kind::Measured(size(px(30.), px(height))) };
            container(column(), vec![measured])
        };
        let mut harness = Harness::new();
        assert_eq!(harness.draw(&frame(12.)).bounds[0].size.height, px(12.));
        assert_eq!(harness.draw(&frame(12.)).bounds[0].size.height, px(12.));
        // Nothing says the new function measures as the old one did.
        assert_eq!(harness.draw(&frame(20.)).bounds[0].size.height, px(20.));
    }

    #[test]
    fn stretched_root_keeps_its_layout_and_is_restored_when_reused() {
        let root = container(Style::default(), vec![container(sized(20., 20.), vec![])]);
        let mut harness = Harness::new();
        let first = harness.draw(&root);
        assert_eq!(first.bounds[1], Bounds::new(point(px(0.), px(0.)), VIEWPORT));

        // Stretched to the same viewport, the root isn't laid out again.
        let mut requested = Requested::default();
        let id = harness.request(&root, Parent { scope: 0, hidden: false }, REM, &mut requested);
        harness.engine.stretch_auto_size_to_fill(id, VIEWPORT, 1.);
        assert!(!harness.engine.taffy.dirty(id.0).unwrap());
        harness.engine.clear();

        // Elements without ids are matched by request order, and children are
        // requested first, so this frame's child reuses the root's node.
        assert_frames_match_rebuilt(&[
            container(Style::default(), vec![]),
            container(sized(100., 100.), vec![container(Style::default(), vec![])]),
        ]);
    }

    #[test]
    fn root_is_detached_from_a_parent_no_longer_requested() {
        let mut padded = Style::default();
        padded.padding.left = px(10.).into();
        padded.padding.top = px(10.).into();
        // The second frame's root reuses the first frame's innermost node,
        // whose parent sat inside padding.
        assert_frames_match_rebuilt(&[
            container(padded.clone(), vec![container(padded, vec![container(sized(5., 5.), vec![])])]),
            container(sized(50., 50.), vec![]),
        ]);
    }

    #[test]
    fn hidden_element_is_laid_out_again_when_shown() {
        let frame = |shown: bool| {
            let mut child = sized(20., 20.);
            if !shown {
                child.display = Display::None;
            }
            container(Style::default(), vec![container(Style::default(), vec![container(child, vec![text(40.)])])])
        };
        assert_frames_match_rebuilt(&[frame(false), frame(false), frame(true), frame(false), frame(true)]);
    }

    #[test]
    fn rem_size_changes_resolve_lengths_again() {
        let style = Style { size: size(crate::rems(2.5).into(), crate::rems(1.25).into()), ..Style::default() };
        let root = container(Style::default(), vec![container(style, vec![])]);
        let mut harness = Harness::new();
        for (rem_size, width) in [(16., 40.), (13., 32.), (13., 32.)] {
            let roots = [(0, &root)];
            let drawn = harness.frame(&roots, AvailableSpace::min_size(), px(rem_size));
            assert_eq!(drawn, Harness::new().frame(&roots, AvailableSpace::min_size(), px(rem_size)));
            assert_eq!(drawn.bounds[0].size.width, px(width));
        }
    }

    #[test]
    fn root_computed_twice_in_a_frame_gets_fresh_bounds() {
        let root = container(Style::default(), vec![text(200.)]);
        let mut harness = Harness::new();
        for _ in 0..2 {
            let mut requested = Requested::default();
            let id = harness.request(&root, Parent { scope: 0, hidden: false }, REM, &mut requested);
            harness.compute(id, AvailableSpace::min_size());
            assert_eq!(harness.engine.layout_bounds(id, 1.).size, size(px(200.), px(10.)));
            harness.compute(id, size(px(100.), px(100.)).into());
            assert_eq!(harness.engine.layout_bounds(id, 1.).size, size(px(100.), px(20.)));
            harness.engine.clear();
        }
    }

    #[test]
    fn root_computed_in_two_spaces_is_laid_out_again_in_the_last_one_alone() {
        let root = text(200.);
        let max_content = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
        let mut harness = Harness::new();
        let mut requested = Requested::default();
        let id = harness.request(&root, Parent { scope: 1, hidden: false }, REM, &mut requested);
        harness.compute(id, size(px(80.), px(100.)).into());
        harness.compute(id, max_content);
        // The unconstrained probe is answered from the wrapped layout.
        assert_eq!(harness.engine.layout_bounds(id, 1.).size, size(px(80.), px(30.)));
        harness.engine.clear();

        let second = harness.frame(&[(1, &root)], max_content, REM);
        assert_eq!(second.bounds[0].size, size(px(200.), px(10.)));
        assert_eq!(second, Harness::new().frame(&[(1, &root)], max_content, REM));

        // Laid out in one space now, so the next frame skips it.
        let (calls, third) = harness.calls_in(|harness| harness.frame(&[(1, &root)], max_content, REM));
        assert_eq!(calls, 0);
        assert_eq!(second, third);
    }

    #[test]
    fn reset_drops_sizes_cached_at_another_scale() {
        struct Fixed;
        impl MeasureState for Fixed {}

        let max_content = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
        let mut engine = Engine::new();
        let draw = |engine: &mut Engine, scale_factor: f32| {
            let id = engine.request_retained_measured_layout(
                1,
                &Style::default(),
                REM,
                scale_factor,
                |_: &Fixed| true,
                || (Rc::new(Fixed), Box::new(|_, _| size(px(40.), px(10.))) as TestMeasure),
            );
            engine.compute(id.0, max_content, scale_factor, |measure, known_dimensions, available_space| {
                measure(known_dimensions, available_space)
            });
            let bounds = engine.layout_bounds(id.0, scale_factor);
            engine.clear();
            bounds.size
        };
        assert_eq!(draw(&mut engine, 1.), size(px(40.), px(10.)));
        engine.reset();
        assert_eq!(draw(&mut engine, 2.), size(px(40.), px(10.)));
    }

    #[test]
    fn nodes_unrequested_under_a_requested_path_are_reused_then_removed() {
        let frame = |children: usize| container(Style::default(), vec![container(sized(5., 5.), vec![]); children]);
        let mut harness = Harness::new();
        harness.draw(&frame(4));
        assert_eq!(harness.engine.taffy.total_node_count(), 5);

        // The first two children and the root's node are matched by order. The
        // other two nodes are detached at once, for the next frame to reuse.
        harness.draw(&frame(2));
        assert_eq!(harness.engine.free_nodes.len(), 2);
        // The third child reuses the root's old node, the root takes a free
        // one, and the other free one is removed.
        harness.draw(&frame(3));
        assert_eq!(harness.engine.free_nodes.len(), 0);
        assert_eq!(harness.engine.taffy.total_node_count(), 4);

        harness.draw(&frame(0));
        harness.draw(&frame(0));
        assert_eq!(harness.engine.taffy.total_node_count(), 1);
        assert_eq!(harness.engine.nodes.len(), 1);
    }

    #[test]
    fn nodes_under_an_unrequested_path_are_removed_after_a_while() {
        let frame = |id| Spec {
            id: Some(id),
            ..container(Style::default(), vec![container(sized(5., 5.), vec![]), text(30.)])
        };
        let mut harness = Harness::new();
        let mut node_counts = Vec::new();
        // No node of a frame can be matched in the next one.
        for id in 0..3 * UNREQUESTED_FRAMES {
            harness.draw(&frame(id));
            node_counts.push(harness.engine.taffy.total_node_count());
        }
        let steady = &node_counts[2 * UNREQUESTED_FRAMES as usize..];
        assert!(steady.iter().all(|count| *count == steady[0]), "{steady:?}");
        assert!(steady[0] <= 3 * (UNREQUESTED_FRAMES as usize + 3), "{}", steady[0]);
        assert_eq!(harness.engine.nodes.len(), steady[0]);
        assert!(harness.engine.scopes.len() <= UNREQUESTED_FRAMES as usize + 3);
    }

    #[test]
    fn tree_drawn_again_after_frames_without_it_is_not_laid_out_again() {
        let view = container(column(), vec![container(sized(80., 40.), vec![text(200.)]), text(50.)]);
        let window = container(Style::default(), vec![]);
        let space = size(px(100.), px(100.)).into();
        let mut harness = Harness::new();
        let first = harness.frame(&[(0, &window), (1, &view)], space, REM);
        for _ in 0..10 {
            harness.frame(&[(0, &window)], space, REM);
        }
        let (calls, again) = harness.calls_in(|harness| harness.frame(&[(0, &window), (1, &view)], space, REM));
        assert_eq!(calls, 0);
        assert_eq!(again, first);

        // Changed while it was away, it is laid out like any changed tree.
        for _ in 0..10 {
            harness.frame(&[(0, &window)], space, REM);
        }
        let changed = container(column(), vec![container(sized(60., 40.), vec![text(200.)]), text(50.)]);
        let roots = [(0, &window), (1, &changed)];
        assert_eq!(harness.frame(&roots, space, REM), Harness::new().frame(&roots, space, REM));
    }

    #[test]
    fn plain_measure_function_is_dropped_when_its_path_goes_unrequested() {
        let held = Rc::new(());
        let style = Style::default();
        let mut harness = Harness::new();
        let measure = {
            let held = Rc::clone(&held);
            Box::new(move |_, _| size(px(Rc::strong_count(&held) as f32), px(1.)))
        };
        let id = harness.engine.request_measured_layout(7, &style, REM, 1., measure);
        harness.compute(id, AvailableSpace::min_size());
        harness.engine.clear();
        assert_eq!(Rc::strong_count(&held), 2);
        harness.engine.clear();
        assert_eq!(Rc::strong_count(&held), 1);

        // The node is still there, and no longer sized by the function.
        let id = harness.engine.request_layout(7, &style, REM, 1., &[]);
        harness.compute(id, AvailableSpace::min_size());
        assert_eq!(harness.engine.layout_bounds(id, 1.).size, size(px(0.), px(0.)));
        assert_eq!(harness.engine.taffy.total_node_count(), 1);
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.below(100) < percent
        }

        fn pixels(&mut self, max: u64) -> Pixels {
            px(self.below(max) as f32)
        }

        fn style(&mut self) -> Style {
            let mut style = Style::default();
            if self.chance(35) {
                style.flex_direction = FlexDirection::Column;
            }
            if self.chance(35) {
                style.size.width = if self.chance(25) { relative(0.5).into() } else { self.pixels(220).into() };
            }
            if self.chance(30) {
                style.size.height = if self.chance(25) { relative(0.5).into() } else { self.pixels(120).into() };
            }
            if self.chance(10) {
                style.min_size.width = self.pixels(60).into();
            }
            if self.chance(10) {
                style.max_size.width = self.pixels(150).into();
            }
            if self.chance(20) {
                style.padding.left = self.pixels(9).into();
                style.padding.top = self.pixels(9).into();
            }
            if self.chance(10) {
                style.margin.right = self.pixels(9).into();
            }
            if self.chance(10) {
                style.gap = size(self.pixels(6).into(), self.pixels(6).into());
            }
            if self.chance(15) {
                style.flex_grow = 1.;
            }
            if self.chance(10) {
                style.flex_shrink = 0.;
            }
            if self.chance(10) {
                style.flex_wrap = FlexWrap::Wrap;
            }
            if self.chance(10) {
                style.align_items = Some(AlignItems::Center);
            }
            if self.chance(10) {
                style.justify_content = Some(JustifyContent::SpaceBetween);
            }
            if self.chance(6) {
                style.display = Display::None;
            }
            if self.chance(5) {
                style.display = Display::Block;
            }
            if self.chance(6) {
                style.position = Position::Absolute;
                style.inset.left = self.pixels(30).into();
                style.inset.top = self.pixels(30).into();
            }
            style
        }

        fn spec(&mut self, depth: u32) -> Spec {
            let kind = match self.below(if depth >= 4 { 2 } else { 5 }) {
                0 => Kind::Text((20 + self.below(26) * 10) as f32),
                1 => Kind::Measured(size(self.pixels(90), self.pixels(40))),
                _ => {
                    let children = self.below(4) as usize;
                    Kind::Container(std::iter::repeat_with(|| self.spec(depth + 1)).take(children).collect())
                }
            };
            // Text and measured leaves are styled like the elements that request them.
            let style = if matches!(kind, Kind::Container(_)) { self.style() } else { Style::default() };
            Spec { id: self.chance(30).then(|| self.below(6)), style, kind }
        }

        fn mutate(&mut self, spec: &mut Spec, depth: u32) {
            if self.chance(2) {
                *spec = self.spec(depth);
                return;
            }
            if self.chance(4) {
                spec.id = self.chance(50).then(|| self.below(6));
            }
            match &mut spec.kind {
                Kind::Container(children) => {
                    if self.chance(6) {
                        spec.style = self.style();
                    }
                    if self.chance(6) && depth < 4 {
                        let ix = self.below(children.len() as u64 + 1) as usize;
                        children.insert(ix, self.spec(depth + 1));
                    }
                    if self.chance(6) && !children.is_empty() {
                        children.remove(self.below(children.len() as u64) as usize);
                    }
                    if self.chance(4) && children.len() > 1 {
                        let ix = self.below(children.len() as u64 - 1) as usize;
                        children.swap(ix, ix + 1);
                    }
                    for child in children {
                        self.mutate(child, depth + 1);
                    }
                }
                Kind::Measured(measured) => {
                    if self.chance(8) {
                        *measured = size(self.pixels(90), self.pixels(40));
                    }
                }
                Kind::Text(width) => {
                    if self.chance(8) {
                        *width = (20 + self.below(26) * 10) as f32;
                    }
                }
            }
        }
    }

    #[test]
    fn random_frames_match_a_rebuilt_tree() {
        for seed in 1..=300 {
            let mut rng = Rng(u64::wrapping_mul(seed, 0x9E37_79B9_7F4A_7C15));
            // The window root, then two views: a clean view isn't drawn.
            let mut roots: Vec<Spec> = std::iter::repeat_with(|| {
                container(rng.style(), std::iter::repeat_with(|| rng.spec(1)).take(3).collect())
            })
            .take(3)
            .collect();
            let mut space = AvailableSpace::min_size();
            let mut rem_size = REM;
            let mut kept = Harness::new();
            for frame in 0..80 {
                // Most frames change little or nothing, as most real frames do.
                if rng.chance(60) {
                    for root in &mut roots {
                        rng.mutate(root, 0);
                    }
                }
                if rng.chance(10) {
                    space = match rng.below(3) {
                        0 => AvailableSpace::min_size(),
                        1 => size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                        _ => size(rng.pixels(250), rng.pixels(250)).into(),
                    };
                }
                if rng.chance(3) {
                    rem_size = px(12. + rng.below(8) as f32);
                }
                let drawn_roots: Vec<(u64, &Spec)> = roots
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| *ix == 0 || rng.chance(60))
                    .map(|(ix, root)| (ix as u64, root))
                    .collect();
                let drawn = kept.frame(&drawn_roots, space, rem_size);
                let rebuilt = Harness::new().frame(&drawn_roots, space, rem_size);
                assert_eq!(drawn, rebuilt, "seed {seed}, frame {frame}");
            }
        }
    }

    #[test]
    fn style_to_taffy_maps_unset_alignment_to_normal() {
        let taffy_style = LayoutStyle::new(&Style::default(), px(16.), 1.).to_taffy();
        assert_eq!(taffy_style.align_items, style::AlignItems::NORMAL);
        assert_eq!(taffy_style.align_content, style::AlignContent::NORMAL);
        assert_eq!(taffy_style.justify_content, style::JustifyContent::NORMAL);
    }

    #[test]
    fn style_to_taffy_maps_set_alignment() {
        let gpui_style = Style {
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::SpaceBetween),
            ..Style::default()
        };
        let taffy_style = LayoutStyle::new(&gpui_style, px(16.), 1.).to_taffy();
        assert_eq!(taffy_style.align_items, style::AlignItems::CENTER);
        assert_eq!(taffy_style.justify_content, style::JustifyContent::SPACE_BETWEEN);
    }

    #[test]
    fn available_space_min_size() {
        let min_content_size = AvailableSpace::min_size();
        assert_eq!(min_content_size.width, AvailableSpace::MinContent);
        assert_eq!(min_content_size.height, AvailableSpace::MinContent);
    }
}
