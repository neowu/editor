use gpui::IntoElement;

use crate::IconDecoration;
use crate::prelude::App;
use crate::prelude::Icon;
use crate::prelude::ParentElement;
use crate::prelude::RenderOnce;
use crate::prelude::Styled;
use crate::prelude::Window;
use crate::prelude::div;

#[derive(IntoElement)]
pub struct DecoratedIcon {
    icon: Icon,
    decoration: Option<IconDecoration>,
}

impl DecoratedIcon {
    pub const fn new(icon: Icon, decoration: Option<IconDecoration>) -> Self {
        Self { icon, decoration }
    }
}

impl RenderOnce for DecoratedIcon {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().relative().size(self.icon.size).child(self.icon).children(self.decoration)
    }
}
