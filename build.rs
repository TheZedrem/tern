//! Embeds Tern's icon and version info in tern.exe, so Explorer, the
//! taskbar and the MSI's Start menu shortcut show them (spec 0012). Only a
//! Windows build does anything here; the crate isn't even built elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The versions come from Cargo.toml. FileDescription is the name
        // Task Manager shows, so it's the product name, not the description.
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("ProductName", "Tern")
            .set("FileDescription", "Tern");
        if let Err(error) = resource.compile() {
            panic!("embedding the icon and version info failed: {error}");
        }
    }
}
