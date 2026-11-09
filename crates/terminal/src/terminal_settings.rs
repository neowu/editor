use std::sync::LazyLock;

use gpui::Pixels;
use gpui::px;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
pub use settings::AlternateScroll;
use settings::TerminalBlink;
use settings::TerminalLineHeight;
use settings::WorkingDirectory;
use task::Shell;

#[derive(Clone, Debug, Deserialize)]
pub struct TerminalSettings {
    pub shell: Shell,
    pub working_directory: WorkingDirectory,
    pub line_height: TerminalLineHeight,
    pub env: FxHashMap<String, String>,
    pub cursor_shape: CursorShape,
    pub blinking: TerminalBlink,
    pub alternate_scroll: AlternateScroll,
    pub option_as_meta: bool,
    pub copy_on_select: bool,
    pub keep_selection_on_copy: bool,
    pub open_links_in_mouse_mode: bool,
    pub button: bool,
    pub default_height: Pixels,
    pub detect_venv: bool,
    pub max_scroll_history_lines: Option<usize>,
    pub scroll_multiplier: f32,
    pub minimum_contrast: f32,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
}

impl TerminalSettings {
    pub fn get() -> &'static Self {
        static SETTINGS: LazyLock<TerminalSettings> = LazyLock::new(TerminalSettings::built_in);
        &SETTINGS
    }

    fn built_in() -> Self {
        TerminalSettings {
            shell: Shell::System,
            working_directory: WorkingDirectory::CurrentProjectDirectory,
            line_height: TerminalLineHeight::Standard,
            env: FxHashMap::default(),
            cursor_shape: CursorShape::Bar,
            blinking: TerminalBlink::On,
            alternate_scroll: AlternateScroll::On,
            option_as_meta: true,
            copy_on_select: true,
            keep_selection_on_copy: true,
            open_links_in_mouse_mode: true,
            button: true,
            default_height: px(320.0),
            detect_venv: true,
            max_scroll_history_lines: Some(10000),
            scroll_multiplier: 1.0,
            minimum_contrast: 45.0,
            path_hyperlink_regexes: vec![
                "File \"(?<path>[^\"]+)\", line (?<line>[0-9]+)".into(),
                [
                    "(?x)",
                    "(?<path>",
                    "    (",
                    "        # multi-char path: first char (not opening delimiter, space, or box drawing char)",
                    "        [^({\\[<\"'`\\ \\u2500-\\u257F]",
                    "        # middle chars: non-space, and colon/paren only if not followed by digit/paren/space",
                    "        ([^\\ :(]|[:(][^0-9()\\ ])*",
                    "        # last char: not closing delimiter or colon",
                    "        [^()}\\]>\"'`.,;:\\ ]",
                    "    |",
                    "        # single-char path: not delimiter, punctuation, space, or box drawing char",
                    "        [^(){}\\[\\]<>\"'`.,;:\\ \\u2500-\\u257F]",
                    "    )",
                    "    # optional line/column suffix (included in path for PathWithPosition::parse_str)",
                    "    (:+[0-9]+(:[0-9]+)?|:?\\([0-9]+([,:]?[0-9]+)?\\))?",
                    ")",
                ]
                .join("\n"),
            ],
            path_hyperlink_timeout_ms: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Cursor is a block like `█`.
    #[default]
    Block,
    /// Cursor is an underscore like `_`.
    Underline,
    /// Cursor is a vertical bar like `⎸`.
    Bar,
    /// Cursor is a hollow box like `▯`.
    Hollow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_path_hyperlink_regexes_compile() {
        let settings = TerminalSettings::built_in();
        assert_eq!(settings.path_hyperlink_regexes.len(), 2);
        let python = regex::Regex::new(&settings.path_hyperlink_regexes[0]).unwrap();
        let captures = python.captures("File \"a/b.py\", line 12").unwrap();
        assert_eq!(&captures["path"], "a/b.py");
        assert_eq!(&captures["line"], "12");
        let common = regex::Regex::new(&settings.path_hyperlink_regexes[1]).unwrap();
        assert_eq!(&common.captures("src/main.rs:10:5.").unwrap()["path"], "src/main.rs:10:5");
    }

    #[test]
    fn get_returns_cached_built_in() {
        let settings = TerminalSettings::get();
        assert_eq!(settings.path_hyperlink_regexes, TerminalSettings::built_in().path_hyperlink_regexes);
        assert!(std::ptr::eq(settings, TerminalSettings::get()));
    }
}
