use std::cmp;
use std::ops::Range;

use gpui::Entity;
use gpui::Rgba;
use gpui::Task;
use gpui::WeakEntity;
use language::Buffer;
use language::range_from_lsp;
use multi_buffer::Anchor;
use multi_buffer::MultiBufferSnapshot;
use project::DocumentColor;
use project::InlayId;
use project::Project;
use rustc_hash::FxHashMap;
use text::Bias;
use text::BufferId;
use ui::Context;
use ui::Window;
use util::post_inc;

use crate::Editor;
use crate::InlaySplice;
use crate::LSP_REQUEST_DEBOUNCE_TIMEOUT;
use crate::LspDataKind;
use crate::inlays::Inlay;

#[derive(Debug)]
pub(super) struct LspColorData {
    buffer_colors: FxHashMap<BufferId, BufferColors>,
}

#[derive(Debug, Default)]
struct BufferColors {
    colors: Vec<(Range<Anchor>, DocumentColor, InlayId)>,
    inlay_colors: FxHashMap<InlayId, usize>,
}

impl LspColorData {
    pub fn new() -> LspColorData {
        LspColorData { buffer_colors: FxHashMap::default() }
    }

    fn set_colors(&mut self, buffer_id: BufferId, colors: Vec<(Range<Anchor>, DocumentColor, InlayId)>) -> bool {
        let buffer_colors = self.buffer_colors.entry(buffer_id).or_default();
        if buffer_colors.colors == colors {
            return false;
        }

        buffer_colors.inlay_colors = colors.iter().enumerate().map(|(i, (_, _, id))| (*id, i)).collect();
        buffer_colors.colors = colors;
        true
    }

    /// Drops colors of `buffer_id` that are no longer inside its excerpts (all of them if
    /// `buffers_removed`), returning the inlay ids to remove.
    fn remove_invalid_colors(
        &mut self,
        buffer_id: BufferId,
        buffers_removed: bool,
        snapshot: &MultiBufferSnapshot,
    ) -> Vec<InlayId> {
        let Some(buffer_colors) = self.buffer_colors.get(&buffer_id) else {
            return Vec::new();
        };
        let (valid_colors, invalid_colors) =
            buffer_colors.colors.iter().cloned().partition::<Vec<_>, _>(|(range, _, _)| {
                !buffers_removed
                    && [range.start, range.end].iter().all(|anchor| {
                        anchor
                            .raw_text_anchor()
                            .is_none_or(|text_anchor| snapshot.anchor_in_excerpt(text_anchor).is_some())
                    })
            });
        if invalid_colors.is_empty() {
            return Vec::new();
        }
        if valid_colors.is_empty() {
            self.buffer_colors.remove(&buffer_id);
        } else {
            self.set_colors(buffer_id, valid_colors);
        }
        invalid_colors.into_iter().map(|(_, _, inlay_id)| inlay_id).collect()
    }
}

impl Editor {
    pub(super) fn refresh_document_colors(&mut self, buffer_id: Option<BufferId>, _: &Window, cx: &mut Context<Self>) {
        if !self.lsp_data_enabled() {
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if self.colors.is_none() {
            return;
        }

        let project = project.downgrade();
        let buffers_to_query = self.lsp_data_buffers(buffer_id, cx);
        self.spawn_lsp_data_tasks(LspDataKind::DocumentColors, buffer_id, buffers_to_query, cx, |buffer, cx| {
            Editor::fetch_document_colors(project.clone(), buffer, cx)
        });
    }

    pub(super) fn remove_invalid_document_colors(
        &mut self,
        buffer_ids: &[BufferId],
        buffers_removed: bool,
        snapshot: &MultiBufferSnapshot,
        cx: &mut Context<Self>,
    ) {
        let Some(colors) = &mut self.colors else {
            return;
        };
        let invalid_inlays = buffer_ids
            .iter()
            .flat_map(|buffer_id| colors.remove_invalid_colors(*buffer_id, buffers_removed, snapshot))
            .collect::<Vec<_>>();
        if invalid_inlays.is_empty() {
            return;
        }
        self.splice_inlays(&invalid_inlays, Vec::new(), cx);
        cx.notify();
    }

    fn fetch_document_colors(project: WeakEntity<Project>, buffer: Entity<Buffer>, cx: &Context<Self>) -> Task<()> {
        cx.spawn(async move |editor, cx| {
            cx.background_executor().timer(LSP_REQUEST_DEBOUNCE_TIMEOUT).await;

            let Some((buffer_snapshot, colors_task)) = project
                .update(cx, |project, cx| {
                    project.lsp_store().update(cx, |lsp_store, cx| {
                        let buffer_snapshot = buffer.read(cx).snapshot();
                        Some((buffer_snapshot, lsp_store.document_colors(buffer, cx)?))
                    })
                })
                .ok()
                .flatten()
            else {
                return;
            };

            let colors = colors_task.await;
            let Some(multi_buffer_snapshot) = editor.update(cx, |editor, cx| editor.buffer.read(cx).snapshot(cx)).ok()
            else {
                return;
            };

            let mut new_editor_colors: FxHashMap<BufferId, Vec<(Range<Anchor>, DocumentColor)>> = FxHashMap::default();
            match colors {
                Ok(colors) => {
                    if colors.colors.is_empty() {
                        new_editor_colors.entry(buffer_snapshot.buffer_id()).or_default().clear();
                    } else {
                        for color in colors.colors {
                            let color_range = range_from_lsp(color.lsp_range);
                            let color_start = color_range.start;
                            let color_end = color_range.end;

                            let Some(range) = multi_buffer_snapshot.buffer_anchor_range_to_anchor_range(
                                buffer_snapshot.anchor_range_outside(
                                    buffer_snapshot.clip_point_utf16(color_start, Bias::Left)
                                        ..buffer_snapshot.clip_point_utf16(color_end, Bias::Right),
                                ),
                            ) else {
                                continue;
                            };

                            let new_buffer_colors = new_editor_colors.entry(buffer_snapshot.buffer_id()).or_default();

                            let (Ok(i) | Err(i)) = new_buffer_colors.binary_search_by(|(probe, _)| {
                                probe
                                    .start
                                    .cmp(&range.start, &multi_buffer_snapshot)
                                    .then_with(|| probe.end.cmp(&range.end, &multi_buffer_snapshot))
                            });
                            new_buffer_colors.insert(i, (range, color));
                        }
                    }
                }
                Err(e) => log::error!("Failed to retrieve document colors: {e}"),
            }

            editor
                .update(cx, |editor, cx| {
                    let mut colors_splice = InlaySplice::default();
                    let Some(colors) = &mut editor.colors else {
                        return;
                    };
                    let mut updated = false;
                    for (buffer_id, new_buffer_colors) in new_editor_colors {
                        let mut new_buffer_color_inlays = Vec::with_capacity(new_buffer_colors.len());
                        let mut existing_buffer_colors =
                            colors.buffer_colors.entry(buffer_id).or_default().colors.iter().peekable();
                        for (new_range, new_color) in new_buffer_colors {
                            let rgba_color = Rgba {
                                r: new_color.color.red,
                                g: new_color.color.green,
                                b: new_color.color.blue,
                                a: new_color.color.alpha,
                            };

                            loop {
                                match existing_buffer_colors.peek() {
                                    Some((existing_range, existing_color, existing_inlay_id)) => {
                                        match existing_range
                                            .start
                                            .cmp(&new_range.start, &multi_buffer_snapshot)
                                            .then_with(|| {
                                                existing_range.end.cmp(&new_range.end, &multi_buffer_snapshot)
                                            }) {
                                            cmp::Ordering::Less => {
                                                colors_splice.to_remove.push(*existing_inlay_id);
                                                existing_buffer_colors.next();
                                                continue;
                                            }
                                            cmp::Ordering::Equal => {
                                                if existing_color == &new_color {
                                                    new_buffer_color_inlays.push((
                                                        new_range,
                                                        new_color,
                                                        *existing_inlay_id,
                                                    ));
                                                } else {
                                                    colors_splice.to_remove.push(*existing_inlay_id);

                                                    let inlay = Inlay::color(
                                                        post_inc(&mut editor.next_color_inlay_id),
                                                        new_range.start,
                                                        rgba_color,
                                                    );
                                                    let inlay_id = inlay.id;
                                                    colors_splice.to_insert.push(inlay);
                                                    new_buffer_color_inlays.push((new_range, new_color, inlay_id));
                                                }
                                                existing_buffer_colors.next();
                                                break;
                                            }
                                            cmp::Ordering::Greater => {
                                                let inlay = Inlay::color(
                                                    post_inc(&mut editor.next_color_inlay_id),
                                                    new_range.start,
                                                    rgba_color,
                                                );
                                                let inlay_id = inlay.id;
                                                colors_splice.to_insert.push(inlay);
                                                new_buffer_color_inlays.push((new_range, new_color, inlay_id));
                                                break;
                                            }
                                        }
                                    }
                                    None => {
                                        let inlay = Inlay::color(
                                            post_inc(&mut editor.next_color_inlay_id),
                                            new_range.start,
                                            rgba_color,
                                        );
                                        let inlay_id = inlay.id;
                                        colors_splice.to_insert.push(inlay);
                                        new_buffer_color_inlays.push((new_range, new_color, inlay_id));
                                        break;
                                    }
                                }
                            }
                        }

                        if existing_buffer_colors.peek().is_some() {
                            colors_splice.to_remove.extend(existing_buffer_colors.map(|(_, _, id)| *id));
                        }
                        updated |= colors.set_colors(buffer_id, new_buffer_color_inlays);
                    }

                    if !colors_splice.is_empty() {
                        editor.splice_inlays(&colors_splice.to_remove, colors_splice.to_insert, cx);
                        updated = true;
                    }

                    if updated {
                        cx.notify();
                    }
                })
                .ok();
        })
    }
}
