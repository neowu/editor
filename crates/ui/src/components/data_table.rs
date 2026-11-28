use std::ops::Range;
use std::rc::Rc;

use gpui::DefiniteLength;
use gpui::Entity;
use gpui::EntityId;
use gpui::FocusHandle;
use gpui::Length;
use gpui::ListHorizontalSizingBehavior;
use gpui::ListSizingBehavior;
use gpui::Point;
use gpui::Stateful;
use gpui::UniformListScrollHandle;
use gpui::WeakEntity;
use gpui::transparent_black;
use gpui::uniform_list;

use crate::AnyElement;
use crate::App;
use crate::Context;
use crate::Div;
use crate::ElementId;
use crate::FluentBuilder as _;
use crate::HeaderResizeInfo;
use crate::InteractiveElement as _;
use crate::IntoElement;
use crate::ParentElement as _;
use crate::Pixels;
use crate::RedistributableColumnsState;
use crate::RenderOnce;
use crate::ScrollAxes;
use crate::ScrollableHandle as _;
use crate::Scrollbars;
use crate::SharedString;
use crate::StatefulInteractiveElement as _;
use crate::Styled as _;
use crate::StyledExt as _;
use crate::StyledTypography as _;
use crate::Window;
use crate::WithScrollbar as _;
use crate::bind_redistributable_columns;
use crate::div;
use crate::h_flex;
use crate::redistribute_hidden_widths;
use crate::render_redistributable_columns_resize_handles;
use crate::table_row::IntoTableRow as _;
use crate::table_row::TableRow;

/// Represents an unchecked table row, which is a vector of elements.
/// Will be converted into `TableRow<T>` internally.
pub(super) type UncheckedTableRow<T> = Vec<T>;

struct UniformListData {
    #[allow(clippy::type_complexity)]
    render_list_of_rows_fn: Box<dyn Fn(Range<usize>, &mut Window, &mut App) -> Vec<UncheckedTableRow<AnyElement>>>,
    element_id: ElementId,
    row_count: usize,
}

enum TableContents {
    Vec(Vec<TableRow<AnyElement>>),
    UniformList(UniformListData),
}

impl TableContents {
    const fn rows_mut(&mut self) -> Option<&mut Vec<TableRow<AnyElement>>> {
        match self {
            TableContents::Vec(rows) => Some(rows),
            TableContents::UniformList(_) => None,
        }
    }

    const fn len(&self) -> usize {
        match self {
            TableContents::Vec(rows) => rows.len(),
            TableContents::UniformList(data) => data.row_count,
        }
    }

    const fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct TableInteractionState {
    pub focus_handle: FocusHandle,
    pub scroll_handle: UniformListScrollHandle,
}

impl TableInteractionState {
    pub fn new(cx: &mut App) -> Self {
        Self { focus_handle: cx.focus_handle(), scroll_handle: UniformListScrollHandle::new() }
    }

    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_handle.offset()
    }

    pub fn set_scroll_offset(&self, offset: Point<Pixels>) {
        self.scroll_handle.set_offset(offset);
    }

    pub fn listener<E: ?Sized>(
        this: &Entity<Self>,
        f: impl Fn(&mut Self, &E, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = this.downgrade();
        move |e: &E, window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| f(view, e, window, cx)).ok();
        }
    }
}

pub enum ColumnWidthConfig {
    /// Static column widths (no resize handles).
    Static {
        widths: StaticColumnWidths,
        /// Controls widths of the whole table.
        table_width: Option<DefiniteLength>,
    },
    /// Redistributable columns — dragging redistributes the fixed available space
    /// among columns without changing the overall table width.
    Redistributable { columns_state: Entity<RedistributableColumnsState>, table_width: Option<DefiniteLength> },
}

pub enum StaticColumnWidths {
    /// All columns share space equally (flex-1 / Length::Auto).
    Auto,
    /// Each column has a specific width.
    Explicit(TableRow<DefiniteLength>),
}

impl ColumnWidthConfig {
    /// Auto-width columns, auto-size table.
    const fn auto() -> Self {
        ColumnWidthConfig::Static { widths: StaticColumnWidths::Auto, table_width: None }
    }

    /// Auto-width columns, fixed table width.
    fn auto_with_table_width(width: impl Into<DefiniteLength>) -> Self {
        ColumnWidthConfig::Static { widths: StaticColumnWidths::Auto, table_width: Some(width.into()) }
    }

    /// Explicit column widths with no fixed table width.
    pub fn explicit<T: Into<DefiniteLength>>(widths: Vec<T>) -> Self {
        let cols = widths.len();
        ColumnWidthConfig::Static {
            widths: StaticColumnWidths::Explicit(
                widths.into_iter().map(Into::into).collect::<Vec<_>>().into_table_row(cols),
            ),
            table_width: None,
        }
    }

    /// Column widths for rendering.
    fn widths_to_render(&self, cx: &App) -> Option<TableRow<Length>> {
        match self {
            ColumnWidthConfig::Static { widths: StaticColumnWidths::Auto, .. } => None,
            ColumnWidthConfig::Static { widths: StaticColumnWidths::Explicit(widths), .. } => {
                Some(widths.map_cloned(Length::Definite))
            }
            ColumnWidthConfig::Redistributable { columns_state: entity, .. } => {
                Some(entity.read(cx).widths_to_render())
            }
        }
    }

    /// Table-level width.
    fn table_width(&self) -> Option<Length> {
        match self {
            ColumnWidthConfig::Static { table_width, .. } | ColumnWidthConfig::Redistributable { table_width, .. } => {
                table_width.map(Length::Definite)
            }
        }
    }

    /// ListHorizontalSizingBehavior for uniform_list.
    fn list_horizontal_sizing(&self) -> ListHorizontalSizingBehavior {
        match self.table_width() {
            Some(_) => ListHorizontalSizingBehavior::Unconstrained,
            None => ListHorizontalSizingBehavior::FitList,
        }
    }
}

/// A table component.
#[derive(IntoElement)]
pub struct Table {
    striped: bool,
    show_row_borders: bool,
    show_row_hover: bool,
    headers: Option<TableRow<AnyElement>>,
    rows: TableContents,
    interaction_state: Option<WeakEntity<TableInteractionState>>,
    column_width_config: ColumnWidthConfig,
    #[allow(clippy::type_complexity)]
    map_row: Option<Rc<dyn Fn((usize, Stateful<Div>), &mut Window, &mut App) -> AnyElement>>,
    use_ui_font: bool,
    #[allow(clippy::type_complexity)]
    empty_table_callback: Option<Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>>,
    /// The number of columns in the table. Used to assert column numbers in `TableRow` collections.
    cols: usize,
    /// Optional per-column visibility mask. When set, it overrides any filter derived from the
    /// column width config. Columns whose entry is `true` are hidden.
    column_filter: Option<TableRow<bool>>,
}

impl Table {
    /// Creates a new table with the specified number of columns.
    pub fn new(cols: usize) -> Self {
        Self {
            cols,
            striped: false,
            show_row_borders: true,
            show_row_hover: true,
            headers: None,
            rows: TableContents::Vec(Vec::new()),
            interaction_state: None,
            map_row: None,
            use_ui_font: true,
            empty_table_callback: None,
            column_width_config: ColumnWidthConfig::auto(),
            column_filter: None,
        }
    }

    /// Sets a per-column visibility mask. Columns whose entry is `true` are filtered out (hidden).
    ///
    /// This is useful when column visibility is driven by state that isn't part of the column
    /// width config (for example a `Static`/`Explicit` width config). When set, this overrides
    /// any filter derived from the column width config.
    pub fn column_filter(mut self, filter: TableRow<bool>) -> Self {
        self.column_filter = Some(filter);
        self
    }

    /// Enables uniform list rendering.
    /// The provided function will be passed directly to the `uniform_list` element.
    /// Therefore, if this method is called, any calls to [`Table::row`] before or after
    /// this method is called will be ignored.
    pub fn uniform_list(
        mut self,
        id: impl Into<ElementId>,
        row_count: usize,
        render_item_fn: impl Fn(Range<usize>, &mut Window, &mut App) -> Vec<UncheckedTableRow<AnyElement>> + 'static,
    ) -> Self {
        self.rows = TableContents::UniformList(UniformListData {
            element_id: id.into(),
            row_count,
            render_list_of_rows_fn: Box::new(render_item_fn),
        });
        self
    }

    /// Hides the border lines between rows.
    pub const fn hide_row_borders(mut self) -> Self {
        self.show_row_borders = false;
        self
    }

    /// Sets a fixed table width with auto column widths.
    ///
    /// This is a shorthand for `.width_config(ColumnWidthConfig::auto_with_table_width(width))`.
    /// For resizable columns or explicit column widths, use [`Table::width_config`] directly.
    pub fn width(mut self, width: impl Into<DefiniteLength>) -> Self {
        self.column_width_config = ColumnWidthConfig::auto_with_table_width(width);
        self
    }

    /// Sets the column width configuration for the table.
    pub fn width_config(mut self, config: ColumnWidthConfig) -> Self {
        self.column_width_config = config;
        self
    }

    /// Enables interaction (primarily scrolling) with the table.
    ///
    /// Vertical scrolling will be enabled by default if the table is taller than its container.
    ///
    /// Horizontal scrolling will only be enabled if a table width is set via [`ColumnWidthConfig`],
    /// otherwise the list will always shrink the table columns to fit their contents.
    pub fn interactable(mut self, interaction_state: &Entity<TableInteractionState>) -> Self {
        self.interaction_state = Some(interaction_state.downgrade());
        self
    }

    pub fn header(mut self, headers: UncheckedTableRow<impl IntoElement>) -> Self {
        self.headers = Some(headers.into_table_row(self.cols).map(IntoElement::into_any_element));
        self
    }

    pub fn row(mut self, items: UncheckedTableRow<impl IntoElement>) -> Self {
        if let Some(rows) = self.rows.rows_mut() {
            rows.push(items.into_table_row(self.cols).map(IntoElement::into_any_element));
        }
        self
    }

    pub fn map_row(
        mut self,
        callback: impl Fn((usize, Stateful<Div>), &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.map_row = Some(Rc::new(callback));
        self
    }

    /// Hides the default hover background on table rows.
    /// Use this when you want to handle row hover styling manually via `map_row`.
    pub const fn hide_row_hover(mut self) -> Self {
        self.show_row_hover = false;
        self
    }
}

fn base_cell_style(width: Option<Length>) -> Div {
    div()
        .px_1p5()
        .when_some(width, |this, width| this.w(width))
        .when(width.is_none(), |this| this.flex_1())
        .whitespace_nowrap()
        .text_ellipsis()
        .overflow_hidden()
}

fn base_cell_style_text(width: Option<Length>, use_ui_font: bool, cx: &App) -> Div {
    base_cell_style(width).when(use_ui_font, |el| el.text_ui(cx))
}

fn render_cell(width: Option<Length>, cell: AnyElement, ctx: &TableRenderContext, cx: &App) -> Div {
    base_cell_style_text(width, ctx.use_ui_font, cx).px_1().py_0p5().child(cell)
}

fn render_header_cell(
    header: AnyElement,
    width: Option<Length>,
    header_idx: usize,
    shared_element_id: &SharedString,
    resize_info: Option<&HeaderResizeInfo>,
    use_ui_font: bool,
    cx: &App,
) -> Stateful<Div> {
    base_cell_style_text(width, use_ui_font, cx)
        .px_1()
        .py_0p5()
        .child(header)
        .id(ElementId::NamedInteger(shared_element_id.clone(), header_idx as u64))
        .when_some(resize_info.cloned(), |this, info| {
            if info.resize_behavior[header_idx].is_resizable() {
                this.on_click(move |event, window, cx| {
                    if event.click_count() > 1 {
                        info.reset_column(header_idx, window, cx);
                    }
                })
            } else {
                this
            }
        })
}

fn render_table_row(
    row_index: usize,
    items: TableRow<impl IntoElement>,
    table_context: TableRenderContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let is_striped = table_context.striped;
    let is_last = row_index == table_context.total_row_count - 1;
    let bg = (row_index % 2 == 1 && is_striped).then(|| theme::theme().colors().text.opacity(0.05));
    let cols = items.cols();
    let column_widths = match &table_context.column_widths {
        Some(widths) => widths.clone().map(Some),
        None => vec![None; cols].into_table_row(cols),
    };
    let row = div()
        // NOTE: `h_flex()` sneakily applies `items_center()` which is not default behavior for div element.
        // Applying `.flex().flex_row()` manually to overcome that
        .flex()
        .flex_row()
        .id(("table_row", row_index))
        .size_full()
        .when_some(bg, |row, bg| row.bg(bg))
        .when(table_context.show_row_hover, |row| {
            row.hover(|s| s.bg(theme::theme().colors().element_hover.opacity(0.6)))
        });

    let column_filter = table_context.column_filter.as_ref();

    // Only absolute pixel widths should leave the wrapper
    // hugging their combined width so the row border stops at the last column. Percentage
    // widths (`Redistributable`) need a full-size ancestor for their fractions to resolve
    // against, and `flex_1`/auto cells are meant to stretch across the whole row anyway.
    let is_absolute_width_table = table_context.column_widths.as_ref().is_some_and(|widths| {
        widths.as_slice().iter().all(|width| matches!(width, Length::Definite(DefiniteLength::Absolute(_))))
    });

    let cells = items
        .into_vec()
        .into_iter()
        .map(IntoElement::into_any_element)
        .zip(column_widths.into_vec())
        .enumerate()
        .filter(|(idx, _)| column_is_visible(column_filter, *idx))
        .map(|(_, (cell, width))| render_cell(width, cell, &table_context, cx));

    let row = row.child(
        div()
            .flex()
            .flex_row()
            .when(!is_striped && table_context.show_row_borders, |row| {
                row.border_b_1().map(|row| {
                    if is_last {
                        row.border_color(transparent_black())
                    } else {
                        row.border_color(theme::theme().colors().border)
                    }
                })
            })
            .children(cells)
            .when(!is_absolute_width_table, |this| this.size_full()),
    );

    let row = if let Some(map_row) = table_context.map_row {
        map_row((row_index, row), window, cx)
    } else {
        row.into_any_element()
    };

    div().size_full().child(row).into_any_element()
}

pub fn render_table_header(
    headers: TableRow<impl IntoElement>,
    table_context: TableRenderContext,
    resize_info: Option<HeaderResizeInfo>,
    entity_id: Option<EntityId>,
    cx: &mut App,
) -> AnyElement {
    let cols = headers.cols();
    let column_widths =
        table_context.column_widths.map_or_else(|| vec![None; cols].into_table_row(cols), |widths| widths.map(Some));

    let element_id = entity_id.map_or_default(|entity| entity.to_string());

    let shared_element_id: SharedString = format!("table-{element_id}").into();
    let outer = h_flex().py_1().w_full().border_b_1().border_color(theme::theme().colors().border_variant);

    let use_ui_font = table_context.use_ui_font;
    let resize_info_ref = resize_info.as_ref();
    let column_filter = table_context.column_filter.as_ref();

    outer
        .children(
            headers
                .into_vec()
                .into_iter()
                .enumerate()
                .zip(column_widths.into_vec())
                .filter(|((idx, _), _)| column_is_visible(column_filter, *idx))
                .map(|((header_idx, h), width)| {
                    render_header_cell(
                        h.into_any_element(),
                        width,
                        header_idx,
                        &shared_element_id,
                        resize_info_ref,
                        use_ui_font,
                        cx,
                    )
                }),
        )
        .into_any_element()
}

#[derive(Clone)]
pub struct TableRenderContext {
    striped: bool,
    show_row_borders: bool,
    show_row_hover: bool,
    total_row_count: usize,
    column_widths: Option<TableRow<Length>>,
    #[allow(clippy::type_complexity)]
    map_row: Option<Rc<dyn Fn((usize, Stateful<Div>), &mut Window, &mut App) -> AnyElement>>,
    use_ui_font: bool,
    /// Per-column visibility mask. When `Some`, columns whose entry is `true` are filtered
    /// out (hidden) and not rendered. Indices map to the table's original column positions.
    column_filter: Option<TableRow<bool>>,
}

impl TableRenderContext {
    fn new(table: &Table, cx: &App) -> Self {
        Self {
            striped: table.striped,
            show_row_borders: table.show_row_borders,
            show_row_hover: table.show_row_hover,
            total_row_count: table.rows.len(),
            column_widths: table
                .column_width_config
                .widths_to_render(cx)
                .map(|widths| redistribute_hidden_widths(&widths, table.column_filter.as_ref())),
            map_row: table.map_row.clone(),
            use_ui_font: table.use_ui_font,
            column_filter: table.column_filter.clone(),
        }
    }

    pub fn for_column_widths(column_widths: Option<TableRow<Length>>, use_ui_font: bool) -> Self {
        Self {
            striped: false,
            show_row_borders: true,
            show_row_hover: true,
            total_row_count: 0,
            column_widths,
            map_row: None,
            use_ui_font,
            column_filter: None,
        }
    }

    /// Sets the per-column visibility mask. Columns whose entry is `true` are hidden.
    pub fn with_column_filter(mut self, column_filter: Option<TableRow<bool>>) -> Self {
        self.column_filter = column_filter;
        self
    }
}

/// Returns `true` when the column at `col_idx` should be rendered (i.e. not filtered out).
fn column_is_visible(filter: Option<&TableRow<bool>>, col_idx: usize) -> bool {
    filter.is_none_or(|mask| !mask.get(col_idx).copied().unwrap_or(false))
}

impl RenderOnce for Table {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let interaction_state = self.interaction_state.clone().and_then(|state| state.upgrade());
        let table_context = TableRenderContext::new(&self, cx);

        let header_resize_info = interaction_state.as_ref().and_then(|_| match &self.column_width_config {
            ColumnWidthConfig::Redistributable { columns_state, .. } => {
                Some(HeaderResizeInfo::from_redistributable(columns_state, cx))
            }
            ColumnWidthConfig::Static { .. } => None,
        });

        let table_width = self.column_width_config.table_width();
        let horizontal_sizing = self.column_width_config.list_horizontal_sizing();

        let no_rows_rendered = self.rows.is_empty();
        let (redistributable_entity, body_resize_handles) = match &self.column_width_config {
            ColumnWidthConfig::Redistributable { columns_state, .. } if interaction_state.is_some() => (
                Some(columns_state.clone()),
                Some(render_redistributable_columns_resize_handles(
                    columns_state,
                    self.column_filter.as_ref(),
                    window,
                    cx,
                )),
            ),
            ColumnWidthConfig::Static { .. } | ColumnWidthConfig::Redistributable { .. } => (None, None),
        };

        let table = div()
            .when_some(table_width, |this, width| this.w(width))
            .h_full()
            .v_flex()
            .when_some(self.headers.take(), |this, headers| {
                this.child(render_table_header(
                    headers,
                    table_context.clone(),
                    header_resize_info,
                    interaction_state.as_ref().map(Entity::entity_id),
                    cx,
                ))
            })
            .when_some(redistributable_entity, |this, widths| {
                bind_redistributable_columns(this, widths, self.column_filter.clone())
            })
            .child({
                let content = div()
                    .flex_grow_1()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .map(|parent| match self.rows {
                        TableContents::Vec(items) => {
                            parent.children(items.into_iter().enumerate().map(|(index, row)| {
                                div().child(render_table_row(index, row, table_context.clone(), window, cx))
                            }))
                        }
                        TableContents::UniformList(uniform_list_data) => parent.child(
                            uniform_list(uniform_list_data.element_id, uniform_list_data.row_count, {
                                let render_item_fn = uniform_list_data.render_list_of_rows_fn;
                                move |range: Range<usize>, window, cx| {
                                    let elements = render_item_fn(range.clone(), window, cx)
                                        .into_iter()
                                        .map(|raw_row| raw_row.into_table_row(self.cols))
                                        .collect::<Vec<_>>();
                                    elements
                                        .into_iter()
                                        .zip(range)
                                        .map(|(row, row_index)| {
                                            render_table_row(row_index, row, table_context.clone(), window, cx)
                                        })
                                        .collect()
                                }
                            })
                            .size_full()
                            .flex_grow_1()
                            .with_sizing_behavior(ListSizingBehavior::Auto)
                            .with_horizontal_sizing_behavior(horizontal_sizing)
                            .when_some(interaction_state.as_ref(), |this, state| {
                                this.track_scroll(&state.read_with(cx, |s, _| s.scroll_handle.clone()))
                            }),
                        ),
                    })
                    .when_some(body_resize_handles, |parent, handles| parent.child(handles));

                content.into_any_element()
            })
            .when_some(no_rows_rendered.then_some(self.empty_table_callback).flatten(), |this, callback| {
                this.child(h_flex().size_full().p_3().items_start().justify_center().child(callback(window, cx)))
            });

        if let Some(state) = interaction_state.as_ref() {
            // Attach scrollbars (converts Div → Stateful<Div>)
            table
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Both).tracked_scroll_handle(&state.read(cx).scroll_handle),
                    window,
                    cx,
                )
                .restrict_scroll_to_axis()
                .track_focus(&state.read(cx).focus_handle)
                .id(("table", state.entity_id()))
                .into_any_element()
        } else {
            table.into_any_element()
        }
    }
}
