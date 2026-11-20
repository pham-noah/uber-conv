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
    ]
);
