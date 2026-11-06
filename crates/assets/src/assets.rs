// This crate was essentially pulled out verbatim from main `ed` crate to avoid having to run RustEmbed macro whenever ed has to be rebuilt. It saves a second or two on an incremental build.

use std::borrow::Cow;

use anyhow::Context as _;
use gpui::AssetSource;
use gpui::Result;
use gpui::SharedString;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*"]
#[exclude = "*.DS_Store"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Self::get(path).map(|f| Some(f.data)).with_context(|| format!("loading asset at path {path:?}"))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter().filter_map(|p| if p.starts_with(path) { Some(p.into()) } else { None }).collect())
    }
}
