use std::mem;
use std::path::Path;
use std::sync::Arc;

use file_icons::FileIcons;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::IntoElement;
use gpui::ParentElement as _;
use gpui::Render;
use gpui::RenderImage;
use gpui::Styled as _;
use gpui::Subscription;
use gpui::Task;
use gpui::Window;
use gpui::div;
use gpui::img;
use language::Buffer;
use language::BufferEvent;
use multi_buffer::MultiBuffer;
use project::ProjectPath;
use ui::prelude::ActiveTheme as _;
use ui::prelude::FluentBuilder as _;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::InteractiveElement as _;
use ui::prelude::SharedString;
use ui::prelude::StyledImage as _;
use ui::prelude::h_flex;
use ui::prelude::v_flex;
use workspace::Pane;
use workspace::Workspace;
use workspace::item::Item;
use workspace::item::ItemEvent;
use workspace::notifications::NotifyResultExt as _;

pub struct SvgPreviewView {
    focus_handle: FocusHandle,
    buffer: Option<Entity<Buffer>>,
    current_svg: Option<Result<Arc<RenderImage>, SharedString>>,
    _refresh: Task<()>,
    _buffer_subscription: Option<Subscription>,
}

impl SvgPreviewView {
    pub fn new(active_buffer: Entity<MultiBuffer>, window: &mut Window, cx: &mut Context<Workspace>) -> Entity<Self> {
        cx.new(|cx| {
            let buffer = active_buffer.read_with(cx, |buffer, _cx| buffer.as_singleton());

            let subscription = buffer.as_ref().map(|buffer| Self::create_buffer_subscription(buffer, window, cx));

            let mut this = Self {
                focus_handle: cx.focus_handle(),
                buffer,
                current_svg: None,
                _buffer_subscription: subscription,
                _refresh: Task::ready(()),
            };
            this.render_image(window, cx);

            this
        })
    }

    fn render_image(&mut self, window: &Window, cx: &Context<Self>) {
        let Some(buffer) = self.buffer.as_ref() else {
            return;
        };
        const SCALE_FACTOR: f32 = 1.0;

        let renderer = cx.svg_renderer();
        let content = buffer.read(cx).snapshot();
        let background_task =
            cx.background_spawn(async move { renderer.render_single_frame(content.text().as_bytes(), SCALE_FACTOR) });

        self._refresh = cx.spawn_in(window, async move |this, cx| {
            let result = background_task.await;

            this.update_in(cx, |view, window, cx| {
                let current = result.map_err(|e| e.to_string().into());
                view.set_current(Some(current), window, cx);
            })
            .ok();
        });
    }

    pub fn open_for_project_path(
        project_path: ProjectPath,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let open_buffer = workspace.project().update(cx, |project, cx| project.open_buffer(project_path, cx));

        cx.spawn_in(window, async move |workspace, cx| {
            let Some(buffer) = open_buffer.await.notify_workspace_async_err(workspace.clone(), cx) else {
                return;
            };

            workspace
                .update_in(cx, |workspace, window, cx| {
                    let buffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx));
                    let pane = workspace.shown_pane().clone();
                    Self::open_preview_in_pane(buffer, pane, window, cx);
                })
                .ok();
        })
        .detach();
    }

    fn set_current(
        &mut self,
        image: Option<Result<Arc<RenderImage>, SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Ok(image)) = mem::replace(&mut self.current_svg, image) {
            window.drop_image(image).ok();
        }
        cx.notify();
    }

    fn find_existing_preview_item_idx(pane: &Pane, buffer: &Entity<MultiBuffer>, cx: &App) -> Option<usize> {
        let buffer_id = buffer.read(cx).as_singleton()?.entity_id();
        let view = pane
            .items_of_type::<SvgPreviewView>()
            .find(|view| view.read(cx).buffer.as_ref().is_some_and(|buffer| buffer.entity_id() == buffer_id))?;
        pane.index_for_item(&view)
    }

    fn create_buffer_subscription(buffer: &Entity<Buffer>, window: &Window, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe_in(buffer, window, move |this, _buffer, event: &BufferEvent, window, cx| match event {
            BufferEvent::Edited | BufferEvent::Saved => {
                this.render_image(window, cx);
            }
            _ => {}
        })
    }

    pub fn is_svg_path(path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    }

    pub fn is_svg_file(buffer: &Entity<MultiBuffer>, cx: &App) -> bool {
        buffer
            .read(cx)
            .as_singleton()
            .and_then(|buffer| buffer.read(cx).file())
            .is_some_and(|file| Self::is_svg_path(Path::new(file.file_name(cx))))
    }

    pub fn open_preview_in_pane(
        buffer: Entity<MultiBuffer>,
        pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::activate_or_add_preview(buffer, pane, true, window, cx);
    }

    pub fn open_preview_to_the_side_of_pane(
        workspace: &mut Workspace,
        buffer: Entity<MultiBuffer>,
        origin_pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let target_pane = workspace.adjacent_pane_of(&origin_pane, window, cx);
        Self::activate_or_add_preview(buffer, target_pane, false, window, cx);
    }

    fn activate_or_add_preview(
        buffer: Entity<MultiBuffer>,
        pane: Entity<Pane>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let existing_view_idx = Self::find_existing_preview_item_idx(pane.read(cx), &buffer, cx);
        if let Some(existing_view_idx) = existing_view_idx {
            pane.update(cx, |pane, cx| {
                pane.activate_item(existing_view_idx, focus, focus, window, cx);
            });
        } else {
            let view = Self::new(buffer, window, cx);
            pane.update(cx, |pane, cx| pane.add_item(Box::new(view), focus, focus, None, window, cx));
        }
        cx.notify();
    }
}

impl Render for SvgPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("SvgPreview")
            .key_context("SvgPreview")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .flex()
            .justify_center()
            .items_center()
            .map(|this| match self.current_svg.clone() {
                Some(Ok(image)) => this.child(img(image).max_w_full().max_h_full().with_fallback(|| {
                    h_flex()
                        .p_4()
                        .gap_2()
                        .child(Icon::new(IconName::Warning))
                        .child("Failed to load SVG image")
                        .into_any_element()
                })),
                Some(Err(e)) => this.child(div().p_4().child(e).into_any_element()),
                None => this.child(div().p_4().child("No SVG file selected")),
            })
    }
}

impl Focusable for SvgPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for SvgPreviewView {}

impl Item for SvgPreviewView {
    type Event = ();

    fn tab_icon(&self, _window: &Window, cx: &App) -> Option<Icon> {
        self.buffer
            .as_ref()
            .and_then(|buffer| buffer.read(cx).file())
            .and_then(|file| FileIcons::get_icon(file.path().as_std_path(), cx))
            .map(Icon::from_path)
            .or_else(|| Some(Icon::new(IconName::Image)))
    }

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.buffer
            .as_ref()
            .and_then(|svg_path| svg_path.read(cx).file())
            .map_or_else(|| "SVG Preview".into(), |name| format!("Preview {}", name.file_name(cx)).into())
    }

    fn to_item_events(_event: &Self::Event, _f: &mut dyn FnMut(ItemEvent)) {}
}
