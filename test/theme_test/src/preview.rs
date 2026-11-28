//! Paints one appearance of the theme from hardcoded state. Colours come from the theme only,
//! so a palette edit shows here as it would in the app.

use gpui::Div;
use gpui::HighlightStyle;
use gpui::Hsla;
use gpui::SharedString;
use gpui::StyledText;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use theme::Appearance;
use theme::Theme;
use theme::built_in::BUFFER_LINE_HEIGHT;
use theme::built_in::UNNECESSARY_CODE_FADE;
use theme::built_in::theme_for;
use theme::contrast::MINIMUM_CONTRAST;
use theme::contrast::apca_contrast;
use theme::contrast::ensure_minimum_contrast;
use ui::font::BUFFER_FONT_SIZE;
use ui::font::buffer_font;
use ui::font::ui_font;

use crate::sample::CODE;
use crate::sample::Fill;
use crate::sample::Ink;
use crate::sample::Row;
use crate::sample::Span;

const UI_SIZE: f32 = 13.;
const ROW_HEIGHT: f32 = 24.;

/// The whole preview of one appearance, on its chrome.
pub(crate) fn pane(appearance: Appearance) -> Div {
    let theme = theme_for(appearance);
    let c = theme.colors();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(px(10.))
        .p(px(12.))
        .bg(c.background)
        .text_color(c.text)
        .text_size(px(UI_SIZE))
        .font_family(ui_font().family.clone())
        .child(tabs(theme))
        .child(editor(theme))
        .child(div().flex().gap(px(10.)).child(panel(theme)).child(popup(theme)).child(ladder(theme)))
        .child(terminal(theme))
}

fn tab(label: impl Into<SharedString>, text: Hsla, bg: Hsla) -> Div {
    div().px(px(12.)).py(px(4.)).rounded(px(6.)).bg(bg).text_color(text).child(label.into())
}

fn tabs(theme: &Theme) -> Div {
    let c = theme.colors();
    div()
        .flex()
        .gap(px(4.))
        .items_center()
        .bg(c.tab_bar_background)
        .child(tab(theme.name.clone(), c.text, c.tab_active_background))
        .child(tab("inactive tab", c.text_muted, c.tab_inactive_background))
        .child(tab("hovered tab", c.text_muted, c.ghost_element_hover))
        .child(tab("filled hover", c.text, c.tab_inactive_background.blend(c.element_hover)))
        .child(tab("selected", c.text, c.ghost_element_selected))
        .child(tab("button", c.text, c.element_background))
        .child(tab("accent", c.text_accent, c.tab_inactive_background))
        .child(tab("disabled", c.text_disabled, c.element_disabled))
}

fn editor(theme: &Theme) -> Div {
    let c = theme.colors();
    let line_height = px((BUFFER_FONT_SIZE.as_f32() * BUFFER_LINE_HEIGHT).round());
    let lines = CODE.iter().enumerate().map(|(index, (row, spans))| {
        let row_bg = row_background(theme, *row);
        let marker = match row {
            Row::Added => c.version_control_added,
            Row::Deleted => c.version_control_deleted,
            // lint: expect(wildcard_match) only some variants matter here
            _ => c.editor_gutter_background,
        };
        let number = if *row == Row::Active { c.editor_active_line_number } else { c.editor_line_number };
        let (text, highlights) = line(theme, row_bg, spans);
        div()
            .flex()
            .h(line_height)
            .items_center()
            .child(div().w(px(3.)).h_full().bg(marker))
            .child(div().w(px(36.)).pr(px(10.)).text_right().text_color(number).child((index + 1).to_string()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .bg(row_bg)
                    .whitespace_nowrap()
                    .child(StyledText::new(text).with_highlights(highlights)),
            )
    });
    div()
        .flex()
        .rounded(px(8.))
        .overflow_hidden()
        .bg(c.editor_background)
        .text_color(c.editor_foreground)
        .text_size(BUFFER_FONT_SIZE)
        .font_family(buffer_font().family.clone())
        .child(div().flex().flex_col().flex_1().py(px(6.)).children(lines))
        .child(
            div()
                .w(px(10.))
                .py(px(6.))
                .child(div().w(px(6.)).h(px(90.)).rounded(px(3.)).bg(c.scrollbar_thumb_background)),
        )
}

/// The opaque colour behind a line's text.
fn row_background(theme: &Theme, row: Row) -> Hsla {
    let c = theme.colors();
    c.editor_background.blend(match row {
        Row::Plain => return c.editor_background,
        Row::Active => c.editor_active_line_background,
        Row::Highlighted => c.editor_highlighted_line_background,
        Row::Added => c.editor_diff_hunk_added_background,
        Row::Deleted => c.editor_diff_hunk_deleted_background,
    })
}

const fn fill_color(theme: &Theme, fill: Fill) -> Option<Hsla> {
    let c = theme.colors();
    Some(match fill {
        Fill::None => return None,
        Fill::Selection => theme.cursor_colors().selection,
        Fill::Symbol => c.editor_document_highlight_read_background,
        Fill::Search => c.search_match_background,
        Fill::ActiveSearch => c.search_active_match_background,
        Fill::WordAdded => c.version_control_word_added,
        Fill::WordDeleted => c.version_control_word_deleted,
    })
}

/// The style of one span, with the editor's contrast fix applied to text over a fill.
fn span_style(theme: &Theme, row_bg: Hsla, span: &Span) -> HighlightStyle {
    let syntax = |capture: &str| {
        theme.syntax().style_for_name(capture).unwrap_or_else(|| HighlightStyle {
            color: Some(theme.colors().editor_foreground),
            ..HighlightStyle::default()
        })
    };
    let mut style = match span.ink {
        Ink::Syntax(capture) => syntax(capture),
        Ink::Unnecessary(capture) => HighlightStyle { fade_out: Some(UNNECESSARY_CODE_FADE), ..syntax(capture) },
        Ink::Error => HighlightStyle { color: Some(theme.status().error), ..HighlightStyle::default() },
        Ink::Warning => HighlightStyle { color: Some(theme.status().warning), ..HighlightStyle::default() },
    };
    if let Some(fill) = fill_color(theme, span.fill) {
        style.background_color = Some(fill);
        style.color = style.color.map(|color| ensure_minimum_contrast(color, row_bg.blend(fill), MINIMUM_CONTRAST));
    }
    style
}

fn line(theme: &Theme, row_bg: Hsla, spans: &[Span]) -> (SharedString, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let mut text = String::new();
    let mut highlights = Vec::with_capacity(spans.len());
    for span in spans {
        let start = text.len();
        text.push_str(span.text);
        highlights.push((start..text.len(), span_style(theme, row_bg, span)));
    }
    (text.into(), highlights)
}

fn row(label: impl Into<SharedString>, text: Hsla, bg: Hsla) -> Div {
    div()
        .flex()
        .h(px(ROW_HEIGHT))
        .px(px(8.))
        .items_center()
        .rounded(px(4.))
        .bg(bg)
        .text_color(text)
        .whitespace_nowrap()
        .child(label.into())
}

/// A side panel on the chrome: list states and git status colours.
fn panel(theme: &Theme) -> Div {
    let c = theme.colors();
    let none = c.panel_background;
    div()
        .flex()
        .flex_col()
        .flex_1()
        .child(row("crates/theme", c.text, none))
        .child(row("  built_in.rs  selected", c.text, c.ghost_element_selected))
        .child(row("  contrast.rs  hover", c.text, c.ghost_element_hover))
        .child(row("  theme.rs  filled hover", c.text, none.blend(c.element_hover)))
        .child(row("  styles.rs  modified", c.version_control_modified, none))
        .child(row("  preview.rs  added", c.version_control_added, none))
        .child(row("  legacy.rs  deleted", c.version_control_deleted, none))
        .child(row("  moved.rs  renamed", c.version_control_renamed, none))
        .child(row("  merge.rs  conflict", c.version_control_conflict, none))
        .child(row("  target  ignored", c.version_control_ignored, none))
        .child(row("  muted label", c.text_muted, none))
        .child(row("  placeholder", c.text_placeholder, none))
}

/// A popup on the elevated surface: menu states and diagnostics.
fn popup(theme: &Theme) -> Div {
    let c = theme.colors();
    let s = theme.status();
    let surface = c.elevated_surface_background;
    div()
        .flex()
        .flex_col()
        .flex_1()
        .p(px(4.))
        .rounded(px(8.))
        .border_1()
        .border_color(c.border)
        .bg(surface)
        .child(row("completion", c.text, surface))
        .child(row("completion  selected", c.text, c.ghost_element_selected))
        .child(row("completion  hover", c.text, c.ghost_element_hover))
        .child(row("detail  muted", c.text_muted, surface))
        .child(row("link  accent", c.text_accent, surface))
        .child(row("error: mismatched types", s.error, surface.blend(s.error_background)))
        .child(row("warning: unused variable", s.warning, surface.blend(s.warning_background)))
        .child(row("info: 3 references", s.info, s.info_background))
        .child(row("hint: add a type", s.hint, s.hint_background))
        .child(row("success: tests passed", s.success, s.success_background))
        .child(row("created", s.created, surface.blend(s.created_background)))
        .child(row("deleted", s.deleted, surface.blend(s.deleted_background)))
}

/// The contrast ladder: each role in its own colour on the card, with its APCA contrast.
fn ladder(theme: &Theme) -> Div {
    let c = theme.colors();
    let card = c.editor_background;
    let syntax = |capture: &str| theme.syntax().style_for_name(capture).and_then(|style| style.color).unwrap_or(c.text);
    let roles = [
        ("text", c.text),
        ("code", syntax("variable")),
        ("muted", c.text_muted),
        ("keyword", syntax("keyword")),
        ("type", syntax("type")),
        ("string", syntax("string")),
        ("number", syntax("number")),
        ("function def", syntax("definition.function")),
        ("comment", syntax("comment")),
        ("inlay hint", syntax("hint")),
        ("line number", c.editor_line_number),
        ("error", theme.status().error),
    ];
    div()
        .flex()
        .flex_col()
        .flex_1()
        .p(px(4.))
        .rounded(px(8.))
        .bg(card)
        .font_family(buffer_font().family.clone())
        .children(
            roles.map(|(name, color)| {
                row(format!("{name:<13}Lc {:>3.0}", apca_contrast(color, card).abs()), color, card)
            }),
        )
}

/// The terminal card: every ANSI colour dim, normal and bright, as the terminal shows it.
fn terminal(theme: &Theme) -> Div {
    let c = theme.colors();
    let bg = c.terminal_background;
    let rows = [
        (
            "dim   ",
            [
                c.terminal_ansi_dim_black,
                c.terminal_ansi_dim_red,
                c.terminal_ansi_dim_green,
                c.terminal_ansi_dim_yellow,
                c.terminal_ansi_dim_blue,
                c.terminal_ansi_dim_magenta,
                c.terminal_ansi_dim_cyan,
                c.terminal_ansi_dim_white,
                c.terminal_dim_foreground,
            ],
        ),
        (
            "normal",
            [
                c.terminal_ansi_black,
                c.terminal_ansi_red,
                c.terminal_ansi_green,
                c.terminal_ansi_yellow,
                c.terminal_ansi_blue,
                c.terminal_ansi_magenta,
                c.terminal_ansi_cyan,
                c.terminal_ansi_white,
                c.terminal_foreground,
            ],
        ),
        (
            "bright",
            [
                c.terminal_ansi_bright_black,
                c.terminal_ansi_bright_red,
                c.terminal_ansi_bright_green,
                c.terminal_ansi_bright_yellow,
                c.terminal_ansi_bright_blue,
                c.terminal_ansi_bright_magenta,
                c.terminal_ansi_bright_cyan,
                c.terminal_ansi_bright_white,
                c.terminal_bright_foreground,
            ],
        ),
    ];
    let names = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "foreground"];
    div()
        .flex()
        .flex_col()
        .p(px(8.))
        .rounded(px(8.))
        .bg(bg)
        .text_size(BUFFER_FONT_SIZE)
        .font_family(buffer_font().family.clone())
        .children(rows.map(|(label, colors)| {
            div().flex().gap(px(14.)).child(div().text_color(c.terminal_foreground).child(label)).children(
                colors
                    .iter()
                    .zip(names)
                    .map(|(color, name)| {
                        div().text_color(ensure_minimum_contrast(*color, bg, MINIMUM_CONTRAST)).child(name)
                    })
                    .collect::<Vec<_>>(),
            )
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_over_a_fill_keeps_minimum_contrast() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = theme_for(appearance);
            for (kind, spans) in CODE {
                let row_bg = row_background(theme, *kind);
                for span in *spans {
                    let style = span_style(theme, row_bg, span);
                    assert_eq!(style.background_color, fill_color(theme, span.fill));
                    if let (Some(color), Some(fill)) = (style.color, style.background_color) {
                        let contrast = apca_contrast(color, row_bg.blend(fill)).abs();
                        assert!(contrast >= MINIMUM_CONTRAST - 1., "{:?} {contrast}", span.text);
                    }
                }
            }
        }
    }

    #[test]
    fn span_ink_resolves_colour_and_fade() {
        let theme = theme_for(Appearance::Dark);
        let card = theme.colors().editor_background;
        let style = |ink| span_style(theme, card, &Span { text: "x", ink, fill: Fill::None });
        assert_eq!(style(Ink::Syntax("")).color, Some(theme.colors().editor_foreground));
        assert_eq!(style(Ink::Syntax("keyword")).color, theme.syntax().style_for_name("keyword").and_then(|s| s.color));
        assert_eq!(style(Ink::Unnecessary("variable")).fade_out, Some(UNNECESSARY_CODE_FADE));
        assert_eq!(style(Ink::Error).color, Some(theme.status().error));
        assert_eq!(style(Ink::Warning).color, Some(theme.status().warning));
    }

    #[test]
    fn line_highlights_cover_the_text_in_order() {
        let theme = theme_for(Appearance::Light);
        let (kind, spans) = &CODE[7];
        let (text, highlights) = line(theme, row_background(theme, *kind), spans);
        assert_eq!(highlights.len(), spans.len());
        assert_eq!(highlights.first().map(|(range, _)| range.start), Some(0));
        assert_eq!(highlights.last().map(|(range, _)| range.end), Some(text.len()));
        assert!(highlights.iter().zip(&highlights[1..]).all(|(a, b)| a.0.end == b.0.start));
    }

    #[test]
    fn row_background_is_opaque_and_plain_is_the_card() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = theme_for(appearance);
            assert_eq!(row_background(theme, Row::Plain), theme.colors().editor_background);
            for row in [Row::Active, Row::Highlighted, Row::Added, Row::Deleted] {
                let bg = row_background(theme, row);
                assert!(bg.is_opaque());
                assert_ne!(bg, theme.colors().editor_background);
            }
        }
    }
}
