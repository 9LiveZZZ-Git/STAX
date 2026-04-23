fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("stax")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "stax",
        opts,
        Box::new(|cc| Ok(Box::new(stax_editor::StaxApp::new(cc)))),
    )
}
