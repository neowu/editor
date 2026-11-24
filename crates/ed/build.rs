use std::env;
fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"), "This editor only supports macOS.");
    // Sets the minimum macOS of the linked binary; keep in sync with Info.plist and the gpui_macos shader build.
    println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=27.2");
}
