//! Links an optional bundled libheif when `UBERCONV_LIBHEIF_DIR` names an install
//! prefix (`include/libheif/heif.h` plus `heif.lib`, `libheif.a` or
//! `libheif.so`/`libheif.dylib`). Unset, Windows uses its system HEIF codec and
//! other platforms report HEIC as unavailable.

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
        // Static libheif is C++: link it with its decoder and the C++ runtime.
        // libde265's file name gives the link name `libde265`, not `de265`.
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
