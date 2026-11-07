//! A container query element, in the spirit of CSS container queries.
//! The element's own size is determined solely by its style and the space
//! offered by its parent.

use refineable::Refineable as _;

use crate::AnyElement;
use crate::App;
use crate::AvailableSpace;
use crate::Bounds;
use crate::Element;
use crate::ElementId;
use crate::GlobalElementId;
use crate::IntoElement;
use crate::LayoutId;
use crate::Pixels;
use crate::Size;
use crate::Style;
use crate::StyleRefinement;
use crate::Styled;
use crate::Window;

/// A container query element, created with [`container_query`].
pub struct ContainerQuery {
    render: Option<Box<dyn FnOnce(Size<Pixels>, &mut Window, &mut App) -> AnyElement>>,
    style: StyleRefinement,
}

impl Element for ContainerQuery {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let render = self.render.take()?;
        let mut child = render(bounds.size, window, cx);
        child.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
        child.prepaint_at(bounds.origin, window, cx);
        Some(child)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = prepaint {
            child.paint(window, cx);
        }
    }
}

impl IntoElement for ContainerQuery {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for ContainerQuery {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
