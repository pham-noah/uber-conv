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
    ]
);
