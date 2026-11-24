//! Shows the dark and light theme side by side from hardcoded state, to tune the palette by
//! eye: `cargo run -p theme_test`, or `cargo run -p theme_test -- <file.png>` to save a
//! screenshot and exit.

mod preview;
mod sample;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::App;
use gpui::Application;
use gpui::Bounds;
use gpui::Context;
use gpui::Render;
use gpui::Window;
use gpui::WindowBounds;
use gpui::WindowOptions;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::size;
use theme::Appearance;

struct Preview;

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().size_full().child(preview::pane(Appearance::Dark)).child(preview::pane(Appearance::Light))
    }
}

#[expect(clippy::print_stderr, reason = "a dev tool reporting to its terminal")]
fn main() {
    let screenshot = std::env::args_os().nth(1).map(PathBuf::from);
    Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false))).run(move |cx: &mut App| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1560.), px(860.)), cx))),
            ..WindowOptions::default()
        };
        let window = cx.open_window(options, |window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.quit();
                true
            });
            cx.new(|_| Preview)
        });
        let Ok(window) = window else {
            cx.quit();
            return;
        };
        let Some(path) = screenshot else {
            cx.activate(true);
            return;
        };
        cx.spawn(async move |cx| {
            // Let the first frame paint before capturing it.
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let image = window.update(cx, |_, window, cx| {
                window.draw_if_dirty(cx);
                window.capture_image()
            });
            if let Ok(Ok(image)) = image
                && let Err(error) = image.save(&path)
            {
                eprintln!("failed to write {}: {error}", path.display());
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
