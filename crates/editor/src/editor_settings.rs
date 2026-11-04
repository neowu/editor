use settings::Settings;

#[derive(Clone)]
pub struct EditorSettings {
    pub file_diff: FileDiffSettings,
}

settings::register_setting!(EditorSettings);

#[derive(Clone, Copy, Debug)]
pub struct FileDiffSettings {
    /// Whether newly opened file diffs show the full file instead of changes only.
    pub show_full_file: bool,
}

impl Settings for EditorSettings {
    fn from_settings(content: &settings::SettingsContent) -> EditorSettings {
        let file_diff = content.git.as_ref().unwrap().file_diff.unwrap();
        EditorSettings { file_diff: FileDiffSettings { show_full_file: file_diff.show_full_file.unwrap() } }
    }
}
