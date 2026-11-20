//! Embeds the app icon and version info into `uberconv-app.exe`.
//!
//! GPUI's Windows backend picks the window and taskbar icon up straight from the
//! executable (it loads icon resource #1 — `load_icon()` in gpui's
//! `platform/windows/platform.rs`), so declaring the resource here is the whole
//! job; there is no runtime icon code in the app.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/logo/uberconv.ico");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/logo/uberconv.ico");
        // Shown in Explorer's details pane and in the "Apps" list.
        res.set("ProductName", "uberconv");
        res.set("FileDescription", "uberconv — convert anything to anything");
        res.set("OriginalFilename", "uberconv-app.exe");
        res.set("InternalName", "uberconv");
        res.set("Comments", "https://github.com/pham-noah/uber-conv");
        res.set_version_info(winresource::VersionInfo::FILEVERSION, 0x0000_0002_0002_0000);
        res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, 0x0000_0002_0002_0000);
        if let Err(e) = res.compile() {
            panic!(
                "could not embed the Windows icon resources ({e}). \
                 The resource compiler (rc.exe) comes with the Windows SDK."
            );
        }
    }
}
