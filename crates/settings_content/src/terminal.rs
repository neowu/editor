use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::EnumDiscriminants)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectory {
    /// Use the current file's directory, falling back to the project directory,
    /// then the first project in the workspace.
    CurrentFileDirectory,
    /// Use the current file's project directory. Fallback to the
    /// first project directory strategy if unsuccessful.
    CurrentProjectDirectory,
    /// Use the first project in this workspace's directory. Fallback to using
    /// this platform's home directory.
    FirstProjectDirectory,
    /// Always use this platform's home directory (if it can be found).
    AlwaysHome,
    /// Always use a specific directory. This value will be shell expanded.
    /// If this path is not a valid directory the terminal will default to
    /// this platform's home directory  (if it can be found).
    Always { directory: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLineHeight {
    /// Use a line height that's comfortable for reading, 1.618
    #[default]
    Comfortable,
    /// Use a standard line height, 1.3. This option is useful for TUIs,
    /// particularly if they use box characters
    Standard,
    /// Use a custom line height.
    Custom(#[serde(serialize_with = "crate::serialize_f32_with_two_decimal_places")] f32),
}

impl TerminalLineHeight {
    pub const fn value(&self) -> f32 {
        match self {
            TerminalLineHeight::Comfortable => 1.618,
            TerminalLineHeight::Standard => 1.3,
            TerminalLineHeight::Custom(line_height) => f32::max(*line_height, 1.),
        }
    }
}

/// When to show the scrollbar.
///
/// Default: auto
#[derive(
    Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ShowScrollbar {
    /// Show the scrollbar if there's important information or
    /// follow the system's configured behavior.
    #[default]
    Auto,
    /// Match the system's configured behavior.
    System,
    /// Always show the scrollbar.
    Always,
    /// Never show the scrollbar.
    Never,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBlink {
    /// Never blink the cursor, ignoring the terminal mode.
    Off,
    /// Default the cursor blink to off, but allow the terminal to
    /// set blinking.
    TerminalControlled,
    /// Always blink the cursor, ignoring the terminal mode.
    On,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, strum::VariantArray, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
pub enum AlternateScroll {
    On,
    Off,
}
