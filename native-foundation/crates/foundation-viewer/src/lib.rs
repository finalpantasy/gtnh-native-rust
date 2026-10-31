//! Camera and input rules for the optional native technical viewer.
pub mod camera;
pub mod input;
pub mod inspection;
pub mod stepper;

#[cfg(all(test, feature = "desktop"))]
mod font_tests {
    #[test]
    fn bundled_interface_fonts_exist_in_the_desktop_build() {
        assert!(
            !egui::FontDefinitions::default().font_data.is_empty(),
            "desktop UI cannot render labels without bundled fonts"
        );
    }
}
