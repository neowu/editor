# Dead code cleanup — handoff

Goal: the workspace is self-contained (no external users of any crate), so every
`pub` item that no crate uses is dead and should be deleted. rustc's `dead_code`
lint can't see this for library crates, because it treats `pub` items as used.

## Done (first pass)

- Deleted 341 dead free functions, methods, associated functions/consts, consts,
  statics and type aliases, plus the imports and empty inherent `impl` blocks
  they left behind.
- Removed what that exposed: `Divider` dashed style, the `KeyBinding` keystrokes
  source, `ScrollStrategy::Top` (div), `ReservedSpace::StableTrack`,
  `ShapedLineCursor`, `CornerSolver`, `DivRegistrar`, `TruncatedPatch`,
  `StyledAnsiTextHandler`, `MapSeekTarget` and its adaptor.
- Removed unused deps: `rand` (gpui), `diffy` (language and workspace),
  `futures-lite` and `rust-embed` (util). `editor` and `git_ui_core` now enable
  `rand`'s `thread_rng` themselves; before, they got it through gpui via feature
  unification.
- Visibility is unchanged: every `pub` is still `pub`.

## Method

1. **Narrow:** rewrite every `pub` item, field and `use` to `pub(crate)`. Skip
   proc-macro crates and files containing `macro_rules!`.
2. **Restore:** `cargo check --workspace --all-targets --all-features` and
   put `pub` back on whatever another crate needs. Map each error to the
   definition through the "defined here" span. When there's no span (E0616,
   E0451, E0365, "cannot find", "associated type … is private"), fall back to
   the item's name. Repeat until there are no errors. Each fix lets more
   downstream crates compile, so this takes many rounds. Use
   `CARGO_INCREMENTAL=0`; incremental mode hit an internal compiler error.
3. **Collect dead items:** keep only items that rustc reports as dead in
   **both** `cargo check --lib --bins` and `cargo check --tests`. A test build
   alone reports items the normal build uses, and the normal build alone
   reports items only the tests use.
4. **Delete:** remove each item together with its doc comments and
   attributes, then rebuild. If a deleted name shows up in an error, put that
   item back and try again.
5. **Repeat:** prune unused imports, then run another round (round 2 found
   16 more, round 3 found none).
6. **Revert visibility:** turn every remaining narrowed `pub(crate)` back into
   `pub`, matching lines against a snapshot taken before deletion. Then fix
   the warnings that newly appear for private items that only the deleted code
   used.

### Pitfalls found

- **Silent fallback:** hiding an inherent method can make a call resolve to a
  different method without any error. `SharedString::as_str` fell back to
  `str::as_str`, and `AnyElement::prepaint`/`paint` fell back to the `Element`
  trait methods. That's why the delete step skips any item whose name appears
  in another crate (see the appendix). An item that is only bypassed this way
  shows up as dead, and deleting it would change behaviour.
- **Globs hide privacy errors:** a glob re-export (`pub use m::*`) silently
  leaves out items that aren't public, so narrowing shows up as "cannot find X"
  instead of a privacy error.
- **Derives copy visibility:** `#[derive(Refineable)]` generates its
  `…Refinement` types with the struct's visibility, so Refineable structs and
  their fields have to stay `pub`.
- **Trait items:** a trait method reported as dead can't be deleted while its
  impls still exist.
- **Empty impls:** only delete empty *inherent* impls. Empty trait impls
  (`impl EventEmitter<E> for X {}`, marker traits) are required.

## Remaining work

1. **Dead types, not yet deleted:** structs, enums, traits, fields and enum
   variants that are dead under narrowing. They were skipped because deleting
   them also means deleting their impls and constructors, so they need edits
   one at a time. Re-run steps 1–3 to list them.
2. **Possibly dead modules:** these glob re-exports had no users outside their
   own crate (list taken under narrowing; 20 of 24 shown, re-run to get the
   rest). Check whether each module is used at all; if not, delete it:
   - `crates/clock/src/clock.rs`: `pub use system_clock::*;`
   - `crates/editor/src/display_map.rs`: `pub use crease_map::*;`
   - `crates/git/src/git.rs`: `pub use crate::remote::*;`
   - `crates/gpui/src/elements/mod.rs`: `pub use container_query::*;`
   - `crates/gpui/src/gpui.rs`: `pub use spring::*;`
   - `crates/gpui/src/window.rs`: `pub use prompts::*;`
   - `crates/tabular_data_preview/src/types.rs`: `pub use table_cell::*;`,
     `pub use table_like_content::*;`
   - `crates/ui/src/components.rs`: `avatar`, `callout`, `dropdown_menu`,
     `facepile`, `keybinding_hint`, `notification`, `progress`,
     `tree_view_item`
   - `crates/ui/src/components/button.rs`: `button_link`
   - `crates/ui/src/components/label.rs`: `loading_label`
   - `crates/ui/src/components/list.rs`: `list_bullet_item`, `list_header`
3. **Items kept by the name guard:** the appendix lists 259 dead items that
   were kept only because the same name appears in another crate. For each,
   check whether the uses elsewhere really belong to a different item
   (rust-analyzer find references works), then delete.
4. **Files never narrowed:** files with `macro_rules!` and the proc-macro
   crates were skipped, so dead code in them hasn't been checked.
5. **Linux-only code:** only macOS was compiled. Check the Linux remote-server
   build (`target_os = "linux"` code paths) for anything that used deleted
   items.

## Appendix: dead items kept by the name guard

Line numbers are left out because they changed during the cleanup; search for
the name in the file.

- `crates/command_palette/src/command_palette.rs`: `set_query`
- `crates/editor/src/code_actions.rs`: `id`
- `crates/editor/src/display_map.rs`: `unique`
- `crates/editor/src/display_map/block_map.rs`: `style`
- `crates/editor/src/display_map/crease_map.rs`: `block`, `inline`, `metadata`
- `crates/editor/src/display_map/wrap_map.rs`: `longest_row`
- `crates/editor/src/hover_popover.rs`: `scroll`
- `crates/editor/src/inlays/inlay_hints.rs`: `supports_inlay_hints`
- `crates/editor/src/runnables.rs`: `task_context`
- `crates/editor/src/scroll/scroll_amount.rs`: `direction`, `pixels`
- `crates/fuzzy_nucleo/src/fuzzy_nucleo.rs`: `from_bool`
- `crates/git/src/repository.rs`: `short_sha`
- `crates/git/src/status.rs`: `DELETED`, `index`, `is_ignored`, `worktree`
- `crates/git_ui/src/branch_picker.rs`: `new`, `open`, `popover`, `switch`
- `crates/git_ui/src/project_diff.rs`: `diff_base`, `editor`
- `crates/git_ui/src/stash_picker.rs`: `new`, `open`
- `crates/git_ui_core/src/worktree_service.rs`: `branch_target`
- `crates/gpui/src/action.rs`: `documentation`
- `crates/gpui/src/app.rs`: `invoke`, `remove`, `replace`, `to_async`, `update`
- `crates/gpui/src/app/async_context.rs`: `read_global`
- `crates/gpui/src/app/entity_map.rs`: `as_mut`
- `crates/gpui/src/color.rs`: `color_space`, `green`, `yellow`
- `crates/gpui/src/colors.rs`: `dark`, `get_global`, `light`
- `crates/gpui/src/element.rs`: `downcast_mut`
- `crates/gpui/src/elements/animation.rs`: `map_element`, `quadratic`
- `crates/gpui/src/elements/deferred.rs`: `priority`
- `crates/gpui/src/elements/div.rs`: `children_count`
- `crates/gpui/src/elements/svg.rs`: `data`, `scale`, `translate`
- `crates/gpui/src/elements/text.rs`: `new`, `on_click`, `on_hover`, `tooltip`, `wrapped_text`
- `crates/gpui/src/executor.rs`: `prevent_app_nap`
- `crates/gpui/src/geometry.rs`: `is_center`, `maximized`, `scale`
- `crates/gpui/src/keymap/binding.rs`: `meta`
- `crates/gpui/src/path_builder.rs`: `scale`, `transform`, `translate`
- `crates/gpui/src/platform.rs`: `apply_to`
- `crates/gpui/src/platform/app_menu.rs`: `checked`, `is_checked`, `is_disabled`
- `crates/gpui/src/platform/keystroke.rs`: `control`, `control_shift`, `shift`
- `crates/gpui/src/scene.rs`: `apply`
- `crates/gpui/src/spring.rs`: `clamp`, `from`, `interpolate`, `new`, `to`
- `crates/gpui/src/text_system.rs`: `ALL`, `EXTRA_BOLD`, `EXTRA_LIGHT`, `LIGHT`, `THIN`, `bold`, `cap_height`, `grapheme`, `line_gap`, `new`, `recv`, `report`, `reset`, `underline_position`, `underline_thickness`, `x_height`
- `crates/gpui/src/text_system/line.rs`: `split_at`, `x_offset`
- `crates/gpui/src/text_system/line_layout.rs`: `font_size`, `runs`, `split_at`
- `crates/gpui/src/window.rs`: `border_widths`, `duration`, `pause`, `remaining`, `resume`, `timeout`
- `crates/language/src/buffer.rs`: `highlighted_chunks`
- `crates/language/src/diagnostic_set.rs`: `groups`, `resolve`
- `crates/language/src/language.rs`: `code_fence_block_name`, `collapsed_placeholder`, `language_name`
- `crates/language/src/language_registry.rs`: `lsp_adapter`
- `crates/language/src/outline.rs`: `as_match`, `body_range`, `candidate_id`
- `crates/language/src/syntax_map.rs`: `update_count`
- `crates/language/src/text_diff.rs`: `finish`, `flush`, `new`, `process_change`
- `crates/language_core/src/grammar.rs`: `around`
- `crates/lsp/src/lsp.rs`: `process_name`
- `crates/markdown/src/markdown.rs`: `has_selection`
- `crates/multi_buffer/src/multi_buffer.rs`: `MIN`, `has_multiple_hunks`, `starts_new_buffer`
- `crates/notifications/src/status_toast.rs`: `auto_dismiss`
- `crates/path/src/abs_path.rs`: `canonicalize`, `home_dir`, `new`, `push`
- `crates/path/src/rel_path.rs`: `is_descendant_of`
- `crates/picker/src/popover_menu.rs`: `new`, `offset`, `with_handle`
- `crates/project/src/git_store.rs`: `display_name`
- `crates/project/src/git_store/diff_buffer_list.rs`: `statuses_by_path`
- `crates/project/src/lsp_store.rs`: `new`
- `crates/project/src/manifest_tree/manifest_store.rs`: `unregister`
- `crates/project/src/project.rs`: `root_path`
- `crates/project/src/project_search.rs`: `flush`, `new`, `push`
- `crates/project/src/terminals.rs`: `first_project_directory`
- `crates/project/src/worktree_store.rs`: `diagnostics`, `is_empty`
- `crates/refineable/src/refineable.rs`: `base`
- `crates/rope/src/chunk.rs`: `floor_char_boundary`, `tabs`
- `crates/rope/src/rope.rs`: `is_char_boundary`
- `crates/scheduler/src/executor.rs`: `session_id`, `spawn_dedicated`
- `crates/scheduler/src/scheduler.rs`: `weight`
- `crates/search/src/buffer_search/registrar.rs`: `new`
- `crates/search/src/search.rs`: `none`
- `crates/settings_content/src/font.rs`: `EXTRA_BOLD`, `EXTRA_LIGHT`, `LIGHT`, `THIN`
- `crates/sum_tree/src/cursor.rs`: `item_summary`
- `crates/sum_tree/src/tree_map.rs`: `closest`
- `crates/tabular_data_preview/src/types/coordinates.rs`: `get`, `new`
- `crates/terminal/src/alacritty.rs`: `new`
- `crates/terminal/src/pty_info.rs`: `pid`
- `crates/terminal/src/terminal.rs`: `finish`
- `crates/terminal_view/src/terminal_element.rs`: `column`, `line`
- `crates/terminal_view/src/terminal_view.rs`: `entity`
- `crates/text/src/locator.rs`: `is_empty`, `len`
- `crates/text/src/patch.rs`: `retain_mut`
- `crates/text/src/selection.rs`: `equals`
- `crates/text/src/text.rs`: `tabs`
- `crates/theme/src/styles/status.rs`: `diagnostic`
- `crates/ui/src/components/avatar.rs`: `border_color`, `grayscale`, `indicator`, `new`, `size`
- `crates/ui/src/components/button/button_like.rs`: `hoverable_tooltip`
- `crates/ui/src/components/button/button_link.rs`: `label_color`, `label_size`, `new`
- `crates/ui/src/components/button/icon_button.rs`: `hoverable_tooltip`
- `crates/ui/src/components/button/toggle_button.rs`: `new`, `selected`, `tooltip`
- `crates/ui/src/components/facepile.rs`: `empty`, `new`
- `crates/ui/src/components/icon.rs`: `indicator_color`
- `crates/ui/src/components/indicator.rs`: `bar`
- `crates/ui/src/components/keybinding_hint.rs`: `new`, `prefix`, `size`, `suffix`
- `crates/ui/src/components/label/highlighted_label.rs`: `flex_grow`, `flex_shrink_0`
- `crates/ui/src/components/label/label.rs`: `flex_grow`, `flex_shrink_0`, `line_clamp`, `text_center`
- `crates/ui/src/components/label/label_like.rs`: `line_clamp`
- `crates/ui/src/components/label/loading_label.rs`: `new`
- `crates/ui/src/components/label/spinner_label.rs`: `dots`
- `crates/ui/src/components/list/list_bullet_item.rs`: `label_color`, `new`
- `crates/ui/src/components/list/list_item.rs`: `on_toggle`, `outlined`, `rounded`
- `crates/ui/src/components/list/list_sub_header.rs`: `left_icon`
- `crates/ui/src/components/modal.rs`: `contained`, `description`, `end_slot`, `headline`, `meta`, `new`, `padded`, `show_dismiss`
- `crates/ui/src/components/popover.rs`: `aside`
- `crates/ui/src/components/popover_menu.rs`: `full_width`
- `crates/ui/src/components/progress/progress_bar.rs`: `bg_color`, `new`, `value`
- `crates/ui/src/components/tab.rs`: `content_height`
- `crates/ui/src/components/toggle.rs`: `checkbox`, `color`, `description`, `disabled`, `new`, `placeholder`, `switch`, `tab_index`, `tooltip`
- `crates/ui/src/components/tooltip.rs`: `meta`
- `crates/ui/src/styles/typography.rs`: `pixels`
- `crates/ui/src/utils/corner_solver.rs`: `corner_radius`, `new`
- `crates/util/src/command.rs`: `env_remove`
- `crates/util/src/path_list.rs`: `deserialize`, `order`, `serialize`
- `crates/util/src/process.rs`: `kill`
- `crates/util/src/shell.rs`: `program`, `shell_kind`
- `crates/util/src/util.rs`: `default`
- `crates/watch/src/watch.rs`: `constant`
- `crates/workspace/src/dock.rs`: `zoom_out`
- `crates/workspace/src/pane_group.rs`: `edge`, `resize`
- `crates/workspace/src/path_link.rs`: `first_unbalanced_open_paren`
- `crates/workspace/src/workspace.rs`: `workspaces`
- `crates/worktree/src/worktree.rs`: `MIN`, `client`
