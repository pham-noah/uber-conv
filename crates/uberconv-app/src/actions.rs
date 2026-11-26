use gpui::actions;

actions!(
    uberconv,
    [
        OpenFiles,
        ConvertAll,
        CancelAll,
        ClearFinished,
        ToggleSettings,
        CloseOverlay,
        NextField,
        PrevField,
        Undo,
        /// Del — remove the row the keyboard is on.
        RemovePicked,
        /// Space — open or close the picked row's per-file options.
        TogglePickedOptions,
        /// Ctrl+Shift+C — every error in the queue, one per line.
        CopyErrors,
        /// Combine every image in the queue into one PDF.
        CombinePdf,
        /// Enter — convert the row the keyboard is on, and only that row.
        ConvertPicked,
        /// Up / Down — walk the queue without the mouse.
        NextRow,
        PrevRow,
        /// F1 — the keyboard sheet.
        ToggleHelp,
        /// "/" or Ctrl+F — put the cursor in the search box.
        FocusSearch,
        /// Shift+Down / Shift+Up — extend the picked span.
        ExtendNextRow,
        ExtendPrevRow,
    ]
);
