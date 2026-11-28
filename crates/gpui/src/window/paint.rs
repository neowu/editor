use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use smallvec::SmallVec;

use super::ContentMask;
use super::Window;
use crate::App;
use crate::AtlasTile;
use crate::Background;
use crate::BorderStyle;
use crate::Bounds;
use crate::BoxShadow;
use crate::Corners;
use crate::DevicePixels;
use crate::Edges;
use crate::FontId;
use crate::GlyphId;
use crate::Hsla;
use crate::IsZero as _;
use crate::MonochromeSprite;
use crate::Path;
use crate::Pixels;
use crate::Point;
use crate::PolychromeSprite;
use crate::Quad;
use crate::RenderGlyphParams;
use crate::RenderImage;
use crate::RenderImageParams;
use crate::RenderSvgParams;
use crate::SMOOTH_SVG_SCALE_FACTOR;
use crate::SUBPIXEL_VARIANTS_X;
use crate::SUBPIXEL_VARIANTS_Y;
use crate::ScaledPixels;
use crate::Shadow;
use crate::SharedString;
use crate::SpriteAnimation;
use crate::StrikethroughStyle;
use crate::TransformationMatrix;
use crate::Underline;
use crate::UnderlineStyle;
use crate::point;
use crate::px;
use crate::size;
use crate::transparent_black;
use crate::util::ceil_to_device_pixel;
use crate::util::round_half_toward_zero;
use crate::util::round_stroke_to_device_pixel;
use crate::util::round_to_device_pixel;

impl Window {
    /// Rounds a logical value to the nearest device pixel.
    #[inline]
    pub(crate) fn pixel_snap(&self, value: Pixels) -> Pixels {
        px(round_to_device_pixel(value.0, self.scale_factor()) / self.scale_factor())
    }

    /// f64 variant of [`Self::pixel_snap`].
    #[inline]
    pub fn pixel_snap_f64(&self, value: f64) -> f64 {
        let scale_factor = f64::from(self.scale_factor());
        round_half_toward_zero_f64(value * scale_factor) / scale_factor
    }

    /// Snaps a bounds' origin and size to the nearest device pixel.
    #[inline]
    pub fn pixel_snap_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        bounds.map(|c| self.pixel_snap(c))
    }

    /// Snaps a point's coordinates to the nearest device pixel.
    #[inline]
    pub(crate) fn pixel_snap_point(&self, position: Point<Pixels>) -> Point<Pixels> {
        position.map(|c| self.pixel_snap(c))
    }

    /// Returns the snapped device-space bounds used to paint an underline.
    pub fn underline_bounds(
        &self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &UnderlineStyle,
    ) -> Bounds<ScaledPixels> {
        let scale_factor = self.scale_factor();
        let thickness = self.snap_stroke(style.thickness);
        let height = if style.wavy { ScaledPixels(thickness.0 * 3.) } else { thickness };
        Bounds {
            origin: origin.map(|c| ScaledPixels(round_to_device_pixel(c.0, scale_factor))),
            size: size(self.snap_stroke(width), height),
        }
    }

    /// Paints an underline excluding absolute device-space horizontal spans.
    pub fn paint_underline_with_exclusions(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &UnderlineStyle,
        exclusions: &[Range<ScaledPixels>],
    ) {
        let underline = self.underline(origin, width, style);
        if exclusions.is_empty() {
            self.next_frame.scene.insert_primitive(underline);
            return;
        }

        let bounds = underline.bounds.intersect(&underline.content_mask.bounds);
        if bounds.is_empty() {
            return;
        }

        let mut exclusions = exclusions
            .iter()
            .filter_map(|span| {
                let start = span.start.max(bounds.left());
                let end = span.end.min(bounds.right());
                (start < end).then_some(start..end)
            })
            .collect::<SmallVec<[_; 4]>>();
        if exclusions.is_empty() {
            self.next_frame.scene.insert_primitive(underline);
            return;
        }
        exclusions.sort_unstable_by_key(|span| span.start);

        let mut paint_span = |start, end| {
            self.next_frame.scene.insert_primitive(Underline {
                content_mask: ContentMask {
                    bounds: Bounds::from_corners(point(start, bounds.top()), point(end, bounds.bottom())),
                },
                ..underline
            });
        };
        let mut start = bounds.left();
        for exclusion in exclusions {
            if start < exclusion.start {
                paint_span(start, exclusion.start);
            }
            start = start.max(exclusion.end);
        }
        if start < bounds.right() {
            paint_span(start, bounds.right());
        }
    }

    fn underline(&self, origin: Point<Pixels>, width: Pixels, style: &UnderlineStyle) -> Underline {
        Underline {
            order: 0,
            pad: 0,
            bounds: self.underline_bounds(origin, width, style),
            content_mask: self.snapped_content_mask(),
            color: style.color.unwrap_or_default().opacity(self.element_opacity()),
            thickness: self.snap_stroke(style.thickness),
            wavy: style.wavy.into(),
        }
    }

    #[inline]
    fn snap_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<ScaledPixels> {
        let scale_factor = self.scale_factor();
        let left = round_to_device_pixel(bounds.left().0, scale_factor);
        let top = round_to_device_pixel(bounds.top().0, scale_factor);
        let right = round_to_device_pixel(bounds.right().0, scale_factor).max(left);
        let bottom = round_to_device_pixel(bounds.bottom().0, scale_factor).max(top);
        Bounds::from_corners(
            point(ScaledPixels(left), ScaledPixels(top)),
            point(ScaledPixels(right), ScaledPixels(bottom)),
        )
    }

    /// Rounds half-to-zero but clamps any non-zero input up to 1 dp so thin strokes do not disappear.
    #[inline]
    fn snap_stroke(&self, value: Pixels) -> ScaledPixels {
        ScaledPixels(round_stroke_to_device_pixel(value.0, self.scale_factor()))
    }

    #[inline]
    fn snap_border_widths(&self, edges: Edges<Pixels>) -> Edges<ScaledPixels> {
        edges.map(|e| self.snap_stroke(*e))
    }

    /// Floors the near edge and ceils the far edge, producing a strict superset of the raw region.
    #[inline]
    fn cover_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<ScaledPixels> {
        cover_bounds(bounds, self.scale_factor())
    }

    #[inline]
    fn snapped_content_mask(&self) -> ContentMask<ScaledPixels> {
        ContentMask { bounds: self.cover_bounds(self.content_mask().bounds) }
    }

    /// Creates a new painting layer for the specified bounds. A "layer" is a batch
    /// of geometry that are non-overlapping and have the same draw order. This is typically used
    /// for performance reasons.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        let content_mask = self.content_mask();
        let clipped_bounds = bounds.intersect(&content_mask.bounds);
        if !clipped_bounds.is_empty() {
            self.next_frame.scene.push_layer(self.cover_bounds(clipped_bounds));
        }

        let result = f(self);

        if !clipped_bounds.is_empty() {
            self.next_frame.scene.pop_layer();
        }

        result
    }

    /// Paint the drop (non-inset) shadows from `shadows` into the scene at the current
    /// z-index. Inset shadows are skipped; paint those with [`Self::paint_inset_shadows`]
    /// after the element's background so they layer on top of the fill.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_drop_shadows(&mut self, bounds: Bounds<Pixels>, corner_radii: Corners<Pixels>, shadows: &[BoxShadow]) {
        let scale_factor = self.scale_factor();
        let content_mask = self.snapped_content_mask();
        let opacity = self.element_opacity();
        let element_bounds = self.cover_bounds(bounds);
        let element_corner_radii = corner_radii.scale(scale_factor);
        for shadow in shadows {
            if shadow.inset {
                continue;
            }
            let shadow_bounds = (bounds + shadow.offset).dilate(shadow.spread_radius);
            self.next_frame.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: self.cover_bounds(shadow_bounds),
                content_mask,
                corner_radii: corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
                element_bounds,
                element_corner_radii,
                inset: 0,
                pad: 0,
            });
        }
    }

    /// Paint the inset shadows from `shadows` into the scene at the current z-index. Should
    /// be called after the element's background so the shadow layers on top of the fill.
    /// Drop shadows are skipped; paint those with [`Self::paint_drop_shadows`] before the background.
    pub(crate) fn paint_inset_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        shadows: &[BoxShadow],
    ) {
        let scale_factor = self.scale_factor();
        let content_mask = self.snapped_content_mask();
        let opacity = self.element_opacity();
        let element_bounds = self.cover_bounds(bounds);
        let element_corner_radii = corner_radii.scale(scale_factor);
        for shadow in shadows {
            if !shadow.inset {
                continue;
            }
            let hole = (bounds + shadow.offset).dilate(-shadow.spread_radius);
            // Clamp at zero so a large spread can't produce negative radii, which would
            // break the SDF in the shader.
            let zero = Pixels::ZERO;
            let hole_corner_radii = Corners {
                top_left: (corner_radii.top_left - shadow.spread_radius).max(zero),
                top_right: (corner_radii.top_right - shadow.spread_radius).max(zero),
                bottom_right: (corner_radii.bottom_right - shadow.spread_radius).max(zero),
                bottom_left: (corner_radii.bottom_left - shadow.spread_radius).max(zero),
            };
            self.next_frame.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: self.cover_bounds(hole),
                content_mask,
                corner_radii: hole_corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
                element_bounds,
                element_corner_radii,
                inset: 1,
                pad: 0,
            });
        }
    }

    fn largest_border_interior(quad: &Quad) -> Bounds<ScaledPixels> {
        let radii = &quad.corner_radii;
        let widths = &quad.border_widths;
        let edge_radii = Edges {
            top: radii.top_left.max(radii.top_right),
            right: radii.top_right.max(radii.bottom_right),
            bottom: radii.bottom_left.max(radii.bottom_right),
            left: radii.top_left.max(radii.bottom_left),
        };

        let antialias_inset = point(ScaledPixels(1.0), ScaledPixels(1.0));
        let inset_bounds = |top_left_inset, bottom_right_inset| {
            Bounds::from_corners(
                quad.bounds.origin + top_left_inset + antialias_inset,
                quad.bounds.bottom_right() - bottom_right_inset - antialias_inset,
            )
        };

        // Rounded corners need only be excluded on one axis. Either candidate
        // is empty of border pixels, so use the larger interior.
        let horizontal_band = inset_bounds(
            point(widths.left, widths.top.max(edge_radii.top)),
            point(widths.right, widths.bottom.max(edge_radii.bottom)),
        );
        let vertical_band = inset_bounds(
            point(widths.left.max(edge_radii.left), widths.top),
            point(widths.right.max(edge_radii.right), widths.bottom),
        );

        let area = |bounds: &Bounds<ScaledPixels>| bounds.size.width.0.max(0.) * bounds.size.height.0.max(0.);
        if area(&horizontal_band) >= area(&vertical_band) { horizontal_band } else { vertical_band }
    }

    /// Paint one or more quads into the scene for the next frame at the current stacking context.
    /// Quads are colored rectangular regions with an optional background, border, and corner radius.
    /// see [`fill`], [`outline`], and [`quad`] to construct this type.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// Note that the `quad.corner_radii` are allowed to exceed the bounds, creating sharp corners
    /// where the circular arcs meet. This will not display well when combined with dashed borders.
    /// Use `Corners::clamp_radii_for_quad_size` if the radii should fit within the bounds.
    pub fn paint_quad(&mut self, quad: PaintQuad) {
        let opacity = self.element_opacity();
        let snapped_bounds = self.snap_bounds(quad.bounds);
        let snapped_border_widths = self.snap_border_widths(quad.border_widths);
        let quad = Quad {
            order: 0,
            bounds: snapped_bounds,
            content_mask: self.snapped_content_mask(),
            background: quad.background.opacity(opacity),
            border_color: quad.border_color.opacity(opacity),
            corner_radii: quad.corner_radii.scale(self.scale_factor()),
            border_widths: snapped_border_widths,
            border_style: quad.border_style,
        };

        if !quad.background.is_transparent() {
            self.next_frame.scene.insert_primitive(quad);
            return;
        }

        // Splitting a border-only quad around its empty interior avoids shading
        // every transparent pixel inside large outlines.
        let outer_bounds = quad.bounds;
        let inner_bounds = Self::largest_border_interior(&quad);

        if inner_bounds.is_empty() {
            self.next_frame.scene.insert_primitive(quad);
            return;
        }

        let strips = [
            // Top
            Bounds::from_corners(outer_bounds.origin, point(outer_bounds.right(), inner_bounds.top())),
            // Bottom
            Bounds::from_corners(point(outer_bounds.left(), inner_bounds.bottom()), outer_bounds.bottom_right()),
            // Left
            Bounds::from_corners(point(outer_bounds.left(), inner_bounds.top()), inner_bounds.bottom_left()),
            // Right
            Bounds::from_corners(inner_bounds.top_right(), point(outer_bounds.right(), inner_bounds.bottom())),
        ];

        for strip in strips {
            let content_mask_bounds = quad.content_mask.bounds.intersect(&strip);
            if !content_mask_bounds.is_empty() {
                self.next_frame
                    .scene
                    .insert_primitive(Quad { content_mask: ContentMask { bounds: content_mask_bounds }, ..quad });
            }
        }
    }

    /// Paint the given `Path` into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_path(&mut self, mut path: Path<Pixels>, color: impl Into<Background>) {
        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        path.content_mask = content_mask;
        let color: Background = color.into();
        path.color = color.opacity(opacity);
        self.next_frame.scene.insert_primitive(path.scale(scale_factor));
    }

    /// Paint an underline into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_underline(&mut self, origin: Point<Pixels>, width: Pixels, style: &UnderlineStyle) {
        let underline = self.underline(origin, width, style);
        self.next_frame.scene.insert_primitive(underline);
    }

    /// Paint a strikethrough into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub(crate) fn paint_strikethrough(&mut self, origin: Point<Pixels>, width: Pixels, style: &StrikethroughStyle) {
        let scale_factor = self.scale_factor();
        let height = style.thickness;
        let bounds = Bounds {
            origin: origin.map(|c| ScaledPixels(round_to_device_pixel(c.0, scale_factor))),
            size: size(self.snap_stroke(width), self.snap_stroke(height)),
        };
        let opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds,
            content_mask: self.snapped_content_mask(),
            thickness: self.snap_stroke(style.thickness),
            color: style.color.unwrap_or_default().opacity(opacity),
            wavy: false.into(),
        });
    }

    /// Paints a monochrome (non-emoji) glyph into the scene for the next frame at the current z-index.
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single glyph that has already been shaped.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub(crate) fn paint_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
    ) -> Result<()> {
        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);

        let quantized_origin = Point::new(
            round_half_toward_zero(glyph_origin.x.0 * SUBPIXEL_VARIANTS_X as f32) / SUBPIXEL_VARIANTS_X as f32,
            round_half_toward_zero(glyph_origin.y.0 * SUBPIXEL_VARIANTS_Y as f32) / SUBPIXEL_VARIANTS_Y as f32,
        );
        let subpixel_variant = Point::new(
            (quantized_origin.x.fract() * SUBPIXEL_VARIANTS_X as f32) as u8,
            (quantized_origin.y.fract() * SUBPIXEL_VARIANTS_Y as f32) as u8,
        );
        let integer_origin = quantized_origin.map(|c| ScaledPixels(c.trunc()));
        let dilation = self.text_system().glyph_dilation_for_color(color);
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor,
            is_emoji: false,
            dilation,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .sprite_atlas
                .get_or_insert_with(params.clone().into(), &mut || {
                    let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");
            let bounds = Bounds {
                origin: integer_origin + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.snapped_content_mask();

            self.next_frame.scene.insert_primitive(MonochromeSprite {
                order: 0,
                animation: 0,
                bounds,
                content_mask,
                color: color.opacity(element_opacity),
                tile,
                transformation: TransformationMatrix::unit(),
            });
        }
        Ok(())
    }

    /// Paints an emoji glyph into the scene for the next frame at the current z-index.
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single emoji that has already been shaped.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub(crate) fn paint_emoji(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) -> Result<()> {
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let integer_origin = glyph_origin.map(|c| ScaledPixels(round_half_toward_zero(c.0)));
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant: Point::default(),
            scale_factor,
            is_emoji: true,
            dilation: 0,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .sprite_atlas
                .get_or_insert_with(params.clone().into(), &mut || {
                    let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");

            let bounds = Bounds {
                origin: integer_origin + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.snapped_content_mask();
            let opacity = self.element_opacity();

            self.next_frame.scene.insert_primitive(PolychromeSprite {
                order: 0,
                pad: 0,
                grayscale: false.into(),
                bounds,
                corner_radii: Corners::default(),
                content_mask,
                tile,
                opacity,
            });
        }
        Ok(())
    }

    /// Like [`Self::paint_svg`], with an `animation` the window applies before every
    /// present, so the view isn't re-rendered to advance it.
    pub(crate) fn paint_svg_sprite(
        &mut self,
        bounds: Bounds<Pixels>,
        path: SharedString,
        data: Option<&[u8]>,
        transformation: TransformationMatrix,
        animation: Option<SpriteAnimation>,
        color: Hsla,
        cx: &App,
    ) -> Result<()> {
        let element_opacity = self.element_opacity();
        let bounds = self.snap_bounds(bounds);

        let params = RenderSvgParams {
            path,
            size: bounds.size.map(|pixels| DevicePixels::from((pixels.0 * SMOOTH_SVG_SCALE_FACTOR).ceil() as i32)),
        };

        let Some(tile) = self.sprite_atlas.get_or_insert_with(params.clone().into(), &mut || {
            let Some((size, bytes)) = cx.svg_renderer.render_alpha_mask(&params, data)? else {
                return Ok(None);
            };
            Ok(Some((size, Cow::Owned(bytes))))
        })?
        else {
            return Ok(());
        };
        let content_mask = self.snapped_content_mask();
        let svg_bounds = Bounds {
            origin: bounds.center()
                - Point::new(
                    ScaledPixels(tile.bounds.size.width.0 as f32 / SMOOTH_SVG_SCALE_FACTOR / 2.),
                    ScaledPixels(tile.bounds.size.height.0 as f32 / SMOOTH_SVG_SCALE_FACTOR / 2.),
                ),
            size: tile.bounds.size.map(|value| ScaledPixels(value.0 as f32 / SMOOTH_SVG_SCALE_FACTOR)),
        };
        let final_bounds =
            svg_bounds.map_origin(|value| ScaledPixels(round_half_toward_zero(value.0))).map_size(|size| size.ceil());

        let sprite = MonochromeSprite {
            order: 0,
            animation: 0,
            bounds: final_bounds,
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation,
        };
        match animation {
            Some(animation) => self.next_frame.scene.insert_animated_sprite(sprite, animation),
            None => self.next_frame.scene.insert_primitive(sprite),
        }

        Ok(())
    }

    /// Paint an image into the scene for the next frame at the current z-index.
    /// This method will panic if the frame_index is not valid.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    /// Paint an image into `bounds`, positioning and scaling it according to `image_bounds`.
    ///
    /// The visible region rendered is `bounds.intersect(&image_bounds)`, with `corner_radii`
    /// applied to `bounds`.
    pub(crate) fn paint_image(
        &mut self,
        bounds: Bounds<Pixels>,
        image_bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame_index: usize,
        grayscale: bool,
    ) -> Result<()> {
        let visible_bounds = bounds.intersect(&image_bounds);
        if visible_bounds.size.width <= Pixels::ZERO || visible_bounds.size.height <= Pixels::ZERO {
            return Ok(());
        }
        if image_bounds.size.width <= Pixels::ZERO || image_bounds.size.height <= Pixels::ZERO {
            return Ok(());
        }

        let params = RenderImageParams { image_id: data.id, frame_index };

        let tile = self
            .sprite_atlas
            .get_or_insert_with(params.into(), &mut || {
                Ok(Some((
                    data.size(frame_index),
                    Cow::Borrowed(
                        data.as_bytes(frame_index).expect("It's the caller's job to pass a valid frame index"),
                    ),
                )))
            })?
            .expect("Callback above only returns Some");

        let visible_bounds_snapped = self.snap_bounds(visible_bounds);

        let sub_tile = if visible_bounds == image_bounds {
            tile
        } else {
            let x_offset_ratio = (visible_bounds.origin.x - image_bounds.origin.x) / image_bounds.size.width;
            let y_offset_ratio = (visible_bounds.origin.y - image_bounds.origin.y) / image_bounds.size.height;
            let width_ratio = visible_bounds.size.width / image_bounds.size.width;
            let height_ratio = visible_bounds.size.height / image_bounds.size.height;

            let tile_origin_x = tile.bounds.origin.x.0;
            let tile_origin_y = tile.bounds.origin.y.0;
            let tile_width = tile.bounds.size.width.0;
            let tile_height = tile.bounds.size.height.0;

            let sub_origin_x = tile_origin_x + (x_offset_ratio * tile_width as f32).round() as i32;
            let sub_origin_y = tile_origin_y + (y_offset_ratio * tile_height as f32).round() as i32;
            let sub_width = (width_ratio * tile_width as f32).round() as i32;
            let sub_height = (height_ratio * tile_height as f32).round() as i32;

            let max_x = tile_origin_x + tile_width;
            let max_y = tile_origin_y + tile_height;

            let clamped_origin_x = sub_origin_x.clamp(tile_origin_x, max_x);
            let clamped_origin_y = sub_origin_y.clamp(tile_origin_y, max_y);
            let clamped_width = sub_width.min(max_x - clamped_origin_x).max(0);
            let clamped_height = sub_height.min(max_y - clamped_origin_y).max(0);

            AtlasTile {
                bounds: Bounds {
                    origin: point(DevicePixels(clamped_origin_x), DevicePixels(clamped_origin_y)),
                    size: size(DevicePixels(clamped_width), DevicePixels(clamped_height)),
                },
                ..tile
            }
        };

        let content_mask = self.snapped_content_mask();
        let corner_radii = corner_radii.clamp_radii_for_quad_size(visible_bounds.size).scale(self.scale_factor());
        let opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: grayscale.into(),
            bounds: visible_bounds_snapped,
            content_mask,
            corner_radii,
            tile: sub_tile,
            opacity,
        });
        Ok(())
    }

    /// Removes an image from the sprite atlas.
    pub fn drop_image(&mut self, data: Arc<RenderImage>) -> Result<()> {
        for frame_index in 0..data.frame_count() {
            let params = RenderImageParams { image_id: data.id, frame_index };

            self.sprite_atlas.remove(&params.into());
        }

        Ok(())
    }
}

/// A rectangle to be rendered in the window at the given position and size.
/// Passed as an argument [`Window::paint_quad`].
#[derive(Clone, PartialEq, Eq)]
pub struct PaintQuad {
    /// The bounds of the quad within the window.
    pub bounds: Bounds<Pixels>,
    /// The radii of the quad's corners.
    corner_radii: Corners<Pixels>,
    /// The background color of the quad.
    background: Background,
    /// The widths of the quad's borders.
    pub border_widths: Edges<Pixels>,
    /// The color of the quad's borders.
    pub border_color: Hsla,
    /// The style of the quad's borders.
    border_style: BorderStyle,
}

impl PaintQuad {
    /// Sets the corner radii of the quad.
    pub fn corner_radii(self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        PaintQuad { corner_radii: corner_radii.into(), ..self }
    }

    /// Sets the border widths of the quad.
    pub fn border_widths(self, border_widths: impl Into<Edges<Pixels>>) -> Self {
        PaintQuad { border_widths: border_widths.into(), ..self }
    }

    /// Sets the border color of the quad.
    pub fn border_color(self, border_color: impl Into<Hsla>) -> Self {
        PaintQuad { border_color: border_color.into(), ..self }
    }

    /// Sets the background color of the quad.
    pub fn background(self, background: impl Into<Background>) -> Self {
        PaintQuad { background: background.into(), ..self }
    }
}

/// Creates a quad with the given parameters.
pub fn quad(
    bounds: Bounds<Pixels>,
    corner_radii: impl Into<Corners<Pixels>>,
    background: impl Into<Background>,
    border_widths: impl Into<Edges<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds,
        corner_radii: corner_radii.into(),
        background: background.into(),
        border_widths: border_widths.into(),
        border_color: border_color.into(),
        border_style,
    }
}

/// Creates a filled quad with the given bounds and background color.
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background: background.into(),
        border_widths: (0.).into(),
        border_color: transparent_black(),
        border_style: BorderStyle::default(),
    }
}

/// Creates a rectangle outline with the given bounds, border color, and a 1px border width.
pub fn outline(
    bounds: impl Into<Bounds<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background: transparent_black().into(),
        border_widths: (1.).into(),
        border_color: border_color.into(),
        border_style,
    }
}

#[inline]
fn round_half_toward_zero_f64(value: f64) -> f64 {
    (value.abs() - 0.5).ceil().copysign(value)
}

#[inline]
fn floor_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    (logical * scale_factor).floor()
}

fn cover_bounds(bounds: Bounds<Pixels>, scale_factor: f32) -> Bounds<ScaledPixels> {
    let left = floor_to_device_pixel(bounds.left().0, scale_factor);
    let top = floor_to_device_pixel(bounds.top().0, scale_factor);
    // Rounding outward would turn an empty region into a 1px strip at
    // fractional positions, so return a zero size instead. Negative
    // sizes become zero too, because the blurred shadow shader draws a
    // negative-size hole the same as a positive one.
    if bounds.is_empty() {
        return Bounds::new(point(ScaledPixels(left), ScaledPixels(top)), crate::Size::default());
    }
    let right = ceil_to_device_pixel(bounds.right().0, scale_factor).max(left);
    let bottom = ceil_to_device_pixel(bounds.bottom().0, scale_factor).max(top);
    Bounds::from_corners(point(ScaledPixels(left), ScaledPixels(top)), point(ScaledPixels(right), ScaledPixels(bottom)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_bounds_keeps_empty_regions_empty() {
        for scale_factor in [1.0, 1.25, 1.5, 2.0] {
            for fraction in [0., 0.2, 0.5, 0.75] {
                for empty in
                    [size(px(40.), px(0.)), size(px(0.), px(20.)), size(px(0.), px(0.)), size(px(-3.), px(20.))]
                {
                    let covered =
                        cover_bounds(Bounds::new(point(px(30. + fraction), px(181. + fraction)), empty), scale_factor);
                    assert!(covered.is_empty(), "{empty:?} at {fraction} x{scale_factor}: {covered:?}");
                }
                // Positive-area regions still round outward to a superset.
                let bounds = Bounds::new(point(px(30. + fraction), px(181. + fraction)), size(px(10.2), px(0.1)));
                let covered = cover_bounds(bounds, scale_factor);
                assert!(covered.left().0 <= bounds.left().0 * scale_factor);
                assert!(covered.top().0 <= bounds.top().0 * scale_factor);
                assert!(covered.right().0 >= bounds.right().0 * scale_factor);
                assert!(covered.bottom().0 >= bounds.bottom().0 * scale_factor);
                assert!(!covered.is_empty());
            }
        }
    }
}
