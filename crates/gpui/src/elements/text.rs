use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context as _;
use gpui_util::ResultExt as _;
use itertools::Itertools as _;
use smallvec::SmallVec;

use crate::App;
use crate::AvailableSpace;
use crate::Bounds;
use crate::Element;
use crate::ElementId;
use crate::GlobalElementId;
use crate::HighlightStyle;
use crate::IntoElement;
use crate::LayoutId;
use crate::MeasureState;
use crate::Pixels;
use crate::Point;
use crate::SharedString;
use crate::Size;
use crate::TextOverflow;
use crate::TextRun;
use crate::TextStyle;
use crate::TruncateFrom;
use crate::WhiteSpace;
use crate::Window;
use crate::WrappedLine;
use crate::WrappedLineLayout;

impl Element for &'static str {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = TextLayout::default();
        let layout_id = state.layout(SharedString::new_static(self), None, window, cx);
        (layout_id, state)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        text_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        text_layout.prepaint(bounds, self);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: Bounds<Pixels>,
        text_layout: &mut TextLayout,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        text_layout.paint(self, window, cx);
    }
}

impl IntoElement for &'static str {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl IntoElement for String {
    type Element = SharedString;

    fn into_element(self) -> Self::Element {
        self.into()
    }
}

impl IntoElement for Cow<'static, str> {
    type Element = SharedString;

    fn into_element(self) -> Self::Element {
        self.into()
    }
}

impl Element for SharedString {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = TextLayout::default();
        let layout_id = state.layout(self.clone(), None, window, cx);
        (layout_id, state)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        text_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        text_layout.prepaint(bounds, self.as_ref());
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: Bounds<Pixels>,
        text_layout: &mut Self::RequestLayoutState,
        (): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        text_layout.paint(self.as_ref(), window, cx);
    }
}

impl IntoElement for SharedString {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Renders text with runs of different styles.
///
/// Callers are responsible for setting the correct style for each run.
/// For text with a uniform style, you can usually avoid calling this constructor
/// and just pass text directly.
pub struct StyledText {
    text: SharedString,
    runs: Option<Vec<TextRun>>,
    delayed_highlights: Option<Vec<(Range<usize>, HighlightStyle)>>,
    delayed_font_family_overrides: Option<Vec<(Range<usize>, SharedString)>>,
    layout: TextLayout,
}

impl StyledText {
    /// Construct a new styled text element from the given string.
    pub fn new(text: impl Into<SharedString>) -> Self {
        StyledText {
            text: text.into(),
            runs: None,
            delayed_highlights: None,
            delayed_font_family_overrides: None,
            layout: TextLayout::default(),
        }
    }

    /// Get the layout for this element. This can be used to map indices to pixels and vice versa.
    pub const fn layout(&self) -> &TextLayout {
        &self.layout
    }

    /// Set the styling attributes for the given text, as well as
    /// as any ranges of text that have had their style customized.
    pub fn with_default_highlights(
        self,
        default_style: &TextStyle,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self {
        let runs = Self::compute_runs(&self.text, default_style, highlights);
        self.with_runs(runs)
    }

    /// Set the styling attributes for the given text, as well as
    /// as any ranges of text that have had their style customized.
    pub fn with_highlights(mut self, highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>) -> Self {
        self.delayed_highlights = Some(highlights.into_iter().collect::<Vec<_>>());
        self
    }

    fn compute_runs(
        text: &str,
        default_style: &TextStyle,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Vec<TextRun> {
        let mut runs = Vec::new();
        let mut ix = 0;
        for (range, highlight) in highlights {
            if ix < range.start {
                runs.push(default_style.clone().to_run(range.start - ix));
            }
            runs.push(default_style.clone().highlight(highlight).to_run(range.len()));
            ix = range.end;
        }
        if ix < text.len() {
            runs.push(default_style.to_run(text.len() - ix));
        }
        runs
    }

    /// Override the font family for specific byte ranges of the text.
    ///
    /// This is resolved lazily at layout time, so the overrides are applied
    /// on top of the inherited text style from the parent element.
    /// Can be combined with [`with_highlights`](Self::with_highlights).
    ///
    /// The overrides must be sorted by range start and non-overlapping.
    /// Each override range must fall on character boundaries.
    pub fn with_font_family_overrides(
        mut self,
        overrides: impl IntoIterator<Item = (Range<usize>, SharedString)>,
    ) -> Self {
        self.delayed_font_family_overrides = Some(overrides.into_iter().collect());
        self
    }

    fn apply_font_family_overrides(runs: &mut [TextRun], overrides: &[(Range<usize>, SharedString)]) {
        let mut byte_offset = 0;
        let mut override_idx = 0;
        for run in runs.iter_mut() {
            let run_end = byte_offset + run.len;
            while override_idx < overrides.len() && overrides[override_idx].0.end <= byte_offset {
                override_idx += 1;
            }
            if override_idx < overrides.len() {
                let (ref range, ref family) = overrides[override_idx];
                if byte_offset >= range.start && run_end <= range.end {
                    run.font.family = family.clone();
                }
            }
            byte_offset = run_end;
        }
    }

    /// Set the text runs for this piece of text.
    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self {
        let mut text = &*self.text;
        for run in &runs {
            text = text.get(run.len..).unwrap_or_else(|| panic!("invalid text run. Text: '{text}', run: {run:?}"));
        }
        assert!(text.is_empty(), "invalid text run");
        self.runs = Some(runs);
        self
    }
}

impl Element for StyledText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let font_family_overrides = self.delayed_font_family_overrides.take();
        let mut runs = self.runs.take().or_else(|| {
            self.delayed_highlights
                .take()
                .map(|delayed_highlights| Self::compute_runs(&self.text, &window.text_style(), delayed_highlights))
        });

        if let Some(ref overrides) = font_family_overrides {
            let runs = runs.get_or_insert_with(|| vec![window.text_style().to_run(self.text.len())]);
            Self::apply_font_family_overrides(runs, overrides);
        }

        let layout_id = self.layout.layout(self.text.clone(), runs, window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        (): &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.layout.prepaint(bounds, &self.text);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: Bounds<Pixels>,
        (): &mut Self::RequestLayoutState,
        (): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.layout.paint(&self.text, window, cx);
    }
}

impl IntoElement for StyledText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The Layout for TextElement. This can be used to map indices to pixels and vice versa.
#[derive(Default, Clone)]
pub struct TextLayout(Rc<RefCell<Option<TextLayoutInner>>>);

#[derive(Clone, Copy)]
enum TextPaintPass {
    All,
    Background,
    Foreground,
}

struct TextLayoutInner {
    len: usize,
    lines: SmallVec<[WrappedLine; 1]>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    truncate_width: Option<Pixels>,
    size: Option<Size<Pixels>>,
    bounds: Option<Bounds<Pixels>>,
}

/// What a text measure function measures, kept with it between frames. An
/// element whose text, runs and style are the ones measured last frame takes
/// over that frame's layout instead of being measured again.
struct TextMeasure {
    text: SharedString,
    /// `None` for a single run in the text style.
    runs: Option<Vec<TextRun>>,
    text_style: TextStyle,
    font_size: Pixels,
    line_height: Pixels,
    /// The layout of the element being drawn, which the measure function writes to.
    layout: RefCell<TextLayout>,
}

impl MeasureState for TextMeasure {
    const RESETS: bool = true;

    fn reset(&self) {
        self.layout.borrow().0.borrow_mut().take();
    }
}

impl TextLayout {
    fn layout(&self, text: SharedString, runs: Option<Vec<TextRun>>, window: &mut Window, _: &mut App) -> LayoutId {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(text_style.line_height.to_pixels(font_size.into(), window.rem_size()));

        let (layout_id, measure) = window.request_retained_measured_layout(
            crate::Style::default(),
            |measure: &TextMeasure| {
                measure.font_size == font_size
                    && measure.line_height == line_height
                    && measure.text == text
                    && measure.runs == runs
                    && measure.text_style == text_style
            },
            || {
                let measure = Rc::new(TextMeasure {
                    text: text.clone(),
                    runs: runs.clone(),
                    text_style: text_style.clone(),
                    font_size,
                    line_height,
                    layout: RefCell::new(self.clone()),
                });
                (Rc::clone(&measure), Self::measure_fn(measure))
            },
        );
        self.take_over(&measure);
        layout_id
    }

    /// Makes this the layout `measure` writes to. A measure function kept from
    /// last frame left its lines with that frame's element: they move here.
    fn take_over(&self, measure: &TextMeasure) {
        let mut layout = measure.layout.borrow_mut();
        if !Rc::ptr_eq(&layout.0, &self.0) {
            let lines = layout.0.borrow_mut().take();
            *self.0.borrow_mut() = lines;
            *layout = self.clone();
        }
    }

    fn measure_fn(
        measure: Rc<TextMeasure>,
    ) -> impl Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels> {
        move |known_dimensions, available_space, window, cx| {
            let TextMeasure { text, text_style, font_size, line_height, .. } = &*measure;
            let (font_size, line_height) = (*font_size, *line_height);
            let element_state = measure.layout.borrow().clone();
            let runs = match &measure.runs {
                Some(runs) => Cow::Borrowed(&**runs),
                None => Cow::Owned(vec![text_style.to_run(text.len())]),
            };
            let wrap_width = if text_style.white_space == WhiteSpace::Normal {
                known_dimensions.width.or(match available_space.width {
                    crate::AvailableSpace::Definite(x) => Some(x),
                    _ => None,
                })
            } else {
                None
            };

            let (truncate_width, truncation_affix, truncate_from) =
                if let Some(text_overflow) = text_style.text_overflow.clone() {
                    let width = known_dimensions.width.or(match available_space.width {
                        crate::AvailableSpace::Definite(x) => match text_style.line_clamp {
                            Some(max_lines) => Some(x * max_lines),
                            None => Some(x),
                        },
                        _ => None,
                    });

                    match text_overflow {
                        TextOverflow::Truncate(s) => (width, s, TruncateFrom::End),
                        TextOverflow::TruncateStart(s) => (width, s, TruncateFrom::Start),
                        TextOverflow::TruncateMiddle(s) => (width, s, TruncateFrom::Middle),
                    }
                } else {
                    (None, "".into(), TruncateFrom::End)
                };

            // Only use cached layout if:
            // 1. We have a cached size
            // 2. wrap_width matches (or both are None)
            // 3. truncate_width is None (if truncate_width is Some, we need to re-layout
            //    because the previous layout may have been computed without truncation)
            // 4. the cached layout was not truncated (a truncated layout answers an
            //    unconstrained probe with the truncated size, which poisons intrinsic
            //    sizing with whatever width some earlier measure pass happened to use)
            if let Some(text_layout) = element_state.0.borrow().as_ref()
                && let Some(size) = text_layout.size
                && (wrap_width.is_none() || wrap_width == text_layout.wrap_width)
                && truncate_width.is_none()
                && text_layout.truncate_width.is_none()
            {
                return size;
            }

            let (text, runs) = if let Some(truncate_width) = truncate_width {
                // Taking a wrapper from the pool locks it and moves its character width
                // cache around, so only do it when truncating.
                let mut line_wrapper = cx.text_system().line_wrapper(text_style.font(), font_size);
                if let Some(max_lines) = text_style.line_clamp
                    && let Some(wrap_width) = wrap_width
                {
                    line_wrapper.truncate_wrapped_line(
                        text.clone(),
                        wrap_width,
                        max_lines,
                        &truncation_affix,
                        &runs,
                        truncate_from,
                    )
                } else if let Some(unclipped) =
                    window.text_system().shape_text(text.clone(), font_size, &runs, None, None).log_err()
                    && unclipped.iter().all(|line| line.size(line_height).width <= truncate_width)
                {
                    // The truncation decision below sums per-character advances,
                    // which overestimates the shaped width (no kerning), truncating
                    // text that fits exactly in its measured width. Skip truncation
                    // whenever the honestly-shaped text fits; the shaping result
                    // comes from the line layout cache when the same text was
                    // already measured untruncated this frame.
                    (text.clone(), Cow::Borrowed(&*runs))
                } else {
                    line_wrapper.truncate_line(text.clone(), truncate_width, &truncation_affix, &runs, truncate_from)
                }
            } else {
                (text.clone(), Cow::Borrowed(&*runs))
            };
            let len = text.len();

            let Some(lines) = window
                .text_system()
                .shape_text(
                    text,
                    font_size,
                    &runs,
                    wrap_width,            // Wrap if we know the width.
                    text_style.line_clamp, // Limit the number of lines if line_clamp is set.
                )
                .log_err()
            else {
                element_state.0.borrow_mut().replace(TextLayoutInner {
                    lines: SmallVec::default(),
                    len: 0,
                    line_height,
                    wrap_width,
                    truncate_width,
                    size: Some(Size::default()),
                    bounds: None,
                });
                return Size::default();
            };

            let mut size: Size<Pixels> = Size::default();
            for line in &lines {
                let line_size = line.size(line_height);
                size.height += line_size.height;
                size.width = size.width.max(line_size.width).ceil();
            }

            element_state.0.borrow_mut().replace(TextLayoutInner {
                lines,
                len,
                line_height,
                wrap_width,
                truncate_width,
                size: Some(size),
                bounds: None,
            });

            size
        }
    }

    fn prepaint(&self, bounds: Bounds<Pixels>, text: &str) {
        let mut element_state = self.0.borrow_mut();
        let element_state =
            element_state.as_mut().unwrap_or_else(|| panic!("measurement has not been performed on {text}"));
        element_state.bounds = Some(bounds);
    }

    fn paint(&self, text: &str, window: &mut Window, cx: &mut App) {
        self.paint_lines(TextPaintPass::All, window, cx)
            .unwrap_or_else(|err| panic!("failed to paint {text}: {err:#}"));
    }

    /// Paints the backgrounds of the text runs without the glyphs, so that callers can paint
    /// content, such as highlights, between the backgrounds and the text. Follow it with
    /// [`Self::paint_foreground`] to paint the glyphs.
    pub fn paint_background(&self, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
        self.paint_lines(TextPaintPass::Background, window, cx)
    }

    /// Paints the glyphs and decorations of the text without the run backgrounds.
    pub fn paint_foreground(&self, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
        self.paint_lines(TextPaintPass::Foreground, window, cx)
    }

    fn paint_lines(&self, pass: TextPaintPass, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
        let element_state = self.0.borrow();
        let element_state = element_state.as_ref().context("measurement has not been performed")?;
        let bounds = element_state.bounds.context("prepaint has not been performed")?;

        let line_height = element_state.line_height;
        let mut line_origin = bounds.origin;
        let text_style = window.text_style();
        for line in &element_state.lines {
            if matches!(pass, TextPaintPass::All | TextPaintPass::Background) {
                line.paint_background(line_origin, line_height, text_style.text_align, Some(bounds), window, cx)
                    .log_err();
            }
            if matches!(pass, TextPaintPass::All | TextPaintPass::Foreground) {
                line.paint(line_origin, line_height, text_style.text_align, Some(bounds), window, cx).log_err();
            }
            line_origin.y += line.size(line_height).height;
        }
        Ok(())
    }

    /// Get the byte index into the input of the pixel position.
    pub fn index_for_position(&self, position: Point<Pixels>) -> Result<usize, usize> {
        let element_state = self.0.borrow();
        let element_state = element_state.as_ref().expect("measurement has not been performed");
        let bounds = element_state.bounds.expect("prepaint has not been performed");

        if position.y < bounds.top() {
            return Err(0);
        }

        let line_height = element_state.line_height;
        let mut line_origin = bounds.origin;
        let mut line_start_ix = 0;
        for line in &element_state.lines {
            let line_bottom = line_origin.y + line.size(line_height).height;
            if position.y > line_bottom {
                line_origin.y = line_bottom;
                line_start_ix += line.len() + 1;
            } else {
                let position_within_line = position - line_origin;
                match line.index_for_position(position_within_line, line_height) {
                    Ok(index_within_line) => return Ok(line_start_ix + index_within_line),
                    Err(index_within_line) => return Err(line_start_ix + index_within_line),
                }
            }
        }

        Err(line_start_ix.saturating_sub(1))
    }

    /// Get the pixel position for the given byte index.
    pub fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        let element_state = self.0.borrow();
        let element_state = element_state.as_ref().expect("measurement has not been performed");
        let bounds = element_state.bounds.expect("prepaint has not been performed");
        let line_height = element_state.line_height;

        let mut line_origin = bounds.origin;
        let mut line_start_ix = 0;

        for line in &element_state.lines {
            let line_end_ix = line_start_ix + line.len();
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_origin.y += line.size(line_height).height;
                line_start_ix = line_end_ix + 1;
            } else {
                let ix_within_line = index - line_start_ix;
                return Some(line_origin + line.position_for_index(ix_within_line, line_height)?);
            }
        }

        None
    }

    /// Retrieve the layout for the line containing the given byte index.
    pub fn line_layout_for_index(&self, index: usize) -> Option<Arc<WrappedLineLayout>> {
        let element_state = self.0.borrow();
        let element_state = element_state.as_ref().expect("measurement has not been performed");
        let mut line_start_ix = 0;

        for line in &element_state.lines {
            let line_end_ix = line_start_ix + line.len();
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_start_ix = line_end_ix + 1;
                continue;
            }
            return Some(Arc::clone(&line.layout));
        }

        None
    }

    /// Retrieve all line layouts in source order.
    pub fn line_layouts(&self) -> SmallVec<[Arc<WrappedLineLayout>; 1]> {
        self.0
            .borrow()
            .as_ref()
            .expect("measurement has not been performed")
            .lines
            .iter()
            .map(|line| Arc::clone(&line.layout))
            .collect()
    }

    /// The bounds of this layout.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().as_ref().expect("text layout is measured").bounds.expect("text layout is prepainted")
    }

    /// The line height for this layout.
    pub fn line_height(&self) -> Pixels {
        self.0.borrow().as_ref().expect("text layout is measured").line_height
    }

    /// The UTF-8 length of the underlying text.
    pub fn len(&self) -> usize {
        self.0.borrow().as_ref().expect("text layout is measured").len
    }

    /// Whether the underlying text is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The text for this layout.
    pub fn text(&self) -> String {
        self.0.borrow().as_ref().expect("text layout is measured").lines.iter().map(|s| &s.text).join("\n")
    }

    /// The text for this layout (with soft-wraps as newlines).
    pub fn wrapped_text(&self) -> String {
        let mut accumulator = String::new();

        for wrapped in &self.0.borrow().as_ref().expect("text layout is measured").lines {
            let mut seen = 0;
            for boundary in &wrapped.layout.wrap_boundaries {
                let index = wrapped.layout.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;

                accumulator.push_str(&wrapped.text[seen..index]);
                accumulator.push('\n');
                seen = index;
            }
            accumulator.push_str(&wrapped.text[seen..]);
            accumulator.push('\n');
        }
        // Remove trailing newline
        accumulator.pop();
        accumulator
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use smallvec::SmallVec;

    use super::TextLayout;
    use super::TextLayoutInner;
    use super::TextMeasure;
    use crate::MeasureState as _;
    use crate::TextStyle;
    use crate::px;
    use crate::size;

    #[test]
    fn kept_measure_hands_its_lines_to_the_next_element() {
        let last_frame = TextLayout::default();
        last_frame.0.borrow_mut().replace(TextLayoutInner {
            len: 5,
            lines: SmallVec::new(),
            line_height: px(20.),
            wrap_width: Some(px(80.)),
            truncate_width: None,
            size: Some(size(px(40.), px(20.))),
            bounds: None,
        });
        let measure = TextMeasure {
            text: "hello".into(),
            runs: None,
            text_style: TextStyle::default(),
            font_size: px(14.),
            line_height: px(20.),
            layout: RefCell::new(last_frame.clone()),
        };

        // A handle cloned before layout sees what the element gets.
        let this_frame = TextLayout::default();
        let handle = this_frame.clone();
        this_frame.take_over(&measure);
        assert!(last_frame.0.borrow().is_none());
        assert_eq!(handle.0.borrow().as_ref().and_then(|layout| layout.size), Some(size(px(40.), px(20.))));
        assert!(Rc::ptr_eq(&measure.layout.borrow().0, &this_frame.0));

        // Taking over again changes nothing, and a reset empties the current element's layout.
        this_frame.take_over(&measure);
        assert!(handle.0.borrow().is_some());
        measure.reset();
        assert!(handle.0.borrow().is_none());
    }
}
