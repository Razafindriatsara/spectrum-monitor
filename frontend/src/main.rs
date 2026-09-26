//! Web-Oberfläche in Leptos, läuft als WebAssembly im Browser. Bauen mit
//! `trunk build --release`, der Server liefert `dist/` dann aus.

mod app;
mod format;
mod geo;
mod live;
mod map;
mod spectrum;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
