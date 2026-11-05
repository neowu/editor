use collections::HashMap;
use settings_content::AllLanguageSettingsContent;
use settings_content::AlternateScroll;
use settings_content::AutoIndentMode;
use settings_content::CompletionSettingsContent;
use settings_content::CursorShapeContent;
use settings_content::DelayMs;
use settings_content::DiagnosticSeverityContent;
use settings_content::DiagnosticsSettingsContent;
use settings_content::DocumentFoldingRanges;
use settings_content::DocumentSymbols;
use settings_content::FileFinderSettingsContent;
use settings_content::FileTypeMap;
use settings_content::FontWeightContent;
use settings_content::FormatOnSave;
use settings_content::Formatter;
use settings_content::FormatterList;
use settings_content::GlobalLspSettingsContent;
use settings_content::ImageFileSizeUnit;
use settings_content::ImageViewerSettingsContent;
use settings_content::IncludeIgnoredContent;
use settings_content::IndentGuideBackgroundColoring;
use settings_content::IndentGuideColoring;
use settings_content::IndentGuideSettingsContent;
use settings_content::InlayHintSettingsContent;
use settings_content::InlineDiagnosticsSettingsContent;
use settings_content::JsxTagAutoCloseSettingsContent;
use settings_content::LanguageServerFormatterSpecifier;
use settings_content::LanguageSettingsContent;
use settings_content::LanguageTaskSettingsContent;
use settings_content::LanguageToSettingsMap;
use settings_content::LineEndingSetting;
use settings_content::LspInsertMode;
use settings_content::LspNotificationSettingsContent;
use settings_content::LspPullDiagnosticsSettingsContent;
use settings_content::LspSettings;
use settings_content::LspSettingsMap;
use settings_content::ModalWidthContent;
use settings_content::PathHyperlinkRegex;
use settings_content::PixelSetting;
use settings_content::ProjectSettingsContent;
use settings_content::ProjectTerminalSettingsContent;
use settings_content::RewrapBehavior;
use settings_content::ScanSymlinksSetting;
use settings_content::SemanticTokenRule;
use settings_content::SemanticTokenRules;
use settings_content::SettingsContent;
use settings_content::Shell;
use settings_content::ShowWhitespaceSetting;
use settings_content::SoftWrap;
use settings_content::SoftWrapIndent;
use settings_content::TabSwitcherSettingsContent;
use settings_content::TerminalBlink;
use settings_content::TerminalLineHeight;
use settings_content::TerminalSettingsContent;
use settings_content::WhitespaceMapContent;
use settings_content::WordsCompletionMode;
use settings_content::WorkingDirectory;
use settings_content::WorktreeSettingsContent;

/// The complete application settings. Edit this constructor to change persistent preferences.
pub fn settings() -> SettingsContent {
    SettingsContent {
        project: project_settings(),
        file_finder: Some(FileFinderSettingsContent {
            file_icons: Some(true),
            modal_max_width: Some(ModalWidthContent::Large),
            skip_focus_for_active_in_search: Some(true),
            include_ignored: Some(IncludeIgnoredContent::Smart),
            prefill_query_from_selection: Some(true),
        }),
        tab_switcher: Some(TabSwitcherSettingsContent { show_file_preview: Some(true) }),
        diagnostics: Some(DiagnosticsSettingsContent {
            button: Some(true),
            lsp_pull_diagnostics: Some(LspPullDiagnosticsSettingsContent {
                enabled: Some(true),
                debounce_ms: Some(DelayMs(50)),
            }),
            inline: Some(InlineDiagnosticsSettingsContent {
                update_debounce_ms: Some(DelayMs(150)),
                padding: Some(4),
                min_column: Some(0),
                max_severity: Some(DiagnosticSeverityContent::Error),
            }),
        }),
        global_lsp_settings: Some(GlobalLspSettingsContent {
            button: Some(true),
            request_timeout: Some(120),
            max_buffer_line_length: Some(20000),
            notifications: Some(LspNotificationSettingsContent { dismiss_timeout_ms: Some(5000) }),
            semantic_token_rules: Some(SemanticTokenRules {
                rules: vec![
                    SemanticTokenRule {
                        token_type: Some("enumMember".into()),
                        style: vec!["type".into()],
                        ..Default::default()
                    },
                    SemanticTokenRule {
                        token_type: Some("namespace".into()),
                        style: vec!["variable".into()],
                        ..Default::default()
                    },
                ],
            }),
        }),
        image_viewer: Some(ImageViewerSettingsContent { unit: Some(ImageFileSizeUnit::Binary) }),
        terminal: terminal_settings(),
    }
}

fn terminal_settings() -> Option<TerminalSettingsContent> {
    Some(TerminalSettingsContent {
        project: ProjectTerminalSettingsContent {
            shell: Some(Shell::System),
            working_directory: Some(WorkingDirectory::CurrentProjectDirectory),
            env: Some(HashMap::default()),
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
        all_languages: language_settings(),
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
            read_only_files: Some(vec![]),
        },
        lsp: LspSettingsMap(
            [(
                "rust-analyzer".into(),
                LspSettings {
                    initialization_options: Some(serde_json::json!({
                        "rustfmt": { "extraArgs": ["+nightly"] },
                        "check": { "command": "clippy" },
                        "cargo": { "features": "all" },
                    })),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        ),
    }
}

fn language_settings() -> AllLanguageSettingsContent {
    AllLanguageSettingsContent {
        defaults: LanguageSettingsContent {
            tab_size: Some(std::num::NonZeroU32::new(4).unwrap()),
            hard_tabs: Some(false),
            soft_wrap: Some(SoftWrap::None),
            soft_wrap_indent: Some(SoftWrapIndent::Same),
            preferred_line_length: Some(120),
            show_wrap_guides: Some(true),
            wrap_guides: Some(vec![120]),
            indent_guides: Some(IndentGuideSettingsContent {
                enabled: Some(true),
                line_width: Some(1),
                active_line_width: Some(1),
                coloring: Some(IndentGuideColoring::IndentAware),
                background_coloring: Some(IndentGuideBackgroundColoring::Disabled),
            }),
            format_on_save: Some(FormatOnSave::On),
            remove_trailing_whitespace_on_save: Some(true),
            ensure_final_newline_on_save: Some(true),
            line_ending: Some(LineEndingSetting::EnforceLf),
            formatter: Some(FormatterList::Single(Formatter::Auto)),
            jsx_tag_auto_close: Some(JsxTagAutoCloseSettingsContent { enabled: Some(true) }),
            enable_language_server: Some(true),
            document_folding_ranges: Some(DocumentFoldingRanges::Off),
            document_symbols: Some(DocumentSymbols::On),
            allow_rewrap: Some(RewrapBehavior::InComments),
            show_whitespaces: Some(ShowWhitespaceSetting::Selection),
            whitespace_map: Some(WhitespaceMapContent { space: Some('•'), tab: Some('→') }),
            extend_comment_on_newline: Some(true),
            extend_list_on_newline: Some(true),
            indent_list_on_tab: Some(true),
            inlay_hints: Some(InlayHintSettingsContent {
                show_type_hints: Some(true),
                show_parameter_hints: Some(true),
                show_other_hints: Some(true),
                show_background: Some(false),
                edit_debounce_ms: Some(700),
                scroll_debounce_ms: Some(50),
            }),
            use_autoclose: Some(true),
            use_auto_surround: Some(true),
            always_treat_brackets_as_autoclosed: Some(false),
            use_on_type_format: Some(true),
            code_actions_on_format: Some(HashMap::default()),
            linked_edits: Some(true),
            auto_indent: Some(AutoIndentMode::SyntaxAware),
            auto_indent_on_paste: Some(true),
            tasks: Some(LanguageTaskSettingsContent {
                variables: Some(HashMap::default()),
                enabled: Some(true),
                prefer_lsp: Some(true),
            }),
            show_completions_on_input: Some(true),
            show_completion_documentation: Some(true),
            completions: Some(CompletionSettingsContent {
                words: Some(WordsCompletionMode::Fallback),
                words_min_length: Some(3),
                lsp: Some(true),
                lsp_fetch_timeout_ms: Some(0),
                lsp_insert_mode: Some(LspInsertMode::ReplaceSuffix),
            }),
            word_diff_enabled: Some(true),
            colorize_brackets: Some(false),
        },
        languages: language_overrides(),
        file_types: Some(FileTypeMap(
            [
                ("JSONC".into(), vec!["tsconfig*.json".into()]),
                (
                    "Markdown".into(),
                    vec![".rules".into(), ".cursorrules".into(), ".windsurfrules".into(), ".clinerules".into()],
                ),
                ("Env".into(), vec![".env.*".into()]),
            ]
            .into_iter()
            .collect(),
        )),
    }
}

fn language_overrides() -> LanguageToSettingsMap {
    LanguageToSettingsMap(
        [
            ("C".into(), LanguageSettingsContent { use_on_type_format: Some(false), ..Default::default() }),
            ("C++".into(), LanguageSettingsContent { use_on_type_format: Some(false), ..Default::default() }),
            (
                "Diff".into(),
                LanguageSettingsContent {
                    remove_trailing_whitespace_on_save: Some(false),
                    ensure_final_newline_on_save: Some(false),
                    ..Default::default()
                },
            ),
            (
                "Git Commit".into(),
                LanguageSettingsContent {
                    soft_wrap: Some(SoftWrap::EditorWidth),
                    allow_rewrap: Some(RewrapBehavior::Anywhere),
                    ..Default::default()
                },
            ),
            (
                "Go".into(),
                LanguageSettingsContent {
                    hard_tabs: Some(true),
                    code_actions_on_format: Some([("source.organizeImports".into(), true)].into_iter().collect()),
                    ..Default::default()
                },
            ),
            ("JSON".into(), LanguageSettingsContent { preferred_line_length: Some(120), ..Default::default() }),
            (
                "Markdown".into(),
                LanguageSettingsContent {
                    soft_wrap: Some(SoftWrap::EditorWidth),
                    preferred_line_length: Some(300),
                    remove_trailing_whitespace_on_save: Some(false),
                    allow_rewrap: Some(RewrapBehavior::Anywhere),
                    use_on_type_format: Some(false),
                    // No markdown language server; outline and breadcrumbs come from tree-sitter.
                    document_symbols: Some(DocumentSymbols::Off),
                    completions: Some(CompletionSettingsContent {
                        words: Some(WordsCompletionMode::Disabled),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
            (
                "Plain Text".into(),
                LanguageSettingsContent {
                    soft_wrap: Some(SoftWrap::EditorWidth),
                    allow_rewrap: Some(RewrapBehavior::Anywhere),
                    completions: Some(CompletionSettingsContent {
                        words: Some(WordsCompletionMode::Disabled),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
            (
                "Python".into(),
                LanguageSettingsContent {
                    formatter: Some(FormatterList::Single(Formatter::LanguageServer(
                        LanguageServerFormatterSpecifier::Specific { name: "ruff".into() },
                    ))),
                    code_actions_on_format: Some([("source.organizeImports.ruff".into(), true)].into_iter().collect()),
                    ..Default::default()
                },
            ),
            ("Rust".into(), LanguageSettingsContent { ..Default::default() }),
        ]
        .into_iter()
        .collect(),
    )
}
