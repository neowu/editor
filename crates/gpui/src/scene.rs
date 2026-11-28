use std::fmt::Debug;
use std::iter::Peekable;
use std::ops::Add;
use std::ops::Range;
use std::ops::Sub;
use std::slice;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::AtlasTextureId;
use crate::AtlasTile;
use crate::Background;
use crate::Bounds;
// lint: expect(module_cycle)
use crate::ContentMask;
use crate::Corners;
use crate::Edges;
use crate::Hsla;
use crate::Pixels;
use crate::Point;
use crate::Radians;
use crate::ScaledPixels;
use crate::Size;
// lint: expect(module_cycle)
use crate::Transformation;
use crate::bounds_tree::BoundsTree;
use crate::point;

pub(crate) type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub(crate) struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

#[derive(Default)]
pub struct Scene {
    paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    animations: Vec<SpriteAnimation>,
}

impl Scene {
    pub(crate) fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.polychrome_sprites.clear();
        self.animations.clear();
    }

    /// Inserts a sprite that [`Self::advance_animations`] keeps turning.
    pub(crate) fn insert_animated_sprite(&mut self, mut sprite: MonochromeSprite, animation: SpriteAnimation) {
        sprite.animation = self.animations.len() as u32 + 1;
        let sprite_count = self.monochrome_sprites.len();
        self.insert_primitive(sprite);
        // A sprite that is clipped away isn't inserted.
        if self.monochrome_sprites.len() > sprite_count {
            self.animations.push(animation);
        }
    }

    pub(crate) const fn has_animations(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Moves every animated sprite to its pose at `elapsed` time on the app's
    /// synced animation clock, without the scene being rebuilt.
    pub(crate) fn advance_animations(&mut self, elapsed: Duration) {
        for animation in &self.animations {
            if let Some(sprite) = self.monochrome_sprites.get_mut(animation.sprite_index) {
                sprite.transformation = animation
                    .base
                    // lint: expect(module_cycle)
                    .turned(animation.period, elapsed)
                    // lint: expect(module_cycle)
                    .into_matrix(animation.center, animation.scale_factor);
            }
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.paint_operations.is_empty()
    }

    pub(crate) fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations.push(PaintOperation::StartLayer(bounds));
    }

    pub(crate) fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub(crate) fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let mut primitive = primitive.into();
        // lint: expect(module_cycle)
        let clipped_bounds = primitive.bounds().intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return;
        }

        let order = self.layer_stack.last().copied().unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
        }
        self.paint_operations.push(PaintOperation::Primitive(primitive));
    }

    pub(crate) fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    // Animation ids index the scene they were inserted into.
                    if let Primitive::MonochromeSprite(sprite) = primitive
                        && let Some(animation) = prev_scene.animations.get((sprite.animation as usize).wrapping_sub(1))
                    {
                        self.insert_animated_sprite(*sprite, *animation);
                    } else {
                        self.insert_primitive(primitive.clone());
                    }
                }
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    pub(crate) fn finish(&mut self) {
        let keys = &mut Vec::new();
        sort_in_place(&mut self.shadows, keys, |shadow| (shadow.order, 0));
        sort_in_place(&mut self.quads, keys, |quad| (quad.order, 0));
        // Paths aren't `Copy` and there are few of them.
        self.paths.sort_by_key(|path| path.order);
        sort_in_place(&mut self.underlines, keys, |underline| (underline.order, 0));
        sort_in_place(&mut self.monochrome_sprites, keys, |sprite| (sprite.order, sprite.tile.texture_id.index));
        sort_in_place(&mut self.polychrome_sprites, keys, |sprite| (sprite.order, sprite.tile.texture_id.index));

        // Sorting moved the sprites, so animations learn where theirs ended up.
        if !self.animations.is_empty() {
            for (index, sprite) in self.monochrome_sprites.iter().enumerate() {
                if let Some(animation) = self.animations.get_mut((sprite.animation as usize).wrapping_sub(1)) {
                    animation.sprite_index = index;
                }
            }
        }
    }

    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]

enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    PolychromeSprite,
}

enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    PolychromeSprite(PolychromeSprite),
}

impl Primitive {
    const fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
        }
    }

    // lint: expect(module_cycle)
    const fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
        }
    }
}

struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
}

impl Iterator for BatchIterator<'_> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (self.shadows_iter.peek().map(|s| s.order), PrimitiveKind::Shadow),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (self.underlines_iter.peek().map(|u| u.order), PrimitiveKind::Underline),
            (self.monochrome_sprites_iter.peek().map(|s| s.order), PrimitiveKind::MonochromeSprite),
            (self.polychrome_sprites_iter.peek().map(|s| s.order), PrimitiveKind::PolychromeSprite),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self.shadows_iter.next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind).is_some() {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self.quads_iter.next_if(|quad| (quad.order, batch_kind) < max_order_and_kind).is_some() {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self.paths_iter.next_if(|path| (path.order, batch_kind) < max_order_and_kind).is_some() {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id =
                    self.monochrome_sprites_iter.peek().expect("batch kind came from a peeked sprite").tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites { texture_id, range: sprites_start..sprites_end })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id =
                    self.polychrome_sprites_iter.peek().expect("batch kind came from a peeked sprite").tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites { texture_id, range: sprites_start..sprites_end })
            }
        }
    }
}

#[derive(Debug)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites { texture_id: AtlasTextureId, range: Range<usize> },
    PolychromeSprites { texture_id: AtlasTextureId, range: Range<usize> },
}

impl PrimitiveBatch {
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!("monochrome sprites ({}) on atlas {}", range.len(), texture_id.index)
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!("polychrome sprites ({}) on atlas {}", range.len(), texture_id.index)
            }
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
pub struct Quad {
    pub(crate) order: DrawOrder,
    pub(crate) border_style: BorderStyle,
    pub(crate) bounds: Bounds<ScaledPixels>,
    // lint: expect(module_cycle)
    pub(crate) content_mask: ContentMask<ScaledPixels>,
    pub(crate) background: Background,
    pub(crate) border_color: Hsla,
    pub(crate) corner_radii: Corners<ScaledPixels>,
    pub(crate) border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct Underline {
    pub(crate) order: DrawOrder,
    pub(crate) pad: u32, // align to 8 bytes
    pub(crate) bounds: Bounds<ScaledPixels>,
    // lint: expect(module_cycle)
    pub(crate) content_mask: ContentMask<ScaledPixels>,
    pub(crate) color: Hsla,
    pub(crate) thickness: ScaledPixels,
    pub(crate) wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct Shadow {
    pub(crate) order: DrawOrder,
    pub(crate) blur_radius: ScaledPixels,
    pub(crate) bounds: Bounds<ScaledPixels>,
    pub(crate) corner_radii: Corners<ScaledPixels>,
    // lint: expect(module_cycle)
    pub(crate) content_mask: ContentMask<ScaledPixels>,
    pub(crate) color: Hsla,
    pub(crate) element_bounds: Bounds<ScaledPixels>,
    pub(crate) element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub(crate) inset: u32,
    pub(crate) pad: u32, // align to 8 bytes
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub(crate) struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major.
    rotation_scale: [[f32; 2]; 2],
    /// translation vector.
    translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub(crate) const fn unit() -> Self {
        Self { rotation_scale: [[1.0, 0.0], [0.0, 1.0]], translation: [0.0, 0.0] }
    }

    /// Move the origin by a given point.
    pub(crate) fn translate(self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self { rotation_scale: [[1.0, 0.0], [0.0, 1.0]], translation: [point.x.0, point.y.0] })
    }

    /// Clockwise rotation in radians around the origin.
    pub(crate) fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [[angle.0.cos(), -angle.0.sin()], [angle.0.sin(), angle.0.cos()]],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin.
    pub(crate) fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self { rotation_scale: [[size.width, 0.0], [0.0, size.height]], translation: [0.0, 0.0] })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct MonochromeSprite {
    pub(crate) order: DrawOrder,
    /// One more than the index of the sprite's [`SpriteAnimation`] in its scene, or 0 for none.
    /// It sits in the instance's alignment padding; renderers ignore it.
    pub(crate) animation: u32,
    pub(crate) tile: AtlasTile,
    pub(crate) bounds: Bounds<ScaledPixels>,
    // lint: expect(module_cycle)
    pub(crate) content_mask: ContentMask<ScaledPixels>,
    pub(crate) color: Hsla,
    pub(crate) transformation: TransformationMatrix,
}

/// A continuous rotation the window applies to a sprite right before every
/// present, so the scene can be presented again without being redrawn.
#[derive(Copy, Clone, Debug)]
pub(crate) struct SpriteAnimation {
    // lint: expect(module_cycle)
    base: Transformation,
    period: Duration,
    center: Point<Pixels>,
    scale_factor: f32,
    sprite_index: usize,
}

impl SpriteAnimation {
    /// `period` must not be zero.
    // lint: expect(module_cycle)
    pub(crate) const fn new(base: Transformation, period: Duration, center: Point<Pixels>, scale_factor: f32) -> Self {
        Self { base, period, center, scale_factor, sprite_index: usize::MAX }
    }
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PolychromeSprite {
    pub(crate) order: DrawOrder,
    pub(crate) pad: u32,
    pub(crate) tile: AtlasTile,
    pub(crate) grayscale: PaddedBool32,
    pub(crate) opacity: f32,
    pub(crate) bounds: Bounds<ScaledPixels>,
    // lint: expect(module_cycle)
    pub(crate) content_mask: ContentMask<ScaledPixels>,
    pub(crate) corner_radii: Corners<ScaledPixels>,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct PathId(usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    // lint: expect(module_cycle)
    pub content_mask: ContentMask<P>,
    pub vertices: Vec<PathVertex<P>>,
    pub color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub(crate) fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds { origin: start, size: Size::default() },
            // lint: expect(module_cycle)
            content_mask: ContentMask::default(),
            color: Background::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub(crate) fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            // lint: expect(module_cycle)
            content_mask: self.content_mask.scale(factor),
            vertices: self.vertices.iter().map(|vertex| vertex.scale(factor)).collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub const fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle((self.start, self.current, to), (point(0., 1.), point(0., 1.), point(0., 1.)));
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle((self.start, self.current, to), (point(0., 1.), point(0., 1.), point(0., 1.)));
        }

        self.push_triangle((self.current, ctrl, to), (point(0., 0.), point(0.5, 0.), point(1., 1.)));
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub(crate) fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds { origin: xy.0, size: Size::default() })
            .union(&Bounds { origin: xy.1, size: Size::default() })
            .union(&Bounds { origin: xy.2, size: Size::default() });

        // lint: expect(module_cycle)
        self.vertices.push(PathVertex { xy_position: xy.0, st_position: st.0, content_mask: ContentMask::default() });
        // lint: expect(module_cycle)
        self.vertices.push(PathVertex { xy_position: xy.1, st_position: st.1, content_mask: ContentMask::default() });
        // lint: expect(module_cycle)
        self.vertices.push(PathVertex { xy_position: xy.2, st_position: st.2, content_mask: ContentMask::default() });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    pub fn clipped_bounds(&self) -> Bounds<T> {
        // lint: expect(module_cycle)
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    // lint: expect(module_cycle)
    content_mask: ContentMask<P>,
}

impl PathVertex<Pixels> {
    fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            // lint: expect(module_cycle)
            content_mask: self.content_mask.scale(factor),
        }
    }
}

/// Sorts small `(key, index)` pairs instead of the primitives themselves, then moves each
/// primitive into place once. Primitives are large, so a comparison sort that shuffles them
/// directly spends most of its time copying. Ties keep insertion order, as `sort_by_key`
/// does, because the index is part of what is sorted.
fn sort_in_place<T: Copy>(primitives: &mut [T], keys: &mut Vec<(u64, u32)>, key: impl Fn(&T) -> (u32, u32)) {
    if primitives.is_sorted_by_key(&key) {
        return;
    }
    keys.clear();
    keys.extend(primitives.iter().enumerate().map(|(index, primitive)| {
        let (major, minor) = key(primitive);
        ((u64::from(major) << 32) | u64::from(minor), index as u32)
    }));
    keys.sort_unstable();

    // `keys[target].1` is the source index for `target`. Follow each cycle of that
    // permutation, marking visited slots by pointing them at themselves.
    for start in 0..primitives.len() {
        if keys[start].1 as usize == start {
            continue;
        }
        let displaced = primitives[start];
        let mut target = start;
        loop {
            let source = keys[target].1 as usize;
            keys[target].1 = target as u32;
            if source == start {
                primitives[target] = displaced;
                break;
            }
            primitives[target] = primitives[source];
            target = source;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::MonochromeSprite;
    use super::Scene;
    use super::SpriteAnimation;
    use super::TransformationMatrix;
    use super::sort_in_place;
    use crate::AtlasTextureId;
    use crate::AtlasTextureKind;
    use crate::AtlasTile;
    use crate::Bounds;
    use crate::ContentMask;
    use crate::DevicePixels;
    use crate::Hsla;
    use crate::ScaledPixels;
    use crate::TileId;
    use crate::Transformation;
    use crate::point;
    use crate::px;
    use crate::size;

    fn sprite(x: f32, visible: bool) -> MonochromeSprite {
        let bounds = Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(0.)),
            size: size(ScaledPixels(16.), ScaledPixels(16.)),
        };
        MonochromeSprite {
            order: 0,
            animation: 0,
            tile: AtlasTile {
                texture_id: AtlasTextureId { index: 0, kind: AtlasTextureKind::Monochrome },
                tile_id: TileId(0),
                padding: 0,
                bounds: Bounds::<DevicePixels>::default(),
            },
            bounds,
            content_mask: ContentMask { bounds: if visible { bounds } else { Bounds::default() } },
            color: Hsla::default(),
            transformation: TransformationMatrix::unit(),
        }
    }

    fn spin() -> SpriteAnimation {
        SpriteAnimation::new(Transformation::default(), Duration::from_secs(2), point(px(0.), px(0.)), 1.)
    }

    fn animated_sprite_xs(scene: &Scene) -> Vec<f32> {
        scene
            .monochrome_sprites
            .iter()
            .filter(|sprite| sprite.animation != 0)
            .map(|sprite| sprite.bounds.origin.x.0)
            .collect()
    }

    #[test]
    fn animated_sprites_turn_in_place_and_survive_replay() {
        let mut scene = Scene::default();
        // Overlapping sprites get increasing orders, so sorting keeps this order.
        scene.insert_primitive(sprite(0., true));
        scene.insert_animated_sprite(sprite(100., false), spin());
        scene.insert_animated_sprite(sprite(4., true), spin());
        scene.insert_primitive(sprite(8., true));
        assert!(scene.has_animations());
        assert_eq!(animated_sprite_xs(&scene), [4.], "a clipped sprite takes no animation");

        // A cached view's primitives are replayed into the next scene, where
        // animation ids are assigned afresh.
        let mut next = Scene::default();
        next.insert_animated_sprite(sprite(200., true), spin());
        next.replay(0..scene.len(), &scene);
        next.finish();
        assert_eq!(animated_sprite_xs(&next), [200., 4.]);

        // A quarter turn: only the animated sprites move.
        next.advance_animations(Duration::from_millis(500));
        for sprite in &next.monochrome_sprites {
            let [[cos, _], [sin, _]] = sprite.transformation.rotation_scale;
            let expected = if sprite.animation == 0 { (1., 0.) } else { (0., 1.) };
            assert!((cos - expected.0).abs() < 1e-5 && (sin - expected.1).abs() < 1e-5);
        }

        next.clear();
        assert!(!next.has_animations());
    }

    #[test]
    fn sort_in_place_matches_stable_sort_by_key() {
        // (key, id) pairs; ties on key must keep id order.
        let mut seed = 0x9e37_79b9_u32;
        let mut items: Vec<(u32, u32)> = (0..500)
            .map(|id| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed % 37, id)
            })
            .collect();
        let mut expected = items.clone();
        expected.sort_by_key(|item| item.0);

        sort_in_place(&mut items, &mut Vec::new(), |item| (item.0, 0));
        assert_eq!(items, expected);

        // Already sorted input is left alone.
        sort_in_place(&mut items, &mut Vec::new(), |item| (item.0, 0));
        assert_eq!(items, expected);
    }
}
