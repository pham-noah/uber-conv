//! Embeds the icon and version info into `uberconv-app.exe`; GPUI's Windows
//! backend loads icon resource #1 straight from the executable.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/logo/uberconv.ico");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/logo/uberconv.ico");
        // Explorer's details pane and the "Apps" list.
        res.set("ProductName", "uberconv");
        res.set("FileDescription", "uberconv — convert anything to anything");
        res.set("OriginalFilename", "uberconv-app.exe");
        res.set("InternalName", "uberconv");
        res.set("Comments", "https://github.com/pham-noah/uber-conv");
        // 0.1.0.0 — four u16 fields, so this mirrors the workspace version.
        res.set_version_info(winresource::VersionInfo::FILEVERSION, 0x0000_0001_0000_0000);
        res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, 0x0000_0001_0000_0000);
        res.set("LegalCopyright", "Copyright 2026 Noah Pham, Apache-2.0");
        if let Err(e) = res.compile() {
            panic!(
                "could not embed the Windows icon resources ({e}). \
                 The resource compiler (rc.exe) comes with the Windows SDK."
            );
        }
    }
}
