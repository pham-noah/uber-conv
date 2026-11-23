//! uberconv desktop UI, built on GPUI.

// The GUI binary must not allocate a console window: launched from Explorer, a
// console-subsystem exe flashes (or leaves) a terminal next to the app. Debug
// builds keep the console so `println!` and panics stay visible in `cargo run`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod assets;
mod dialogs;
mod files;
mod settings;
mod theme;
mod ui;
mod views;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext as _, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds,
    WindowOptions,
};

use crate::{actions::*, app::ConverterApp, settings::Settings};

fn main() {
    Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
        assets::install_fonts(cx);

        // `secondary` is Cmd on macOS and Ctrl elsewhere.
        cx.bind_keys([
            KeyBinding::new("secondary-o", OpenFiles, Some("ConverterApp")),
            KeyBinding::new("secondary-enter", ConvertAll, Some("ConverterApp")),
            KeyBinding::new("secondary-.", CancelAll, Some("ConverterApp")),
            KeyBinding::new("secondary-k", ClearFinished, Some("ConverterApp")),
            KeyBinding::new("secondary-,", ToggleSettings, Some("ConverterApp")),
            KeyBinding::new("escape", CloseOverlay, Some("ConverterApp")),
            KeyBinding::new("secondary-z", Undo, Some("ConverterApp")),
            KeyBinding::new("tab", NextField, Some("ConverterApp")),
            KeyBinding::new("shift-tab", PrevField, Some("ConverterApp")),
            KeyBinding::new("delete", RemovePicked, Some("ConverterApp")),
            KeyBinding::new("space", TogglePickedOptions, Some("ConverterApp")),
            KeyBinding::new("secondary-shift-c", CopyErrors, Some("ConverterApp")),
        ]);

        let settings = Settings::load();
        let (w, h) = settings.window.unwrap_or((880.0, 720.0));
        let bounds = Bounds::centered(None, size(px(w.max(620.0)), px(h.max(520.0))), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(620.), px(520.))),
                titlebar: Some(TitlebarOptions { title: Some("uberconv".into()), ..Default::default() }),
                ..Default::default()
            },
            move |window, cx| {
                let app = cx.new(|cx| {
                    let mut app = ConverterApp::new(settings, cx);
                    window.focus(&app.focus);
                    // Repaint when the system switches between light and dark.
                    let weak = cx.weak_entity();
                    window
                        .observe_window_appearance(move |_, cx| {
                            let _ = weak.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    // `uberconv-app a.png folder/` / "Open with" queues files at startup.
                    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
                    if !args.is_empty() {
                        app.add_paths(args, cx);
                    }
                    app
                });
                let handle = app.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    handle.update(cx, |app, _| app.settings.save());
                    true
                });
                app
            },
        )
        .expect("failed to open window");

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
