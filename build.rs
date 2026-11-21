use std::{env, fs::File, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/netladder.png");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set"));
    let icon_path = out_dir.join("netladder.ico");
    create_icon(&icon_path);

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(icon_path.to_str().expect("icon path is not UTF-8"))
        .set("ProductName", "NetLadder")
        .set("FileDescription", "Per-process download bandwidth limiter");

    if env::var("PROFILE").as_deref() == Ok("release") {
        resource.set_manifest(
            r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#,
        );
    }

    resource
        .compile()
        .expect("failed to compile Windows resources");
}

fn create_icon(output: &PathBuf) {
    let source = image::open("assets/netladder.png")
        .expect("failed to load assets/netladder.png")
        .to_rgba8();
    let mut directory = ico::IconDir::new(ico::ResourceType::Icon);

    for size in [16, 24, 32, 48, 64, 128, 256] {
        let resized =
            image::imageops::resize(&source, size, size, image::imageops::FilterType::Lanczos3);
        let icon = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        directory.add_entry(ico::IconDirEntry::encode(&icon).expect("failed to encode icon"));
    }

    let file = File::create(output).expect("failed to create generated ICO");
    directory
        .write(file)
        .expect("failed to write generated ICO");
}
