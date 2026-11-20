`xattr/` is xattr 0.2.3 with a one-line fix: on Linux/Android `ENOATTR` is now
`ENODATA`, because current `libc` releases no longer export `ENOATTR` there.
GPUI 0.2.x pulls this crate in transitively (gpui -> gpui_http_client ->
zed-async-tar -> xattr) and fails to compile on Linux without it.
Wired in through `[patch.crates-io]` in the workspace Cargo.toml. Delete the
patch once the GPUI release you use no longer depends on xattr 0.2.3.
