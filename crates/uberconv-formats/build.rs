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
    let has = |name: &str| lib.join(name).is_file();
    let msvc_static = has("heif.lib");
    let unix_static = has("libheif.a");
    let shared = has("libheif.so") || has("libheif.dylib");
    if !(msvc_static || unix_static || shared) {
        println!("cargo:warning=no libheif library in {lib:?}; HEIC stays disabled");
        return;
    }

    println!("cargo:rustc-cfg=heic_libheif");
    println!("cargo:rustc-link-search=native={}", lib.display());

    if msvc_static {
        // Static libheif is a C++ library: link it whole, plus the decoder it was
        // built against and the C++ runtime. libde265 ships as `libde265.lib`, so
        // the link name is `libde265`, not `de265`. Building the libs with /MT
        // would drop the MSVCP140.dll dependency, but rustc links the dynamic
        // CRT, so /MD builds of both are the ones that fit.
        println!("cargo:rustc-link-lib=static=heif");
        println!("cargo:rustc-link-lib=static=libde265");
        println!("cargo:rustc-link-lib=msvcprt");
    } else if unix_static {
        println!("cargo:rustc-link-lib=static=heif");
        println!("cargo:rustc-link-lib=de265");
        println!("cargo:rustc-link-lib=dylib=stdc++");
    } else {
        println!("cargo:rustc-link-lib=dylib=heif");
    }
}
