use std::collections::hash_map;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::io::Cursor;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use futures::channel::oneshot;
use gpui::App;
use gpui::AppContext;
use gpui::AsyncApp;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::Img;
use gpui::Subscription;
use gpui::Task;
use gpui::WeakEntity;
use image::ExtendedColorType;
use image::GenericImageView;
pub use image::ImageFormat as RasterImageFormat;
use image::ImageReader;
use language::DiskState;
use language::File;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use util::ResultExt;
use util::rel_path::RelPath;
use worktree::LoadedBinaryFile;
use worktree::PathChange;
use worktree::Worktree;

use crate::Project;
use crate::ProjectEntryId;
use crate::ProjectItem;
use crate::ProjectPath;
use crate::worktree_store::WorktreeStore;
use crate::worktree_store::WorktreeStoreEvent;

#[derive(Clone, Copy, Debug, Hash, PartialEq, PartialOrd, Ord, Eq)]
pub struct ImageId(NonZeroU64);
impl Display for ImageId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<NonZeroU64> for ImageId {
    fn from(id: NonZeroU64) -> Self {
        ImageId(id)
    }
}

#[derive(Debug)]
pub enum ImageItemEvent {
    ReloadNeeded,
    Reloaded,
    FileHandleChanged,
    MetadataUpdated,
}

impl EventEmitter<ImageItemEvent> for ImageItem {}

pub enum ImageStoreEvent {
    ImageAdded(Entity<ImageItem>),
}

impl EventEmitter<ImageStoreEvent> for ImageStore {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Raster(RasterImageFormat),
    Svg,
}

#[derive(Debug, Clone, Copy)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub colors: Option<ImageColorInfo>,
    pub format: ImageFormat,
}

#[derive(Debug, Clone, Copy)]
pub struct ImageColorInfo {
    pub channels: u8,
    pub bits_per_channel: u8,
}

impl ImageColorInfo {
    pub fn from_color_type(color_type: impl Into<ExtendedColorType>) -> Option<Self> {
        let (channels, bits_per_channel) = match color_type.into() {
            ExtendedColorType::L8 => (1, 8),
            ExtendedColorType::L16 => (1, 16),
            ExtendedColorType::La8 => (2, 8),
            ExtendedColorType::La16 => (2, 16),
            ExtendedColorType::Rgb8 => (3, 8),
            ExtendedColorType::Rgb16 => (3, 16),
            ExtendedColorType::Rgba8 => (4, 8),
            ExtendedColorType::Rgba16 => (4, 16),
            ExtendedColorType::A8 => (1, 8),
            ExtendedColorType::Bgr8 => (3, 8),
            ExtendedColorType::Bgra8 => (4, 8),
            ExtendedColorType::Cmyk8 => (4, 8),
            _ => return None,
        };

        Some(Self { channels, bits_per_channel })
    }

    pub const fn bits_per_pixel(&self) -> u8 {
        self.channels * self.bits_per_channel
    }
}

pub struct ImageItem {
    pub id: ImageId,
    pub file: Arc<worktree::File>,
    pub image: Arc<gpui::Image>,
    reload_task: Option<Task<()>>,
    pub image_metadata: Option<ImageMetadata>,
}

impl ImageItem {
    pub fn compute_metadata_from_bytes(image_bytes: &[u8]) -> Result<ImageMetadata> {
        let image_format = match image::guess_format(image_bytes) {
            Ok(format) => format,
            Err(_) => {
                let svg = usvg::Tree::from_data(image_bytes, &usvg::Options::default())?;
                let size = svg.size().to_int_size();
                return Ok(ImageMetadata {
                    width: size.width(),
                    height: size.height(),
                    file_size: image_bytes.len() as u64,
                    format: ImageFormat::Svg,
                    colors: None,
                });
            }
        };

        let mut image_reader = ImageReader::new(Cursor::new(image_bytes));
        image_reader.set_format(image_format);
        let image = image_reader.decode()?;

        let (width, height) = image.dimensions();

        Ok(ImageMetadata {
            width,
            height,
            file_size: image_bytes.len() as u64,
            format: ImageFormat::Raster(image_format),
            colors: ImageColorInfo::from_color_type(image.color()),
        })
    }

    pub async fn load_image_metadata(
        image: Entity<ImageItem>,
        project: Entity<Project>,
        cx: &mut AsyncApp,
    ) -> Result<ImageMetadata> {
        let (fs, image_path) = cx.update(|cx| {
            let fs = project.read(cx).fs().clone();
            let image_path = image.read(cx).abs_path(cx).context("absolutizing image file path")?;
            anyhow::Ok((fs, image_path))
        })?;

        let image_bytes = fs.load_bytes(&image_path).await?;
        Self::compute_metadata_from_bytes(&image_bytes)
    }

    pub fn project_path(&self, cx: &App) -> ProjectPath {
        ProjectPath { worktree_id: self.file.worktree_id(cx), path: self.file.path().clone() }
    }

    pub fn abs_path(&self, cx: &App) -> Option<PathBuf> {
        Some(self.file.as_local()?.abs_path(cx))
    }

    fn file_updated(&mut self, new_file: Arc<worktree::File>, cx: &mut Context<Self>) {
        let mut file_changed = false;

        let old_file = &self.file;
        if new_file.path() != old_file.path() {
            file_changed = true;
        }

        let old_state = old_file.disk_state();
        let new_state = new_file.disk_state();
        if old_state != new_state {
            file_changed = true;
            if matches!(new_state, DiskState::Present { .. }) {
                cx.emit(ImageItemEvent::ReloadNeeded)
            }
        }

        self.file = new_file;
        if file_changed {
            cx.emit(ImageItemEvent::FileHandleChanged);
            cx.notify();
        }
    }

    fn reload(&mut self, cx: &Context<Self>) -> Option<oneshot::Receiver<()>> {
        let local_file = self.file.as_local()?;
        let (tx, rx) = oneshot::channel();

        let content = local_file.load_bytes(cx);
        self.reload_task = Some(cx.spawn(async move |this, cx| {
            if let Some(image) =
                content.await.context("Failed to load image content").and_then(create_gpui_image).log_err()
            {
                this.update(cx, |this, cx| {
                    this.image = image;
                    cx.emit(ImageItemEvent::Reloaded);
                })
                .log_err();
            }
            _ = tx.send(());
        }));
        Some(rx)
    }
}

pub fn is_image_file(project: &Entity<Project>, path: &ProjectPath, cx: &App) -> bool {
    let ext = (|| {
        let worktree_abs_path = project.read(cx).worktree_for_id(path.worktree_id, cx)?.read(cx).abs_path();
        path.path.extension().or_else(|| worktree_abs_path.extension()?.to_str()).map(str::to_lowercase)
    })();

    match ext {
        Some(ext) => Img::extensions().contains(&ext.as_str()) && !ext.contains("svg"),
        None => false,
    }
}

impl ProjectItem for ImageItem {
    fn try_open(
        project: &Entity<Project>,
        path: &ProjectPath,
        cx: &mut App,
    ) -> Option<Task<anyhow::Result<Entity<Self>>>> {
        if is_image_file(project, path, cx) {
            Some(cx.spawn({
                let path = path.clone();
                let project = project.clone();
                async move |cx| project.update(cx, |project, cx| project.open_image(path, cx)).await
            }))
        } else {
            None
        }
    }

    fn entry_id(&self, _: &App) -> Option<ProjectEntryId> {
        self.file.entry_id
    }

    fn project_path(&self, cx: &App) -> Option<ProjectPath> {
        Some(self.project_path(cx))
    }

    fn is_dirty(&self) -> bool {
        false
    }
}

struct LocalImageStore {
    local_image_ids_by_path: FxHashMap<ProjectPath, ImageId>,
    local_image_ids_by_entry_id: FxHashMap<ProjectEntryId, ImageId>,
    image_store: WeakEntity<ImageStore>,
    _subscription: Subscription,
}

pub struct ImageStore {
    state: Entity<LocalImageStore>,
    opened_images: FxHashMap<ImageId, WeakEntity<ImageItem>>,
    worktree_store: Entity<WorktreeStore>,
    #[allow(clippy::type_complexity)]
    loading_images_by_path:
        FxHashMap<ProjectPath, watch::Receiver<Option<Result<Entity<ImageItem>, Arc<anyhow::Error>>>>>,
}

impl ImageStore {
    pub fn local(worktree_store: Entity<WorktreeStore>, cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        Self {
            state: cx.new(|cx| {
                let subscription = cx.subscribe(&worktree_store, |this: &mut LocalImageStore, _, event, cx| {
                    if let WorktreeStoreEvent::WorktreeAdded(worktree) = event {
                        this.subscribe_to_worktree(worktree, cx);
                    }
                });

                LocalImageStore {
                    local_image_ids_by_path: FxHashMap::default(),
                    local_image_ids_by_entry_id: FxHashMap::default(),
                    image_store: this,
                    _subscription: subscription,
                }
            }),
            opened_images: FxHashMap::default(),
            loading_images_by_path: FxHashMap::default(),
            worktree_store,
        }
    }

    pub fn images(&self) -> impl '_ + Iterator<Item = Entity<ImageItem>> {
        self.opened_images.values().filter_map(|image| image.upgrade())
    }

    pub fn get(&self, image_id: ImageId) -> Option<Entity<ImageItem>> {
        self.opened_images.get(&image_id).and_then(|image| image.upgrade())
    }

    pub fn get_by_path(&self, path: &ProjectPath, cx: &App) -> Option<Entity<ImageItem>> {
        self.images().find(|image| &image.read(cx).project_path(cx) == path)
    }

    pub fn open_image(&mut self, project_path: ProjectPath, cx: &mut Context<Self>) -> Task<Result<Entity<ImageItem>>> {
        let existing_image = self.get_by_path(&project_path, cx);
        if let Some(existing_image) = existing_image {
            return Task::ready(Ok(existing_image));
        }

        let Some(worktree) = self.worktree_store.read(cx).worktree_for_id(project_path.worktree_id, cx) else {
            return Task::ready(Err(anyhow::anyhow!("no such worktree")));
        };

        let loading_watch = match self.loading_images_by_path.entry(project_path.clone()) {
            // If the given path is already being loaded, then wait for that existing
            // task to complete and return the same image.
            hash_map::Entry::Occupied(e) => e.get().clone(),

            // Otherwise, record the fact that this path is now being loaded.
            hash_map::Entry::Vacant(entry) => {
                let (mut tx, rx) = watch::channel(None);
                entry.insert(rx.clone());

                let load_image = LocalImageStore::open_image(&self.state, project_path.path.clone(), worktree, cx);

                cx.spawn(async move |this, cx| {
                    let load_result = load_image.await;
                    let result = this.update(cx, |this, _cx| {
                        // Record the fact that the image is no longer loading.
                        this.loading_images_by_path.remove(&project_path);
                        let image = load_result.map_err(Arc::new)?;
                        Ok(image)
                    })?;
                    tx.send(Some(result)).ok();
                    anyhow::Ok(())
                })
                .detach();
                rx
            }
        };

        cx.background_spawn(async move {
            Self::wait_for_loading_image(loading_watch).await.map_err(|e| anyhow::anyhow!("{e:#}"))
        })
    }

    pub async fn wait_for_loading_image(
        mut receiver: watch::Receiver<Option<Result<Entity<ImageItem>, Arc<anyhow::Error>>>>,
    ) -> Result<Entity<ImageItem>, Arc<anyhow::Error>> {
        loop {
            if let Some(result) = receiver.borrow().as_ref() {
                match result {
                    Ok(image) => return Ok(image.to_owned()),
                    Err(e) => return Err(e.to_owned()),
                }
            }
            if receiver.changed().await.is_err() {
                return Err(Arc::new(anyhow::anyhow!("image loading was dropped")));
            }
        }
    }

    pub fn reload_images(
        &self,
        images: FxHashSet<Entity<ImageItem>>,
        cx: &mut Context<ImageStore>,
    ) -> Task<Result<()>> {
        if images.is_empty() {
            return Task::ready(Ok(()));
        }

        LocalImageStore::reload_images(images, cx)
    }

    fn add_image(&mut self, image: Entity<ImageItem>, cx: &mut Context<ImageStore>) -> Result<()> {
        let image_id = image.read(cx).id;
        self.opened_images.insert(image_id, image.downgrade());
        cx.subscribe(&image, Self::on_image_event).detach();
        cx.emit(ImageStoreEvent::ImageAdded(image));
        Ok(())
    }

    fn on_image_event(&mut self, image: Entity<ImageItem>, event: &ImageItemEvent, cx: &mut Context<Self>) {
        if let ImageItemEvent::FileHandleChanged = event {
            self.state.update(cx, |local, cx| {
                local.image_changed_file(image, cx);
            })
        }
    }
}
impl LocalImageStore {
    fn open_image(
        this: &Entity<Self>,
        path: Arc<RelPath>,
        worktree: Entity<Worktree>,
        cx: &mut Context<ImageStore>,
    ) -> Task<Result<Entity<ImageItem>>> {
        let this = this.clone();

        let load_file = worktree.update(cx, |worktree, cx| worktree.load_binary_file(path.as_ref(), cx));
        cx.spawn(async move |image_store, cx| {
            let LoadedBinaryFile { file, content } = load_file.await?;
            let image = create_gpui_image(content)?;

            let entity = cx.new(|cx| ImageItem {
                id: cx.entity_id().as_non_zero_u64().into(),
                file: file.clone(),
                image,
                image_metadata: None,
                reload_task: None,
            });

            let image_id = cx.read_entity(&entity, |model, _| model.id);

            this.update(cx, |this, cx| {
                image_store.update(cx, |image_store, cx| image_store.add_image(entity.clone(), cx))??;
                this.local_image_ids_by_path
                    .insert(ProjectPath { worktree_id: file.worktree_id(cx), path: file.path.clone() }, image_id);

                if let Some(entry_id) = file.entry_id {
                    this.local_image_ids_by_entry_id.insert(entry_id, image_id);
                }

                anyhow::Ok(())
            })?;

            Ok(entity)
        })
    }

    fn reload_images(images: FxHashSet<Entity<ImageItem>>, cx: &Context<ImageStore>) -> Task<Result<()>> {
        cx.spawn(async move |_, cx| {
            for image in images {
                if let Some(rec) = image.update(cx, |image, cx| image.reload(cx)) {
                    rec.await?
                }
            }
            Ok(())
        })
    }
}
impl LocalImageStore {
    fn subscribe_to_worktree(&self, worktree: &Entity<Worktree>, cx: &mut Context<Self>) {
        cx.subscribe(worktree, |this, worktree, event, cx| {
            if let worktree::Event::UpdatedEntries(changes) = event {
                this.local_worktree_entries_changed(&worktree, changes, cx);
            }
        })
        .detach();
    }

    fn local_worktree_entries_changed(
        &mut self,
        worktree_handle: &Entity<Worktree>,
        changes: &[(Arc<RelPath>, ProjectEntryId, PathChange)],
        cx: &mut Context<Self>,
    ) {
        let snapshot = worktree_handle.read(cx).snapshot();
        for (path, entry_id, _) in changes {
            self.local_worktree_entry_changed(*entry_id, path, worktree_handle, &snapshot, cx);
        }
    }

    fn local_worktree_entry_changed(
        &mut self,
        entry_id: ProjectEntryId,
        path: &Arc<RelPath>,
        worktree: &Entity<worktree::Worktree>,
        snapshot: &worktree::Snapshot,
        cx: &mut Context<Self>,
    ) -> Option<()> {
        let project_path = ProjectPath { worktree_id: snapshot.id(), path: path.clone() };
        let image_id = match self.local_image_ids_by_entry_id.get(&entry_id) {
            Some(&image_id) => image_id,
            None => self.local_image_ids_by_path.get(&project_path).copied()?,
        };

        let image = self
            .image_store
            .update(cx, |image_store, _| {
                if let Some(image) = image_store.get(image_id) {
                    Some(image)
                } else {
                    image_store.opened_images.remove(&image_id);
                    None
                }
            })
            .ok()
            .flatten();
        let image = if let Some(image) = image {
            image
        } else {
            self.local_image_ids_by_path.remove(&project_path);
            self.local_image_ids_by_entry_id.remove(&entry_id);
            return None;
        };

        image.update(cx, |image, cx| {
            let old_file = &image.file;
            if old_file.worktree != *worktree {
                return;
            }

            let snapshot_entry = old_file
                .entry_id
                .and_then(|entry_id| snapshot.entry_for_id(entry_id))
                .or_else(|| snapshot.entry_for_path(old_file.path.as_ref()));

            let new_file = if let Some(entry) = snapshot_entry {
                worktree::File {
                    disk_state: match entry.mtime {
                        Some(mtime) => DiskState::Present { mtime, size: entry.size },
                        None => old_file.disk_state,
                    },
                    entry_id: Some(entry.id),
                    path: entry.path.clone(),
                    worktree: worktree.clone(),
                }
            } else {
                worktree::File {
                    disk_state: DiskState::Deleted,
                    entry_id: old_file.entry_id,
                    path: old_file.path.clone(),
                    worktree: worktree.clone(),
                }
            };

            if new_file == **old_file {
                return;
            }

            if new_file.path != old_file.path {
                self.local_image_ids_by_path
                    .remove(&ProjectPath { path: old_file.path.clone(), worktree_id: old_file.worktree_id(cx) });
                self.local_image_ids_by_path.insert(
                    ProjectPath { worktree_id: new_file.worktree_id(cx), path: new_file.path.clone() },
                    image_id,
                );
            }

            if new_file.entry_id != old_file.entry_id {
                if let Some(entry_id) = old_file.entry_id {
                    self.local_image_ids_by_entry_id.remove(&entry_id);
                }
                if let Some(entry_id) = new_file.entry_id {
                    self.local_image_ids_by_entry_id.insert(entry_id, image_id);
                }
            }

            image.file_updated(Arc::new(new_file), cx);
        });
        None
    }

    fn image_changed_file(&mut self, image: Entity<ImageItem>, cx: &App) -> Option<()> {
        let image = image.read(cx);
        let file = &image.file;

        let image_id = image.id;
        if let Some(entry_id) = file.entry_id {
            match self.local_image_ids_by_entry_id.get(&entry_id) {
                Some(_) => {
                    return None;
                }
                None => {
                    self.local_image_ids_by_entry_id.insert(entry_id, image_id);
                }
            }
        };
        self.local_image_ids_by_path
            .insert(ProjectPath { worktree_id: file.worktree_id(cx), path: file.path.clone() }, image_id);

        Some(())
    }
}

fn create_gpui_image(content: Vec<u8>) -> anyhow::Result<Arc<gpui::Image>> {
    let format = match image::guess_format(&content) {
        Ok(format) => format,
        Err(_) => {
            // SVG has no magic bytes; accept content that parses as one.
            usvg::Tree::from_data(&content, &usvg::Options::default())?;
            return Ok(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Svg, content)));
        }
    };

    Ok(Arc::new(gpui::Image::from_bytes(
        match format {
            image::ImageFormat::Png => gpui::ImageFormat::Png,
            image::ImageFormat::Jpeg => gpui::ImageFormat::Jpeg,
            image::ImageFormat::WebP => gpui::ImageFormat::Webp,
            image::ImageFormat::Gif => gpui::ImageFormat::Gif,
            format => anyhow::bail!("Image format {format:?} not supported"),
        },
        content,
    )))
}

#[cfg(test)]
mod tests {
    use super::ImageFormat;
    use super::ImageItem;
    use super::create_gpui_image;

    const SVG: &[u8] =
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="16"><rect width="24" height="16"/></svg>"#;

    #[test]
    fn svg_metadata_comes_from_its_own_size() {
        let metadata = ImageItem::compute_metadata_from_bytes(SVG).unwrap();
        assert_eq!((metadata.width, metadata.height), (24, 16));
        assert_eq!(metadata.format, ImageFormat::Svg);
        assert_eq!(metadata.file_size, SVG.len() as u64);
        assert!(metadata.colors.is_none());
        ImageItem::compute_metadata_from_bytes(b"not an image").unwrap_err();
    }

    #[test]
    fn create_gpui_image_accepts_svg_and_rejects_unknown_content() {
        let image = create_gpui_image(SVG.to_vec()).unwrap();
        assert_eq!(image.format, gpui::ImageFormat::Svg);
        create_gpui_image(b"not an image".to_vec()).unwrap_err();
    }
}
