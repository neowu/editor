use crate::prelude::App;
use crate::prelude::DynamicSpacing;
use crate::prelude::IntoElement;
use crate::prelude::RenderOnce;
use crate::prelude::Styled as _;
use crate::prelude::Window;
use crate::prelude::div;

#[derive(IntoElement)]
pub struct ListSeparator;

impl RenderOnce for ListSeparator {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        div().h_px().w_full().my(DynamicSpacing::Base06.rems()).bg(theme::theme().colors().border_variant)
    }
}
