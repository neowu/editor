use std::fmt::Display;

use serde::Deserialize;
use serde::Serialize;

use crate::serialize_f32_with_two_decimal_places;

/// When to populate a new search's query based on the text under the cursor.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum SeedQuerySetting {
    /// Always populate the search query with the word under the cursor.
    Always,
    /// Only populate the search query when there is text selected.
    Selection,
    /// Never populate the search query
    Never,
}

/// Where to show LSP results that can contain multiple locations.
#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum OpenResultsIn {
    /// Open the results in a multibuffer.
    #[default]
    MultiBuffer,
    /// Open the results in a filterable picker.
    Picker,
}

/// Opacity of the inactive panes. 0 means transparent, 1 means opaque.
///
/// Valid range: 0.0 to 1.0
/// Default: 1.0
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, PartialOrd, derive_more::FromStr)]
#[serde(transparent)]
pub struct InactiveOpacity(#[serde(serialize_with = "serialize_f32_with_two_decimal_places")] pub f32);

impl Display for InactiveOpacity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.1}", self.0)
    }
}

impl From<f32> for InactiveOpacity {
    fn from(x: f32) -> Self {
        Self(x)
    }
}
