use rustc_hash::FxHashMap;
use settings_content::AlternateScroll;
use settings_content::CursorShapeContent;
use settings_content::FontWeightContent;
use settings_content::PathHyperlinkRegex;
use settings_content::PixelSetting;
use settings_content::ProjectSettingsContent;
use settings_content::ProjectTerminalSettingsContent;
use settings_content::ScanSymlinksSetting;
use settings_content::SettingsContent;
use settings_content::Shell;
use settings_content::TerminalBlink;
use settings_content::TerminalLineHeight;
use settings_content::TerminalSettingsContent;
use settings_content::WorkingDirectory;
use settings_content::WorktreeSettingsContent;

/// The complete application settings. Edit this constructor to change persistent preferences.
pub fn settings() -> SettingsContent {
    SettingsContent { project: project_settings(), terminal: terminal_settings() }
}

fn terminal_settings() -> Option<TerminalSettingsContent> {
    Some(TerminalSettingsContent {
        project: ProjectTerminalSettingsContent {
            shell: Some(Shell::System),
            working_directory: Some(WorkingDirectory::CurrentProjectDirectory),
            env: Some(FxHashMap::default()),
            detect_venv: Some(true),
            path_hyperlink_regexes: Some(vec![
                PathHyperlinkRegex::SingleLine("File \"(?<path>[^\"]+)\", line (?<line>[0-9]+)".into()),
                PathHyperlinkRegex::MultiLine(vec![
                    "(?x)".into(),
                    "(?<path>".into(),
                    "    (".into(),
                    "        # multi-char path: first char (not opening delimiter, space, or box drawing char)".into(),
                    "        [^({\\[<\"'`\\ \\u2500-\\u257F]".into(),
                    "        # middle chars: non-space, and colon/paren only if not followed by digit/paren/space"
                        .into(),
                    "        ([^\\ :(]|[:(][^0-9()\\ ])*".into(),
                    "        # last char: not closing delimiter or colon".into(),
                    "        [^()}\\]>\"'`.,;:\\ ]".into(),
                    "    |".into(),
                    "        # single-char path: not delimiter, punctuation, space, or box drawing char".into(),
                    "        [^(){}\\[\\]<>\"'`.,;:\\ \\u2500-\\u257F]".into(),
                    "    )".into(),
                    "    # optional line/column suffix (included in path for PathWithPosition::parse_str)".into(),
                    "    (:+[0-9]+(:[0-9]+)?|:?\\([0-9]+([,:]?[0-9]+)?\\))?".into(),
                    ")".into(),
                ]),
            ]),
            path_hyperlink_timeout_ms: Some(1),
        },
        line_height: Some(TerminalLineHeight::Standard),
        font_weight: Some(FontWeightContent(400.0)),
        cursor_shape: Some(CursorShapeContent::Bar),
        blinking: Some(TerminalBlink::On),
        alternate_scroll: Some(AlternateScroll::On),
        option_as_meta: Some(true),
        copy_on_select: Some(true),
        keep_selection_on_copy: Some(true),
        open_links_in_mouse_mode: Some(true),
        button: Some(true),
        default_height: Some(PixelSetting(320.0)),
        max_scroll_history_lines: Some(10000),
        scroll_multiplier: Some(1.0),
        minimum_contrast: Some(45.0),
        ..Default::default()
    })
}

fn project_settings() -> ProjectSettingsContent {
    ProjectSettingsContent {
        worktree: WorktreeSettingsContent {
            file_scan_exclusions: Some(vec![
                "**/.git".into(),
                "**/.svn".into(),
                "**/.hg".into(),
                "**/.jj".into(),
                "**/.sl".into(),
                "**/.repo".into(),
                "**/CVS".into(),
                "**/.DS_Store".into(),
                "**/Thumbs.db".into(),
                "**/.classpath".into(),
                "**/.settings".into(),
            ]),
            file_scan_inclusions: Some(vec![".env*".into()]),
            scan_symlinks: Some(ScanSymlinksSetting::Expanded),
            file_scan_depth: Some(5),
            hidden_files: Some(vec!["**/.*".into()]),
        },
    }
}
