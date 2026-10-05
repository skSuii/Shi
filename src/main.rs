mod app;
mod i18n;
mod layers;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("Shi — CAD"),
        ..Default::default()
    };
    eframe::run_native(
        "Shi",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
