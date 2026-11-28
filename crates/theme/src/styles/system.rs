use gpui::Hsla;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemColors {
    pub transparent: Hsla,
    pub(crate) mac_os_traffic_light_red: Hsla,
    pub(crate) mac_os_traffic_light_yellow: Hsla,
    pub(crate) mac_os_traffic_light_green: Hsla,
}
