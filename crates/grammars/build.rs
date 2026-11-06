fn main() {
    // `#[derive(RustEmbed)]` embeds files via generated `include_bytes!`, which
    // only tracks files that already existed. Watch the whole
    // directory so added or removed query files trigger a rebuild.
    println!("cargo::rerun-if-changed=src");
}
