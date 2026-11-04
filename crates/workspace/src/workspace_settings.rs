use serde::Deserialize;
pub use settings::EncodingDisplayOptions;
pub use settings::Settings;

#[derive(Deserialize)]
pub struct StatusBarSettings {
    pub show: bool,
    pub active_language_button: bool,
    pub cursor_position_button: bool,
    pub active_encoding_button: EncodingDisplayOptions,
}

settings::register_setting!(StatusBarSettings);

impl Settings for StatusBarSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let status_bar = content.status_bar.clone().unwrap();
        StatusBarSettings {
            show: status_bar.show.unwrap(),
            active_language_button: status_bar.active_language_button.unwrap(),
            cursor_position_button: status_bar.cursor_position_button.unwrap(),
            active_encoding_button: status_bar.active_encoding_button.unwrap(),
        }
    }
}
