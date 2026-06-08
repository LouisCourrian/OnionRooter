// Build script: bakes the OnionRouter app icon into the Windows binary so the
// system tray icon, the .exe in Explorer, and Alt-Tab thumbnails all use the
// real brand logo.
//
// The icon is built from companion/assets/app-icon.png (a high-res PNG with
// transparency), resized to the standard icon sizes and packed into a single
// multi-resolution .ico, embedded as the "MAINICON" resource via winres.

fn main() {
    #[cfg(windows)]
    if let Err(e) = build_windows_resources() {
        // Don't fail the build on resource errors -- the tray just falls back
        // to a default icon. Surface a warning visible in cargo output.
        println!("cargo:warning=failed to build Windows icon resource: {e}");
    }
}

#[cfg(windows)]
fn build_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    let manifest = std::env::var("CARGO_MANIFEST_DIR")?;
    let src = PathBuf::from(&manifest).join("assets").join("app-icon.png");
    println!("cargo:rerun-if-changed={}", src.display());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let ico_path = out_dir.join("app.ico");

    // Decode the master PNG once, then downscale to each icon size.
    let master = image::open(&src)?.to_rgba8();
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let resized = image::imageops::resize(
            &master,
            size,
            size,
            image::imageops::FilterType::Lanczos3,
        );
        let img = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        icon_dir.add_entry(ico::IconDirEntry::encode(&img)?);
    }
    icon_dir.write(std::fs::File::create(&ico_path)?)?;

    // tray-item loads it via LoadIconA(..., "MAINICON").
    let mut res = winres::WindowsResource::new();
    res.set_icon_with_id(ico_path.to_str().ok_or("non-utf8 ico path")?, "MAINICON");
    res.compile()?;

    Ok(())
}
