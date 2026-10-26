use std::fs::File;
use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The shipping VPN needs elevation for its TUN interface. Requiring it for
    // debug and test binaries prevents `cargo test` from running on Windows.
    let execution_level = if std::env::var("PROFILE").as_deref() == Ok("release") {
        "requireAdministrator"
    } else {
        "asInvoker"
    };
    let manifest = r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{{EXECUTION_LEVEL}}" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{4f476546-9377-4f62-9d1d-0d47669f6b63}" />
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
</assembly>
"#
    .replace("{{EXECUTION_LEVEL}}", execution_level);

    println!("cargo:rerun-if-changed=assets/brand-icon.png");
    let icon_path = generate_windows_icon();
    let mut resource = winresource::WindowsResource::new();
    resource
        .set("ProductName", "universe")
        .set("FileDescription", "universe VPN client")
        .set("LegalCopyright", "universe contributors")
        .set_icon(icon_path.to_str().expect("icon path must be valid UTF-8"))
        .set_manifest(&manifest);
    resource
        .compile()
        .expect("failed to compile Windows resources");
}

fn generate_windows_icon() -> PathBuf {
    let mut source = image::load_from_memory(include_bytes!("assets/brand-icon.png"))
        .expect("brand icon must be a valid PNG")
        .to_rgba8();
    apply_rounded_alpha(&mut source);

    let mut icon = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16_u32, 24, 32, 48, 64, 128, 256] {
        let resized =
            image::imageops::resize(&source, size, size, image::imageops::FilterType::Lanczos3);
        let image = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        icon.add_entry(ico::IconDirEntry::encode(&image).expect("failed to encode icon layer"));
    }

    let path = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR must be set"))
        .join("universe-brand.ico");
    icon.write(File::create(&path).expect("failed to create generated icon"))
        .expect("failed to write generated icon");
    path
}

fn apply_rounded_alpha(image: &mut image::RgbaImage) {
    let width = image.width() as f32;
    let height = image.height() as f32;
    let radius = width.min(height) * 0.205;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let x = x as f32 + 0.5;
        let y = y as f32 + 0.5;
        let nearest_x = x.clamp(radius, width - radius);
        let nearest_y = y.clamp(radius, height - radius);
        let distance = ((x - nearest_x).powi(2) + (y - nearest_y).powi(2)).sqrt();
        let coverage = (radius + 0.75 - distance).clamp(0.0, 1.0);
        if coverage < 1.0 {
            pixel.0[3] = ((pixel.0[3] as f32) * coverage).round() as u8;
            if pixel.0[3] == 0 {
                pixel.0[0] = 0;
                pixel.0[1] = 0;
                pixel.0[2] = 0;
            }
        }
    }
}
