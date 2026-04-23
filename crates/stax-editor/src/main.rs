#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast as _;
    let canvas = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document")
        .get_element_by_id("stax_canvas")
        .expect("no #stax_canvas in DOM")
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .expect("#stax_canvas is not a canvas");

    wasm_bindgen_futures::spawn_local(async move {
        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(stax_editor::StaxApp::new(cc)))),
            )
            .await
            .expect("failed to start stax");
    });
}
