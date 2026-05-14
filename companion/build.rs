// Build script: bakes an .ico resource into the Windows binary so the
// system tray icon (and Explorer/Alt-Tab thumbnails) have a proper
// graphic to display.
//
// Generates the icon procedurally rather than committing a binary
// blob -- a small purple-onion silhouette matching the extension's
// accent colour.

fn main() {
    #[cfg(windows)]
    if let Err(e) = build_windows_resources() {
        // Don't fail the build on resource errors -- the tray will just
        // not have a custom icon. Surface a warning visible in cargo output.
        println!("cargo:warning=failed to build Windows icon resource: {e}");
    }
}

#[cfg(windows)]
fn build_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::path::PathBuf;

    let out_dir: PathBuf = std::env::var("OUT_DIR")?.into();
    let ico_path = out_dir.join("tray.ico");

    // Two sizes packed into the same .ico so Windows picks the right
    // resolution for the toolbar (16px) vs Alt-Tab / Explorer (32px).
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 32u32] {
        let rgba = generate_icon_rgba(size as usize);
        let img = ico::IconImage::from_rgba_data(size, size, rgba);
        icon_dir.add_entry(ico::IconDirEntry::encode(&img)?);
    }
    icon_dir.write(File::create(&ico_path)?)?;

    let mut res = winres::WindowsResource::new();
    // Use a known resource name so tray-item can find it via
    // LoadIconA(..., "MAINICON").
    res.set_icon_with_id(ico_path.to_str().ok_or("non-utf8 ico path")?, "MAINICON");
    res.compile()?;

    Ok(())
}

/// Procedural purple-onion glyph at any square size. Soft anti-aliased
/// edge, white onion lines on a purple disc, transparent background.
#[cfg(windows)]
fn generate_icon_rgba(size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; size * size * 4];
    let s = size as f32;
    let cx = s / 2.0 - 0.5;
    let cy = s / 2.0 - 0.5;
    let r_inner = s * 0.38;
    let r_outer = s * 0.46;
    let (pr, pg, pb) = (0x7a_u8, 0x3f_u8, 0xb8_u8);

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let i = (y * size + x) * 4;
            if d <= r_inner {
                buf[i] = pr;
                buf[i + 1] = pg;
                buf[i + 2] = pb;
                buf[i + 3] = 0xff;
            } else if d <= r_outer {
                let t = 1.0 - (d - r_inner) / (r_outer - r_inner);
                buf[i] = pr;
                buf[i + 1] = pg;
                buf[i + 2] = pb;
                buf[i + 3] = (t.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }

    // White onion outline: vertical line + two side arches.
    let mut stamp = |x: i32, y: i32| {
        if x < 0 || y < 0 || x as usize >= size || y as usize >= size {
            return;
        }
        let i = ((y as usize) * size + (x as usize)) * 4;
        if buf[i + 3] == 0 {
            return;
        }
        buf[i] = 0xff;
        buf[i + 1] = 0xff;
        buf[i + 2] = 0xff;
        buf[i + 3] = 0xff;
    };
    let center_x = cx.round() as i32;
    for y in 0..size {
        stamp(center_x, y as i32);
    }
    let arch_w = s * 0.22;
    for y in 0..size {
        let t = (y as f32 - cy) / (s * 0.45);
        if t.abs() > 1.0 {
            continue;
        }
        let phase = (1.0 - t * t).sqrt();
        let offset = arch_w * phase;
        stamp((cx - offset).round() as i32, y as i32);
        stamp((cx + offset).round() as i32, y as i32);
    }

    buf
}
