# Module cycles

`cargo lint` rule `module_cycle`: sibling modules must not use each other in a circle. The sets below still do; each
was silenced by marking the cheapest uses to cut with `// lint: expect(module_cycle)`, 971 lines in total. To work on
one, remove its markers and run `cargo lint`, which then prints the shortest circle.

Most are one type whose `impl` is split across files that call each other (`Editor`, `GitPanel`, `ProjectPanel`,
`LspStore`, `MultiBuffer`, `Workspace`), or core types that are mutually recursive (`App`, `Window`, `Context`,
`Entity`). Each table lists the uses that were marked: fixing a set means removing those uses, not necessarily all of
the set's uses.

## `editor`

50 modules, 59 uses marked on 191 lines.

Modules: `actions`, `clipboard`, `code_actions`, `code_context_menus`, `code_lens`, `columnar_selection`, `completions`, `config`, `diagnostics`, `display_map`, `document_colors`, `document_links`, `document_symbols`, `element`, `events`, `fold`, `folding_ranges`, `git`, `highlight_matching_bracket`, `highlights`, `hover_links`, `hover_popover`, `indent_guides`, `inlays`, `input`, `items`, `jsx_tag_auto_close`, `line_edits`, `linked_editing_ranges`, `lsp_actions`, `lsp_data`, `markdown_actions`, `mouse_context_menu`, `movement`, `navigation`, `references`, `rewrap`, `runnables`, `rust_analyzer_ext`, `scroll`, `selection`, `selections_collection`, `semantic_tokens`, `snapshot`, `snippets`, `spell_check`, `split`, `split_editor_view`, `text_transform`, `workspace_actions`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `code_context_menus` | `code_actions` | 4 | `CodeActionProvider`, `confirm_code_action` |
| `code_context_menus` | `completions` | 20 | `CompletionProvider`, `with_completions_menu_matching_id`, `confirm_completion`, `split_words`, `resolve_completions` |
| `code_lens` | `navigation` | 2 | `navigation_entry`, `navigate_to_hover_links` |
| `code_lens` | `selection` | 1 | `change_selections` |
| `completions` | `input` | 1 | `invalidate_autoclose_regions` |
| `completions` | `selection` | 1 | `change_selections` |
| `completions` | `snippets` | 1 | `insert_snippet` |
| `config` | `element` | 2 | `MinimapRowCache` |
| `diagnostics` | `fold` | 1 | `unfold_ranges` |
| `diagnostics` | `selection` | 1 | `change_selections` |
| `display_map` | `element` | 1 | `EditorElement`, `render_spacer_block` |
| `display_map` | `hover_links` | 6 | `InlayHighlight`, `inlay`, `range` |
| `display_map` | `inlays` | 42 | `position`, `Inlay`, `text`, `id`, `InlayContent`, `content` and 1 more |
| `display_map` | `movement` | 4 | `TextLayoutDetails`, `text_system`, `editor_style`, `rem_size`, `scroll_anchor` |
| `display_map` | `split` | 7 | `patches_for_lhs_range`, `patches_for_rhs_range` |
| `events` | `element` | 2 | `EditorElement`, `mouse_moved` |
| `events` | `indent_guides` | 1 | `dirty` |
| `fold` | `indent_guides` | 1 | `dirty` |
| `fold` | `items` | 2 | `folds`, `update_restoration_data` |
| `fold` | `selection` | 1 | `invalidate_add_selection_goals_after_change` |
| `hover_links` | `element` | 8 | `as_valid`, `PointForPosition`, `next_valid` |
| `hover_links` | `inlays` | 2 | `highlight_inlays`, `update_inlay_link_and_hover_points` |
| `hover_links` | `navigation` | 5 | `navigation_entry`, `navigate_to_hover_links`, `semantics_position`, `go_to_singleton_buffer_point_silently`, `go_to_declaration_or_references` |
| `hover_links` | `selection` | 3 | `select`, `has_pending_selection` |
| `hover_popover` | `selection` | 1 | `change_selections` |
| `inlays` | `completions` | 2 | `is_lsp_relevant`, `visible_buffer_ranges` |
| `inlays` | `selection` | 1 | `has_pending_nonempty_selection` |
| `input` | `clipboard` | 1 | `cut_common` |
| `input` | `element` | 4 | `snapshot`, `text_hitbox`, `previous_valid`, `point_for_position` |
| `input` | `line_edits` | 3 | `backspace`, `delete` |
| `input` | `navigation` | 3 | `select_to_end_of_line`, `select_to_beginning_of_line` |
| `linked_editing_ranges` | `input` | 1 | `linked_editing_ranges_for` |
| `lsp_actions` | `element` | 2 | `EditorElement`, `new` |
| `lsp_data` | `completions` | 5 | `visible_buffers`, `is_lsp_relevant` |
| `lsp_data` | `diagnostics` | 2 | `remove_invalid_inline_diagnostics`, `pull_diagnostics` |
| `lsp_data` | `document_colors` | 2 | `remove_invalid_document_colors`, `refresh_document_colors` |
| `lsp_data` | `document_links` | 1 | `refresh_document_links` |
| `lsp_data` | `document_symbols` | 1 | `refresh_document_symbols` |
| `lsp_data` | `folding_ranges` | 1 | `refresh_folding_ranges` |
| `lsp_data` | `inlays` | 2 | `InlayHintRefreshReason`, `NewLinesShown`, `refresh_inlay_hints` |
| `lsp_data` | `runnables` | 1 | `refresh_runnables` |
| `lsp_data` | `semantic_tokens` | 1 | `refresh_semantic_tokens` |
| `lsp_data` | `spell_check` | 1 | `refresh_spell_check` |
| `movement` | `scroll` | 2 | `SharedScrollAnchor` |
| `runnables` | `code_actions` | 1 | `toggle_code_actions` |
| `runnables` | `completions` | 2 | `is_lsp_relevant`, `visible_buffers` |
| `runnables` | `fold` | 1 | `is_buffer_folded` |
| `scroll` | `element` | 7 | `x_for_index`, `em_advance`, `LineWithInvisibles` |
| `scroll` | `lsp_actions` | 2 | `take_rename` |
| `scroll` | `selection` | 1 | `change_selections` |
| `selection` | `input` | 1 | `invalidate_autoclose_regions` |
| `selection` | `items` | 2 | `selections`, `update_restoration_data` |
| `selection` | `lsp_actions` | 1 | `take_rename` |
| `selection` | `navigation` | 1 | `push_to_nav_history` |
| `snapshot` | `git` | 6 | `BlameRenderer`, `max_author_length`, `render_git_blame_gutter`, `blame_entry_non_text_width`, `GlobalBlameRenderer`, `0` |
| `snapshot` | `scroll` | 4 | `ScrollOffset`, `scroll_position`, `shared_scroll_anchor` |
| `split` | `split_editor_view` | 5 | `new`, `SplitEditorState`, `SplitEditorView` |
| `workspace_actions` | `navigation` | 1 | `open_locations_in_multibuffer` |
| `workspace_actions` | `selection` | 1 | `change_selections` |

## `git_ui`

12 modules, 10 uses marked on 45 lines.

Modules: `commit_context_menu`, `commit_details_panel`, `commit_tooltip`, `commit_view`, `diff_multibuffer`, `git_graph`, `git_panel`, `project_diff`, `revision_semantics`, `solo_diff_view`, `staged_diff`, `unstaged_diff`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `commit_details_panel` | `git_graph` | 8 | `GitGraph`, `is_head_ref`, `search_match_ranges`, `ToggleChangedFilesView`, `format_timestamp` |
| `commit_tooltip` | `commit_view` | 2 | `CommitView`, `open_revealing` |
| `commit_view` | `git_graph` | 2 | `OpenAtCommit`, `sha`, `repo_id` |
| `git_panel` | `commit_view` | 2 | `CommitView`, `open` |
| `git_panel` | `git_graph` | 3 | `open_or_reuse_graph` |
| `git_panel` | `project_diff` | 12 | `ProjectDiff`, `Diff`, `DeployBranchDiff`, `deploy_at`, `autoscroll`, `move_to_entry` |
| `git_panel` | `solo_diff_view` | 2 | `open_or_focus`, `SoloDiffView` |
| `git_panel` | `staged_diff` | 6 | `StagedDiff`, `deploy_at`, `move_to_entry` |
| `git_panel` | `unstaged_diff` | 6 | `UnstagedDiff`, `deploy_at`, `move_to_entry` |
| `revision_semantics` | `commit_view` | 2 | `project_file_path` |

## `git_ui::git_panel`

5 modules, 1 use marked on 1 line.

Modules: `commit`, `discard`, `render`, `selection`, `staging`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `selection` | `render` | 1 | `target_entry_index` |

## `gpui`

27 modules, 36 uses marked on 560 lines.

Modules: `action_table`, `app`, `asset_cache`, `assets`, `bounds_tree`, `element`, `elements`, `executor`, `frame_stats`, `geometry`, `global`, `interactive`, `key_dispatch`, `keymap`, `platform`, `scene`, `scroll_axis_lock`, `style`, `styled`, `svg_renderer`, `tab_stop`, `taffy`, `text_system`, `ui_snapshot`, `util`, `view`, `window`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `action_table` | `window` | 8 | `Window` |
| `app` | `element` | 5 | `Render` |
| `app` | `key_dispatch` | 1 | `view_path_reversed` |
| `app` | `taffy` | 2 | `LayoutId` |
| `app` | `view` | 12 | `AnyView`, `downcast` |
| `app` | `window` | 202 | `Window`, `id`, `AnyWindowHandle`, `WindowId`, `handle`, `WindowHandle` and 47 more |
| `asset_cache` | `app` | 11 | `App`, `EntityId`, `background_executor`, `spawn`, `notify`, `update` |
| `executor` | `app` | 4 | `App`, `foreground_executor` |
| `executor` | `platform` | 11 | `PlatformDispatcher`, `RunnableVariant`, `is_main_thread`, `dispatch`, `dispatch_on_main_thread`, `dispatch_after` |
| `frame_stats` | `window` | 4 | `frame_stats`, `Window` |
| `geometry` | `app` | 5 | `App`, `main_display`, `find_display` |
| `geometry` | `platform` | 5 | `PlatformDisplay`, `DisplayId`, `bounds`, `visible_bounds` |
| `geometry` | `window` | 2 | `rem_size`, `Window` |
| `global` | `app` | 4 | `App`, `global` |
| `key_dispatch` | `window` | 17 | `FocusId`, `Window`, `DispatchPhase` |
| `platform` | `app` | 37 | `App`, `update`, `to_async`, `borrow_mut`, `AsyncWindowContext`, `app` and 2 more |
| `platform` | `elements` | 4 | `ImageSource`, `Image`, `use_data`, `get_data`, `remove_asset` |
| `platform` | `interactive` | 4 | `ExternalPaths`, `0`, `PlatformInput` |
| `platform` | `keymap` | 3 | `Keymap` |
| `platform` | `scene` | 3 | `Scene` |
| `platform` | `window` | 27 | `Window`, `AnyWindowHandle`, `DEFAULT_WINDOW_SIZE`, `DispatchEventResult`, `has_pending_keystrokes`, `WindowControlArea` |
| `scene` | `elements` | 5 | `Transformation`, `turned`, `into_matrix` |
| `scene` | `window` | 17 | `ContentMask`, `bounds`, `scale` |
| `style` | `styled` | 2 | `Styled` |
| `style` | `window` | 12 | `ContentMask`, `Window`, `paint_quad`, `quad`, `rem_size`, `bounds` and 2 more |
| `taffy` | `window` | 5 | `Window`, `scale_factor` |
| `text_system` | `app` | 12 | `App`, `text_system` |
| `text_system` | `platform` | 19 | `PlatformTextSystem`, `glyph_for_char`, `layout_line`, `add_fonts`, `all_font_names`, `font_id` and 6 more |
| `text_system` | `style` | 40 | `TextAlign`, `UnderlineStyle`, `StrikethroughStyle`, `thickness`, `color`, `Left` and 3 more |
| `text_system` | `ui_snapshot` | 4 | `record_ui_text`, `ui_snapshots_enabled` |
| `text_system` | `window` | 30 | `Window`, `paint_quad`, `fill`, `paint_underline`, `paint_strikethrough`, `paint_layer` and 4 more |
| `window` | `element` | 40 | `GlobalElementId`, `Render`, `paint`, `IntoElement`, `into_any_element`, `AnyElement` and 7 more |
| `window` | `elements` | 3 | `AnyImageCache` |
| `window` | `tab_stop` | 11 | `TabStopMap`, `next`, `prev`, `replay`, `paint_index`, `insertion_history` and 3 more |
| `window` | `ui_snapshot` | 4 | `UiElement`, `UiText` |
| `window` | `view` | 7 | `downcast`, `AnyView` |

## `gpui::app`

3 modules, 2 uses marked on 10 lines.

Modules: `async_context`, `context`, `entity_map`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `context` | `async_context` | 4 | `AsyncWindowContext`, `AsyncApp` |
| `entity_map` | `context` | 6 | `Context`, `notify` |

## `language`

9 modules, 9 uses marked on 41 lines.

Modules: `available_languages`, `buffer`, `language_registry`, `language_settings`, `outline`, `runnable`, `syntax_map`, `task_context`, `text_diff`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `available_languages` | `language_registry` | 3 | `LanguageLoader`, `LoadedLanguage` |
| `buffer` | `language_settings` | 10 | `LanguageSettings`, `for_buffer_snapshot`, `resolve_uncached`, `tab_size`, `hard_tabs` |
| `buffer` | `runnable` | 4 | `RunnableRange`, `runnable_ranges` |
| `buffer` | `text_diff` | 2 | `text_diff` |
| `language_registry` | `buffer` | 4 | `File`, `path`, `full_path`, `file_system_abs_path`, `file_name` |
| `language_registry` | `language_settings` | 3 | `all_language_settings`, `file_types` |
| `outline` | `buffer` | 7 | `indent_size_for_line`, `BufferSnapshot`, `syntax_ancestor` |
| `syntax_map` | `buffer` | 6 | `File`, `path` |
| `task_context` | `runnable` | 2 | `RunnableResolver` |

## `multi_buffer`

4 modules, 4 uses marked on 8 lines.

Modules: `anchor`, `anchor_resolve`, `path_key`, `transaction`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `anchor` | `anchor_resolve` | 3 | `summary_for_anchor`, `anchor_before`, `anchor_after` |
| `anchor` | `path_key` | 3 | `PathKey` |
| `path_key` | `anchor_resolve` | 1 | `excerpts_for_buffer` |
| `transaction` | `anchor_resolve` | 1 | `summaries_for_anchors` |

## `project::lsp_store`

5 modules, 4 uses marked on 10 lines.

Modules: `capabilities`, `diagnostics`, `dynamic_registration`, `file_watchers`, `semantic_tokens`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `dynamic_registration` | `capabilities` | 2 | `completion_trigger_characters_for_buffer` |
| `dynamic_registration` | `diagnostics` | 4 | `pull_document_diagnostics_for_server`, `clear_unregistered_diagnostics`, `lsp_workspace_diagnostics_refresh` |
| `dynamic_registration` | `semantic_tokens` | 2 | `refresh_semantic_tokens` |
| `file_watchers` | `dynamic_registration` | 2 | `did_change_watched_files` |

## `project_panel`

6 modules, 3 uses marked on 4 lines.

Modules: `clipboard`, `drag`, `entry_edit`, `render`, `undo`, `visible_entries`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `clipboard` | `drag` | 1 | `drop_external_files` |
| `undo` | `entry_edit` | 1 | `build_removal_prompt` |
| `visible_entries` | `render` | 2 | `entry_icon`, `details_for_entry` |

## `workspace`

19 modules, 27 uses marked on 101 lines.

Modules: `dock`, `docks`, `invalid_item_view`, `item`, `item_registry`, `mode`, `navigation_history`, `notifications`, `open`, `pane`, `pane_group`, `pane_management`, `persistence`, `preview`, `searchable`, `serialization`, `status_bar`, `toolbar`, `window_header`.

| From | To | Lines | Items |
| --- | --- | --- | --- |
| `dock` | `docks` | 3 | `dismiss_zoomed_items_to_reveal`, `DraggedDock`, `0` |
| `invalid_item_view` | `item` | 2 | `Item` |
| `item` | `item_registry` | 2 | `SerializableItemRegistry`, `view_to_serializable_item_handle` |
| `item` | `pane` | 15 | `Pane`, `SaveIntent`, `Event`, `active_item`, `close_item_by_id`, `Close` and 4 more |
| `item` | `pane_management` | 1 | `active_pane` |
| `item` | `searchable` | 4 | `SearchableItemHandle` |
| `item` | `serialization` | 3 | `serialize_workspace` |
| `mode` | `navigation_history` | 1 | `content_pane_focused` |
| `mode` | `window_header` | 7 | `render_sidebar_header`, `window_header_in_tab_bar`, `render_sidebar_header_overlay`, `assign_tab_bar_chrome`, `render_zoomed_dock_header` |
| `notifications` | `open` | 1 | `open_url_or_file` |
| `pane` | `item_registry` | 2 | `WorkspaceItemBuilder` |
| `pane` | `navigation_history` | 4 | `navigate_tag_history`, `go_back`, `go_forward` |
| `pane` | `open` | 2 | `load_path`, `prompt_for_new_path` |
| `pane` | `pane_group` | 21 | `SplitDirection`, `Down`, `Right`, `Up`, `Left`, `panes` |
| `pane` | `pane_management` | 5 | `split_pane`, `move_item` |
| `pane` | `preview` | 8 | `OpenPreview`, `has_preview`, `OpenPreviewToTheSide`, `open_preview`, `render_tab_bar_preview_button` |
| `pane` | `status_bar` | 1 | `set_active_pane` |
| `pane_group` | `serialization` | 2 | `serialize_workspace` |
| `pane_management` | `navigation_history` | 2 | `is_deleted_history_path`, `content_pane_focused` |
| `persistence` | `pane_management` | 2 | `add_pane`, `force_remove_pane` |
| `persistence` | `serialization` | 1 | `project_location` |
| `serialization` | `dock` | 1 | `restore_serialized_state` |
| `serialization` | `docks` | 1 | `capture_dock_state` |
| `serialization` | `mode` | 4 | `TerminalLayout`, `Pending`, `terminal_layout_to_save` |
| `serialization` | `open` | 2 | `open_path`, `window` |
| `serialization` | `pane_management` | 3 | `set_active_pane`, `remove_panes` |
| `toolbar` | `pane` | 1 | `Pane` |
