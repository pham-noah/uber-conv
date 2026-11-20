//! Optional bundled libheif.
//!
//! HEIC decoding on Linux/macOS (and on Windows when you would rather not rely
//! on the Microsoft Store HEIF extension) needs libheif. Point
//! `UBERCONV_LIBHEIF_DIR` at an install prefix and this script wires it up:
//!
//! ```text
//! UBERCONV_LIBHEIF_DIR=C:/deps/libheif cargo build --release
//! ```
//!
//! The directory must contain `include/libheif/heif.h` and, in `lib/`,
//! `heif.lib` (MSVC), `libheif.a` or `libheif.so`/`libheif.dylib`.
//! Without it nothing breaks: Windows falls back to the system codec and other
//! platforms report that HEIC needs libheif.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=UBERCONV_LIBHEIF_DIR");
    println!("cargo::rustc-check-cfg=cfg(heic_libheif)");

    let Some(prefix) = std::env::var_os("UBERCONV_LIBHEIF_DIR").map(PathBuf::from) else {
        return;
    };

    let header = prefix.join("include").join("libheif").join("heif.h");
    if !header.is_file() {
        println!("cargo:warning=UBERCONV_LIBHEIF_DIR is set but {header:?} is missing; HEIC stays disabled");
        return;
    }

    let lib = prefix.join("lib");
    let candidates = ["heif.lib", "libheif.a", "heif.lib"];
    let linked = candidates.iter().find(|name| lib.join(name).is_file()).is_some()
        || lib.join("libheif.so").is_file()
        || lib.join("libheif.dylib").is_file();
    if !linked {
        println!("cargo:warning=no libheif library in {lib:?}; HEIC stays disabled");
        return;
    }

    println!("cargo:rustc-cfg=heic_libheif");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=heif");
    // libheif is C++; MSVC needs its runtime, and a static libheif pulls libde265 too.
    println!("cargo:rustc-link-lib=de265");
    if cfg!(target_env = "msvc") {
        println!("cargo:rustc-link-lib=static=msvcprt");
    }
}
