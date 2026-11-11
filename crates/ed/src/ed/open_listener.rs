use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use editor::Editor;
use fs::Fs;
use futures::channel::mpsc;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use git_ui::multi_diff_view::MultiDiffView;
use git_ui_core::file_diff_view::FileDiffView;
use gpui::AsyncApp;
use gpui::Global;
use gpui::WindowHandle;
use util::ResultExt;
use util::paths::PathWithPosition;
use workspace::AppState;
use workspace::OpenResult;
use workspace::Workspace;
use workspace::item::ItemHandle;

#[derive(Default, Debug)]
pub struct OpenRequest {
    pub open_paths: Vec<String>,
    pub diff_paths: Vec<[String; 2]>,
    pub diff_all: bool,
}

impl OpenRequest {
    pub fn parse(request: RawOpenRequest) -> Result<Self> {
        let mut this = Self { diff_paths: request.diff_paths, diff_all: request.diff_all, ..Default::default() };

        for url in request.urls {
            if let Some(file) = url.strip_prefix("file://") {
                this.parse_file_path(file)
            } else {
                log::error!("unhandled url: {}", url);
            }
        }

        Ok(this)
    }

    fn parse_file_path(&mut self, file: &str) {
        if let Some(decoded) = urlencoding::decode(file).log_err() {
            self.open_paths.push(decoded.into_owned())
        }
    }
}

#[derive(Clone)]
pub struct OpenListener(UnboundedSender<RawOpenRequest>);

#[derive(Default)]
pub struct RawOpenRequest {
    pub urls: Vec<String>,
    pub diff_paths: Vec<[String; 2]>,
    pub diff_all: bool,
}

impl Global for OpenListener {}

impl OpenListener {
    pub fn new() -> (Self, UnboundedReceiver<RawOpenRequest>) {
        let (tx, rx) = mpsc::unbounded();
        (OpenListener(tx), rx)
    }

    pub fn open(&self, request: RawOpenRequest) {
        self.0.unbounded_send(request).context("no listener for open requests").log_err();
    }
}

pub async fn open_paths_with_positions(
    path_positions: &[PathWithPosition],
    diff_paths: &[[String; 2]],
    diff_all: bool,
    app_state: Arc<AppState>,
    open_options: workspace::OpenOptions,
    cx: &mut AsyncApp,
) -> Result<(WindowHandle<Workspace>, Vec<Option<Result<Box<dyn ItemHandle>>>>)> {
    let paths = path_positions.iter().map(|path_with_position| path_with_position.path.clone()).collect::<Vec<_>>();

    let OpenResult { window, opened_items: mut items, .. } =
        cx.update(|cx| workspace::open_paths(&paths, app_state.clone(), open_options, cx)).await?;

    if diff_all && !diff_paths.is_empty() {
        let mut diff_pairs = Vec::with_capacity(diff_paths.len());
        for diff_pair in diff_paths {
            let parsed = derive_paths_with_position(app_state.fs.as_ref(), diff_pair).await;
            let (Some(old_parsed), Some(new_parsed)) = (parsed.first(), parsed.get(1)) else {
                continue;
            };
            diff_pairs
                .push([old_parsed.path.to_string_lossy().into_owned(), new_parsed.path.to_string_lossy().into_owned()]);
        }
        if let Ok(diff_view) =
            window.update(cx, |workspace, window, cx| MultiDiffView::open(diff_pairs, workspace, window, cx))
            && let Some(diff_view) = diff_view.await.log_err()
        {
            items.push(Some(Ok(Box::new(diff_view))));
        }
    } else {
        let workspace_weak = window.entity(cx)?.downgrade();
        let canonicalize = async |parsed: &PathWithPosition| {
            app_state
                .fs
                .canonicalize(&parsed.path)
                .await
                .with_context(|| format!("opening --diff path {:?}", parsed.path))
        };
        for diff_pair in diff_paths {
            let parsed = derive_paths_with_position(app_state.fs.as_ref(), diff_pair).await;
            let (Some(old_parsed), Some(new_parsed)) = (parsed.first(), parsed.get(1)) else {
                continue;
            };
            let (old_path, new_path) = match futures::join!(canonicalize(old_parsed), canonicalize(new_parsed)) {
                (Ok(old), Ok(new)) => (old, new),
                (old, new) => {
                    for result in [old, new] {
                        if let Err(err) = result {
                            items.push(Some(Err(err)));
                        }
                    }
                    continue;
                }
            };
            let target_position = new_parsed.row.map(|row| {
                language::Point::new(row.saturating_sub(1), new_parsed.column.unwrap_or(0).saturating_sub(1))
            });
            if let Ok(diff_view) = window.update(cx, |_, window, cx| {
                FileDiffView::open(old_path, new_path, target_position, workspace_weak.clone(), window, cx)
            }) && let Some(diff_view) = diff_view.await.log_err()
            {
                items.push(Some(Ok(Box::new(diff_view))))
            }
        }
    }

    for (item, path) in items.iter_mut().zip(&paths) {
        if let Some(Err(error)) = item {
            *error = anyhow!("error opening {path:?}: {error:#}");
        }
    }

    let items_for_navigation =
        items.iter().map(|item| item.as_ref().and_then(|r| r.as_ref().ok()).cloned()).collect::<Vec<_>>();
    navigate_to_positions(&window, items_for_navigation, path_positions, cx);

    Ok((window, items))
}

fn navigate_to_positions(
    window: &WindowHandle<Workspace>,
    items: impl IntoIterator<Item = Option<Box<dyn ItemHandle>>>,
    positions: &[PathWithPosition],
    cx: &mut AsyncApp,
) {
    for (item, path) in items.into_iter().zip(positions) {
        let Some(item) = item else {
            continue;
        };
        let Some(row) = path.row else {
            continue;
        };
        if let Some(active_editor) = item.downcast::<Editor>() {
            window
                .update(cx, |_, window, cx| {
                    active_editor.update(cx, |editor, cx| {
                        let row = row.saturating_sub(1);
                        let col = path.column.unwrap_or(0).saturating_sub(1);
                        let Some(buffer) = editor.buffer().read(cx).as_singleton() else {
                            return;
                        };
                        let buffer_snapshot = buffer.read(cx).snapshot();
                        let point = buffer_snapshot.point_from_external_input(row, col);
                        editor.go_to_singleton_buffer_point(point, window, cx);
                    });
                })
                .ok();
        }
    }
}

pub async fn derive_paths_with_position(
    fs: &dyn Fs,
    path_strings: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<PathWithPosition> {
    let path_strings: Vec<_> = path_strings.into_iter().collect();
    let mut result = Vec::with_capacity(path_strings.len());
    for path_str in path_strings {
        let original_path = Path::new(path_str.as_ref());
        let mut parsed = PathWithPosition::parse_str(path_str.as_ref());

        // If the unparsed path string actually points to an existing file or directory, use it
        // instead of parsing out the line/col number. This matters for paths whose final
        // component looks like a position suffix, e.g. a folder named `Test (3)` would
        // otherwise be parsed as `Test ` at row 3.
        // Colon : is not valid in NTFS file names, so skip this logic if colon on windows.
        let has_colon = original_path.file_name().and_then(|name| name.to_str()).is_none_or(|name| name.contains(':'));

        if (!has_colon || !false)
            && parsed.row.is_some()
            && parsed.path != original_path
            && (fs.is_file(original_path).await || fs.is_dir(original_path).await)
        {
            parsed = PathWithPosition::from_path(original_path.to_path_buf());
        }

        if let Ok(canonicalized) = fs.canonicalize(&parsed.path).await {
            parsed.path = canonicalized;
        }

        result.push(parsed);
    }
    result
}
