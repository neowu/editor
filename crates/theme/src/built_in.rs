use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use gpui::FontStyle;
use gpui::FontWeight;
use gpui::HighlightStyle;
use gpui::Hsla;
use gpui::SharedString;
use gpui::WindowBackgroundAppearance;
use gpui::rgb;
use gpui::transparent_black;

use crate::AccentColors;
use crate::Appearance;
use crate::CursorColors;
use crate::StatusColors;
use crate::SyntaxTheme;
use crate::SystemColors;
use crate::Theme;
use crate::ThemeColors;
use crate::ThemeStyles;

/// Buffer and terminal line height, relative to the font size.
pub const BUFFER_LINE_HEIGHT: f32 = 1.618;
/// Opacity reduction applied to unnecessary (e.g. unused) code.
pub const UNNECESSARY_CODE_FADE: f32 = 0.3;

static LIGHT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The active theme: Nord with the floating-card chrome, dark or light after the OS appearance.
/// Edit the palettes below to change the look.
#[inline]
pub fn theme() -> &'static Theme {
    theme_for(if LIGHT_ACTIVE.load(Ordering::Relaxed) { Appearance::Light } else { Appearance::Dark })
}

/// The theme of one appearance. Both are built lazily, once.
pub fn theme_for(appearance: Appearance) -> &'static Theme {
    static DARK_THEME: LazyLock<Theme> = LazyLock::new(|| build("nord", "Nord", Appearance::Dark, &DARK));
    static LIGHT_THEME: LazyLock<Theme> =
        LazyLock::new(|| build("nord-light", "Nord Light", Appearance::Light, &LIGHT));
    match appearance {
        Appearance::Dark => &DARK_THEME,
        Appearance::Light => &LIGHT_THEME,
    }
}

/// Switches the theme [`theme`] returns; `true` if it changed. Callers refresh the windows.
pub fn set_appearance(appearance: Appearance) -> bool {
    LIGHT_ACTIVE.swap(appearance.is_light(), Ordering::Relaxed) != appearance.is_light()
}

fn build(id: &str, name: &'static str, appearance: Appearance, p: &Palette) -> Theme {
    Theme {
        id: id.into(),
        name: SharedString::new_static(name),
        appearance,
        styles: ThemeStyles {
            window_background_appearance: WindowBackgroundAppearance::Opaque,
            system: system_colors(),
            accents: accent_colors(p),
            colors: theme_colors(p),
            status: status_colors(p),
            cursor_colors: cursor_colors(p),
            syntax: syntax_theme(p),
        },
    }
}

// Nord palette: with the light inks below, the only raw colour values. Everything else derives from these.
const DARK0: u32 = 0x2e3440;
const DARK1: u32 = 0x3b4252;
const DARK2: u32 = 0x434c5e;
const DARK3: u32 = 0x4c566a;
const LIGHT0: u32 = 0xd8dee9;
const LIGHT1: u32 = 0xe5e9f0;
const LIGHT2: u32 = 0xeceff4;
const FROST0: u32 = 0x8fbcbb;
const FROST1: u32 = 0x88c0d0;
const FROST2: u32 = 0x81a1c1;
const FROST3: u32 = 0x5e81ac;
const RED: u32 = 0xbf616a;
const ORANGE: u32 = 0xd08770;
const YELLOW: u32 = 0xebcb8b;
const GREEN: u32 = 0xa3be8c;
const PURPLE: u32 = 0xb48ead;

// Light inks: the Nord accents deepened to read as text on the light card and chrome. Pastel
// Nord accents are too pale for text on a light surface, and shading them only turns them grey.
const INK_RED: u32 = 0xb0424e;
const INK_ORANGE: u32 = 0xb3583a;
const INK_YELLOW: u32 = 0x8f6a12;
const INK_GREEN: u32 = 0x4f7a3a;
const INK_PURPLE: u32 = 0x8f5a8a;
const INK_TEAL: u32 = 0x2f7d7b;
const INK_CYAN: u32 = 0x1f7891;
const INK_BLUE: u32 = 0x46689a;
const INK_DEEP_BLUE: u32 = 0x3b5b8c;

/// Roles: what each palette colour (or a formula of it) is used for. Theme fields read these only.
struct Palette {
    /// Everything around the cards: gaps, title bar, status bar, tab bar, side docks.
    chrome: u32,
    /// Editor and terminal cards, the far end of the lightness scale from the text (darker than
    /// the chrome when dark, lighter when light); the active tab joins its card.
    card: u32,
    popup: u32,
    border: u32,
    /// Hover fill of filled elements, painted at half opacity.
    hover: u32,
    highlight: u32,
    selected: u32,
    /// Line numbers and guides, the weakest step of the text ladder.
    faint: u32,
    selection: u32,
    /// Occurrences of the symbol under the cursor and the matching bracket: weaker than the
    /// selection, stronger than the active line.
    symbol: u32,
    /// Opacity of the search match fills over the card. Text over them stays readable without
    /// the runtime contrast fix recolouring it.
    search_match: f32,
    search_active_match: f32,
    // The text ladder, strongest first: each step is clearly weaker than the one before, in
    // both appearances (measured in APCA, see the tests).
    text: u32,
    /// Code and terminal text, one step softer than `text`.
    text_soft: u32,
    text_muted: u32,
    /// Comments and punctuation.
    text_dim: u32,
    /// Inlay hints and predictions.
    code_hint: u32,
    red: u32,
    orange: u32,
    yellow: u32,
    green: u32,
    purple: u32,
    teal: u32,
    cyan: u32,
    blue: u32,
    deep_blue: u32,
    ignored: u32,
    word_added: u32,
    word_deleted: u32,
    ansi_black: u32,
    ansi_bright_black: u32,
    ansi_white: u32,
    ansi_bright_white: u32,
    /// Terminal bright / dim variants of an ANSI colour, as `shade` percentages. Bright moves
    /// away from the card and dim towards it, so the signs flip with the appearance.
    bright: i32,
    dim: i32,
}

const DARK_CHROME: u32 = shade(DARK0, -30);
const DARK_CARD: u32 = shade(DARK_CHROME, -33);
const DARK: Palette = Palette {
    chrome: DARK_CHROME,
    card: DARK_CARD,
    popup: shade(DARK1, -30),
    border: DARK1,
    hover: DARK1,
    highlight: DARK2,
    selected: DARK3,
    faint: DARK3,
    selection: shade(DARK3, 50),
    symbol: mix(DARK2, DARK_CARD, 65),
    search_match: 0.3,
    search_active_match: 0.6,
    text: LIGHT2,
    text_soft: LIGHT0,
    text_muted: shade(LIGHT0, -20),
    text_dim: shade(LIGHT0, -30),
    code_hint: shade(DARK3, 20),
    red: RED,
    orange: ORANGE,
    yellow: YELLOW,
    green: GREEN,
    purple: PURPLE,
    teal: FROST0,
    cyan: FROST1,
    blue: FROST2,
    deep_blue: FROST3,
    ignored: shade(YELLOW, -40),
    word_added: shade(GREEN, -50),
    word_deleted: shade(RED, -50),
    ansi_black: DARK1,
    ansi_bright_black: DARK3,
    ansi_white: LIGHT1,
    ansi_bright_white: LIGHT2,
    bright: 20,
    dim: -30,
};

const LIGHT_CARD: u32 = shade(LIGHT2, 5);
const LIGHT_HIGHLIGHT: u32 = shade(LIGHT0, -10);
const LIGHT: Palette = Palette {
    chrome: LIGHT1,
    card: LIGHT_CARD,
    popup: shade(LIGHT2, 10),
    border: LIGHT0,
    hover: LIGHT_HIGHLIGHT,
    highlight: LIGHT_HIGHLIGHT,
    selected: shade(LIGHT0, -18),
    faint: mix(DARK3, LIGHT_CARD, 35),
    selection: FROST2,
    symbol: mix(LIGHT_HIGHLIGHT, LIGHT_CARD, 65),
    search_match: 0.67,
    search_active_match: 0.4,
    text: DARK0,
    text_soft: DARK1,
    text_muted: DARK3,
    // The dim steps fade towards the card rather than lighten, which would turn them blue.
    text_dim: mix(DARK3, LIGHT_CARD, 65),
    code_hint: mix(DARK3, LIGHT_CARD, 45),
    red: INK_RED,
    orange: INK_ORANGE,
    yellow: INK_YELLOW,
    green: INK_GREEN,
    purple: INK_PURPLE,
    teal: INK_TEAL,
    cyan: INK_CYAN,
    blue: INK_BLUE,
    deep_blue: INK_DEEP_BLUE,
    ignored: shade(YELLOW, -40),
    word_added: mix(GREEN, LIGHT_CARD, 55),
    word_deleted: mix(RED, LIGHT_CARD, 40),
    // White stays readable on the light card instead of being literal white, and bright white,
    // which programs use for emphasis, is the stronger of the two.
    ansi_black: DARK1,
    ansi_bright_black: DARK3,
    ansi_white: shade(DARK3, 35),
    ansi_bright_white: DARK2,
    bright: -20,
    dim: 35,
};

/// Search matches sit behind text, so they keep the pastel Nord accents in both appearances.
const SEARCH_MATCH: u32 = YELLOW;
const SEARCH_ACTIVE_MATCH: u32 = RED;

fn theme_colors(p: &Palette) -> ThemeColors {
    let transparent = transparent_black();
    let scrollbar = color(p.highlight).alpha(0.5);
    let scrollbar_hover = color(p.highlight).alpha(0.67);
    let indent_guide = color(p.text_soft).alpha(0.1);
    let indent_guide_active = color(p.text_soft).alpha(0.18);
    ThemeColors {
        border: color(p.border),
        border_variant: color(p.border),
        border_focused: color(p.border),
        border_selected: color(p.border),
        border_transparent: transparent,
        border_disabled: color(p.faint),
        elevated_surface_background: color(p.popup),
        surface_background: color(p.chrome),
        background: color(p.chrome),
        element_background: color(p.border),
        element_hover: color(p.hover).alpha(0.5),
        element_active: color(p.border),
        element_selected: color(p.border),
        element_selection_background: color(p.selection).alpha(0.5),
        element_disabled: color(p.border),
        ghost_element_background: transparent,
        ghost_element_hover: color(p.highlight),
        ghost_element_active: color(p.highlight),
        ghost_element_selected: color(p.selected),
        ghost_element_disabled: color(p.border),
        text: color(p.text),
        text_muted: color(p.text_muted),
        text_placeholder: color(p.text_muted),
        text_disabled: color(p.text_muted),
        text_accent: color(p.cyan),
        icon: color(p.text),
        icon_muted: color(p.text_muted),
        icon_disabled: color(p.text_muted),
        icon_placeholder: color(p.border),
        icon_accent: color(p.cyan),
        status_bar_background: color(p.chrome),
        title_bar_background: color(p.chrome),
        title_bar_inactive_background: color(p.chrome),
        toolbar_background: color(p.card),
        tab_bar_background: color(p.chrome),
        tab_inactive_background: color(p.chrome),
        tab_active_background: color(p.card),
        search_match_background: color(SEARCH_MATCH).alpha(p.search_match),
        search_active_match_background: color(SEARCH_ACTIVE_MATCH).alpha(p.search_active_match),
        panel_background: color(p.chrome),
        panel_focused_border: color(p.cyan),
        panel_indent_guide: indent_guide,
        panel_indent_guide_hover: indent_guide_active,
        panel_indent_guide_active: indent_guide_active,
        panel_overlay_background: color(p.chrome),
        // `element_hover` flattened onto the chrome: the overlay must stay opaque.
        panel_overlay_hover: color(mix(p.hover, p.chrome, 50)),
        pane_focused_border: color(p.cyan),
        pane_group_border: color(p.border),
        scrollbar_thumb_background: scrollbar,
        scrollbar_thumb_hover_background: scrollbar_hover,
        scrollbar_thumb_active_background: scrollbar_hover,
        scrollbar_thumb_border: transparent,
        scrollbar_track_background: color(p.chrome),
        scrollbar_track_border: transparent,
        minimap_thumb_background: scrollbar,
        minimap_thumb_hover_background: scrollbar_hover,
        minimap_thumb_active_background: scrollbar_hover,
        minimap_thumb_border: transparent,
        editor_foreground: color(p.text),
        editor_code_lens_foreground: None,
        editor_background: color(p.card),
        editor_gutter_background: color(p.card),
        editor_subheader_background: color(mix(p.border, p.chrome, 60)),
        editor_active_line_background: color(p.border).alpha(0.5),
        editor_highlighted_line_background: color(p.highlight),
        editor_line_number: color(p.faint),
        editor_active_line_number: color(p.text),
        editor_hover_line_number: color(p.text_soft),
        editor_invisible: color(p.text_dim),
        editor_wrap_guide: color(p.faint),
        editor_active_wrap_guide: color(p.faint),
        editor_indent_guide: indent_guide,
        editor_indent_guide_active: indent_guide_active,
        editor_document_highlight_read_background: color(p.symbol),
        editor_document_highlight_write_background: color(p.symbol),
        editor_document_highlight_bracket_background: color(p.symbol),
        editor_diff_hunk_added_background: color(p.green).alpha(0.12),
        editor_diff_hunk_added_hollow_background: color(p.green).alpha(0.06),
        editor_diff_hunk_added_hollow_border: color(p.green).alpha(0.36),
        editor_diff_hunk_deleted_background: color(p.red).alpha(0.12),
        editor_diff_hunk_deleted_hollow_background: color(p.red).alpha(0.06),
        editor_diff_hunk_deleted_hollow_border: color(p.red).alpha(0.36),
        terminal_background: color(p.card),
        terminal_foreground: color(p.text_soft),
        terminal_bright_foreground: color(p.text),
        terminal_dim_foreground: color(shade(p.text_soft, p.dim)),
        terminal_ansi_background: color(p.card),
        terminal_ansi_black: color(p.ansi_black),
        terminal_ansi_bright_black: color(p.ansi_bright_black),
        terminal_ansi_dim_black: color(shade(p.ansi_black, p.dim)),
        terminal_ansi_red: color(p.red),
        terminal_ansi_bright_red: color(shade(p.red, p.bright)),
        terminal_ansi_dim_red: color(shade(p.red, p.dim)),
        terminal_ansi_green: color(p.green),
        terminal_ansi_bright_green: color(shade(p.green, p.bright)),
        terminal_ansi_dim_green: color(shade(p.green, p.dim)),
        terminal_ansi_yellow: color(p.yellow),
        terminal_ansi_bright_yellow: color(shade(p.yellow, p.bright)),
        terminal_ansi_dim_yellow: color(shade(p.yellow, p.dim)),
        terminal_ansi_blue: color(p.blue),
        terminal_ansi_bright_blue: color(shade(p.blue, p.bright)),
        terminal_ansi_dim_blue: color(shade(p.blue, p.dim)),
        terminal_ansi_magenta: color(p.purple),
        terminal_ansi_bright_magenta: color(shade(p.purple, p.bright)),
        terminal_ansi_dim_magenta: color(shade(p.purple, p.dim)),
        terminal_ansi_cyan: color(p.cyan),
        terminal_ansi_bright_cyan: color(p.teal),
        terminal_ansi_dim_cyan: color(shade(p.cyan, p.dim)),
        terminal_ansi_white: color(p.ansi_white),
        terminal_ansi_bright_white: color(p.ansi_bright_white),
        terminal_ansi_dim_white: color(shade(p.ansi_white, p.dim)),
        link_text_hover: color(p.teal),
        version_control_added: color(p.green),
        version_control_deleted: color(p.red),
        version_control_modified: color(p.yellow),
        version_control_renamed: color(p.orange),
        version_control_conflict: color(p.red),
        version_control_ignored: color(p.ignored),
        version_control_word_added: color(p.word_added),
        version_control_word_deleted: color(p.word_deleted),
        version_control_conflict_marker_ours: color(p.green).alpha(0.125),
        version_control_conflict_marker_theirs: color(p.red).alpha(0.125),
    }
}

fn status_colors(p: &Palette) -> StatusColors {
    StatusColors {
        conflict: color(p.red),
        created: color(p.green),
        created_background: color(p.green).alpha(0.25),
        deleted: color(p.red),
        deleted_background: color(p.red).alpha(0.25),
        error: color(p.red),
        error_background: color(p.red).alpha(0.2),
        error_border: color(p.red),
        hidden: color(p.text_dim),
        hint: color(p.text_dim),
        hint_background: color(p.popup),
        hint_border: color(p.border),
        ignored: color(p.ignored),
        ignored_background: color(p.popup),
        ignored_border: color(p.border),
        info: color(p.text_dim),
        info_background: color(p.popup),
        info_border: color(p.border),
        modified: color(p.yellow),
        renamed: color(p.orange),
        success: color(p.green),
        success_background: color(p.popup),
        success_border: color(p.border),
        unreachable: color(p.text_muted),
        warning: color(p.yellow),
        warning_background: color(p.yellow).alpha(0.2),
        warning_border: color(p.yellow),
    }
}

/// The local cursor and selection.
fn cursor_colors(p: &Palette) -> CursorColors {
    CursorColors { cursor: color(p.text), selection: color(p.selection).alpha(0.5) }
}

fn accent_colors(p: &Palette) -> AccentColors {
    AccentColors([p.blue, p.cyan, p.red, p.orange, p.yellow, p.green, p.purple].map(color))
}

fn system_colors() -> SystemColors {
    SystemColors {
        transparent: transparent_black(),
        mac_os_traffic_light_red: color(0xec6b5f),
        mac_os_traffic_light_yellow: color(0xf4bf4e),
        mac_os_traffic_light_green: color(0x62c454),
    }
}

fn syntax_theme(p: &Palette) -> SyntaxTheme {
    let italic = |hex| HighlightStyle { font_style: Some(FontStyle::Italic), ..fg(hex) };
    let bold = |hex| HighlightStyle { font_weight: Some(FontWeight::BOLD), ..fg(hex) };
    SyntaxTheme::new(
        [
            ("attribute", fg(p.teal)),
            ("boolean", fg(p.blue)),
            ("comment", fg(p.text_dim)),
            ("comment.doc", fg(p.text_dim)),
            ("comment.todo", bold(p.yellow)),
            ("constant", fg(p.text_soft)),
            ("constructor", fg(p.orange)),
            ("definition.function", fg(p.orange)),
            ("embedded", fg(p.orange)),
            ("emphasis", italic(p.blue)),
            ("emphasis.strong", bold(p.blue)),
            ("enum", fg(p.orange)),
            ("function", fg(p.text_soft)),
            ("function.method", fg(p.text_soft)),
            ("hint", fg(p.code_hint)),
            ("keyword", fg(p.blue)),
            ("label", fg(p.blue)),
            ("lifetime", fg(p.teal)),
            ("link_text", HighlightStyle { font_style: Some(FontStyle::Normal), ..fg(p.blue) }),
            ("link_uri", fg(p.blue)),
            ("number", fg(p.purple)),
            ("operator", fg(p.text_dim)),
            ("predictive", fg(p.code_hint)),
            ("preproc", fg(p.deep_blue)),
            ("primary", fg(p.text)),
            ("property", fg(p.text_soft)),
            ("punctuation", fg(p.text_dim)),
            ("punctuation.bracket", fg(p.text_dim)),
            ("punctuation.delimiter", fg(p.text_dim)),
            ("punctuation.list_marker", fg(p.text_dim)),
            ("punctuation.special", fg(p.text_dim)),
            ("string", fg(p.green)),
            ("string.escape", fg(p.yellow)),
            ("string.regex", fg(p.green)),
            ("string.special", fg(p.green)),
            ("tag", fg(p.blue)),
            ("text.literal", fg(p.green)),
            ("title", bold(p.cyan)),
            ("type", fg(p.cyan)),
            ("type.builtin", fg(p.cyan)),
            ("type.super", fg(p.cyan)),
            ("variable", fg(p.text_soft)),
            ("variable.parameter", fg(p.text_soft)),
            ("variable.special", fg(p.blue)),
            ("variant", fg(p.purple)),
        ]
        .map(|(name, style): (&str, HighlightStyle)| (name.to_owned(), style)),
    )
}

fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn fg(hex: u32) -> HighlightStyle {
    HighlightStyle { color: Some(color(hex)), ..HighlightStyle::default() }
}

/// Scales each RGB channel by `100 + percent` percent, clamped: negative darkens, positive lightens.
const fn shade(hex: u32, percent: i32) -> u32 {
    let scale = (100 + percent) as u32;
    let mut out = 0;
    let mut shift = 0;
    while shift < 24 {
        let channel = ((hex >> shift) & 0xff) * scale / 100;
        out |= (if channel > 0xff { 0xff } else { channel }) << shift;
        shift += 8;
    }
    out
}

/// `top` at `percent` opacity flattened onto `bottom`, for surfaces that must stay opaque.
const fn mix(top: u32, bottom: u32, percent: u32) -> u32 {
    let mut out = 0;
    let mut shift = 0;
    while shift < 24 {
        let channel = (((top >> shift) & 0xff) * percent + ((bottom >> shift) & 0xff) * (100 - percent) + 50) / 100;
        out |= channel << shift;
        shift += 8;
    }
    out
}

#[cfg(test)]
mod tests {
    use gpui::Rgba;

    use super::*;
    use crate::contrast::MINIMUM_CONTRAST;
    use crate::contrast::apca_contrast;

    /// APCA contrast of opaque `text` on `background`, the metric the runtime contrast fix uses.
    fn contrast(text: u32, background: u32) -> f32 {
        apca_contrast(color(text), color(background)).abs()
    }

    #[test]
    fn color_parses_rgb_hex() {
        for hex in [0x88c0d0, 0x3b4252, 0x000000] {
            let rgba = Rgba::from(color(hex));
            let channels = [rgba.r, rgba.g, rgba.b].map(|channel| (channel * 255.).round() as u32);
            assert_eq!((channels[0] << 16) | (channels[1] << 8) | channels[2], hex);
            assert_eq!(rgba.a, 1.);
        }
    }

    #[test]
    fn fg_sets_only_color() {
        assert_eq!(fg(RED), HighlightStyle { color: Some(color(RED)), ..HighlightStyle::default() });
    }

    #[test]
    fn shade_scales_and_clamps_channels() {
        assert_eq!(shade(DARK0, 0), DARK0);
        assert_eq!(shade(DARK0, -30), 0x20242c);
        assert_eq!(shade(RED, 20), 0xe5747f);
        assert_eq!(shade(YELLOW, 20), 0xfff3a6);
        assert_eq!(shade(LIGHT0, -100), 0x000000);
    }

    #[test]
    fn mix_flattens_top_onto_bottom() {
        assert_eq!(mix(DARK1, DARK0, 100), DARK1);
        assert_eq!(mix(DARK1, DARK0, 0), DARK0);
        assert_eq!(mix(DARK.border, DARK.chrome, 50), 0x2e333f);
        assert_eq!(mix(DARK.border, DARK.chrome, 60), 0x303643);
    }

    #[test]
    fn dark_roles_derive_from_palette() {
        assert_eq!(DARK.chrome, 0x20242c);
        assert_eq!(DARK.card, 0x15181d);
        assert_eq!(DARK.popup, 0x292e39);
        assert_eq!(DARK.selection, 0x72819f);
        assert_eq!(DARK.symbol, 0x333a47);
        assert_eq!(DARK.text_muted, 0xacb1ba);
        assert_eq!(DARK.text_dim, 0x979ba3);
        assert_eq!(DARK.code_hint, 0x5b677f);
        assert_eq!(DARK.ignored, 0x8d7953);
    }

    #[test]
    fn light_roles_derive_from_palette() {
        assert_eq!(LIGHT.chrome, 0xe5e9f0);
        assert_eq!(LIGHT.card, 0xf7faff);
        assert_eq!(LIGHT.popup, 0xffffff);
        assert_eq!(LIGHT.highlight, 0xc2c7d1);
        assert_eq!(LIGHT.selected, 0xb1b6bf);
        assert_eq!(LIGHT.faint, 0xbbc1cb);
        assert_eq!(LIGHT.symbol, 0xd5d9e1);
        assert_eq!(LIGHT.text_dim, 0x888f9e);
        assert_eq!(LIGHT.code_hint, 0xaab0bc);
        assert_eq!(LIGHT.word_added, 0xc9d9c0);
    }

    #[test]
    fn text_ladder_steps_down_in_both_appearances() {
        for p in [&DARK, &LIGHT] {
            let ladder = [p.text, p.text_soft, p.text_muted, p.text_dim, p.code_hint, p.faint];
            for (stronger, weaker) in ladder.iter().zip(&ladder[1..]) {
                assert!(contrast(*stronger, p.card) > contrast(*weaker, p.card), "{stronger:06x} then {weaker:06x}");
            }
            // Down to comments, text needs no runtime recolouring on the card.
            assert!(contrast(p.text_dim, p.card) >= MINIMUM_CONTRAST, "{:06x}", p.text_dim);
            // Comments are clearly weaker than code, hints clearly weaker than comments.
            assert!(contrast(p.text_dim, p.card) < contrast(p.text_soft, p.card) * 0.67);
            assert!(contrast(p.code_hint, p.card) < contrast(p.text_dim, p.card) * 0.75);
        }
    }

    #[test]
    fn light_inks_are_readable_on_card_and_chrome() {
        // The inks exist for this; the dark theme keeps the Nord accents as they are.
        let p = &LIGHT;
        for ink in [p.red, p.orange, p.yellow, p.green, p.purple, p.teal, p.cyan, p.blue, p.deep_blue] {
            assert!(contrast(ink, p.card) >= 65., "{ink:06x} on {:06x}", p.card);
            assert!(contrast(ink, p.chrome) >= 55., "{ink:06x} on {:06x}", p.chrome);
        }
    }

    #[test]
    fn fills_behind_code_keep_it_readable_and_apart() {
        for p in [&DARK, &LIGHT] {
            let search = mix(SEARCH_MATCH, p.card, (p.search_match * 100.) as u32);
            let active = mix(SEARCH_ACTIVE_MATCH, p.card, (p.search_active_match * 100.) as u32);
            let selection = mix(p.selection, p.card, 50);
            let active_line = mix(p.border, p.card, 50);
            for fill in [search, active, selection, p.symbol, p.word_added, p.word_deleted] {
                assert!(contrast(p.text_soft, fill) >= MINIMUM_CONTRAST, "{:06x} on {fill:06x}", p.text_soft);
            }
            // Lightness order from the card: active line, symbol, selection.
            let distance = |fill: u32| (color(fill).l - color(p.card).l).abs();
            assert!(distance(active_line) < distance(p.symbol));
            assert!(distance(p.symbol) < distance(selection));
        }
    }

    #[test]
    fn light_hover_shows_on_the_chrome() {
        for p in [&DARK, &LIGHT] {
            let hover = mix(p.hover, p.chrome, 50);
            assert!((color(hover).l - color(p.chrome).l).abs() > 0.03, "{hover:06x} on {:06x}", p.chrome);
        }
    }

    #[test]
    fn themes_share_the_floating_chrome() {
        for (appearance, name) in [(Appearance::Dark, "Nord"), (Appearance::Light, "Nord Light")] {
            let theme = theme_for(appearance);
            assert_eq!(theme.name.as_ref(), name);
            assert_eq!(theme.appearance, appearance);

            let colors = theme.colors();
            assert_eq!(colors.background, colors.panel_background);
            assert_eq!(colors.title_bar_background, colors.panel_background);
            assert_eq!(colors.status_bar_background, colors.panel_background);
            assert_eq!(colors.tab_bar_background, colors.panel_background);
            assert_eq!(colors.tab_active_background, colors.editor_background);
            assert_eq!(colors.terminal_ansi_background, colors.terminal_background);
            assert!(colors.panel_overlay_hover.is_opaque());
            assert_eq!(colors.element_selection_background, theme.cursor_colors().selection);
            // The card is further from the text than the chrome is.
            assert_eq!(colors.editor_background.l > colors.panel_background.l, appearance.is_light());
            assert_eq!(colors.text.l < colors.editor_background.l, appearance.is_light());
        }
    }

    #[test]
    fn terminal_bright_and_dim_bracket_normal() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let colors = theme_for(appearance).colors();
            let card = colors.terminal_background.l;
            for (dim, normal, bright) in [
                (colors.terminal_ansi_dim_red, colors.terminal_ansi_red, colors.terminal_ansi_bright_red),
                (colors.terminal_ansi_dim_green, colors.terminal_ansi_green, colors.terminal_ansi_bright_green),
                (colors.terminal_ansi_dim_blue, colors.terminal_ansi_blue, colors.terminal_ansi_bright_blue),
            ] {
                // Dim fades towards the card, bright stands out from it.
                assert!((dim.l - card).abs() < (normal.l - card).abs());
                assert!((normal.l - card).abs() < (bright.l - card).abs());
            }
        }
    }

    #[test]
    fn status_colors_use_nord_surfaces() {
        let status = status_colors(&DARK);
        assert_eq!(status.ignored_background, status.hint_background);
        assert_eq!(status.ignored_border, status.hint_border);
        assert_eq!(status.error_background, color(RED).alpha(0.2));
        assert_eq!(status_colors(&LIGHT).error_background, color(INK_RED).alpha(0.2));
    }

    #[test]
    fn syntax_theme_resolves_capture_names() {
        let syntax = theme_for(Appearance::Dark).syntax();
        assert_eq!(syntax.style_for_name("keyword").and_then(|style| style.color), Some(color(FROST2)));
        assert_eq!(syntax.style_for_name("title").and_then(|style| style.font_weight), Some(FontWeight::BOLD));
        assert!(syntax.highlight_id("string.escape").is_some());
        assert_eq!(syntax.style_for_name("lifetime").and_then(|style| style.color), Some(color(FROST0)));
        assert_eq!(syntax.style_for_name("variant").and_then(|style| style.color), Some(color(PURPLE)));
        assert!(syntax.style_for_name("missing").is_none());

        let light = theme_for(Appearance::Light).syntax();
        assert_eq!(light.style_for_name("keyword").and_then(|style| style.color), Some(color(INK_BLUE)));
    }

    #[test]
    fn syntax_highlight_ids_match_across_appearances() {
        // Grammar highlight maps are resolved once, so an id must mean the same capture in both.
        let (dark, light) = (syntax_theme(&DARK), syntax_theme(&LIGHT));
        let mut id = 0_usize;
        while let Some(name) = dark.get_capture_name(id) {
            assert_eq!(light.get_capture_name(id), Some(name));
            id += 1;
        }
        assert!(id > 0 && light.get_capture_name(id).is_none());
    }

    #[test]
    fn theme_for_is_one_static_instance_per_appearance() {
        assert!(std::ptr::eq(theme_for(Appearance::Dark), theme_for(Appearance::Dark)));
        assert!(std::ptr::eq(theme_for(Appearance::Light), theme_for(Appearance::Light)));
        assert_ne!(theme_for(Appearance::Dark), theme_for(Appearance::Light));
        assert_eq!(theme_for(Appearance::Dark).syntax(), &syntax_theme(&DARK));
    }

    // The only test that touches the active appearance, which is process-wide.
    #[test]
    fn set_appearance_switches_the_active_theme() {
        assert!(std::ptr::eq(theme(), theme_for(Appearance::Dark)));
        assert!(!set_appearance(Appearance::Dark));
        assert!(set_appearance(Appearance::Light));
        assert!(std::ptr::eq(theme(), theme_for(Appearance::Light)));
        assert!(!set_appearance(Appearance::Light));
        assert!(set_appearance(Appearance::Dark));
        assert!(std::ptr::eq(theme(), theme_for(Appearance::Dark)));
    }

    #[test]
    fn accent_colors_cycle() {
        let accents = theme_for(Appearance::Dark).accents();
        assert_eq!(accents.color_for_index(0), color(FROST2));
        assert_eq!(accents.color_for_index(accents.0.len() as u32), accents.color_for_index(0));
    }
}
