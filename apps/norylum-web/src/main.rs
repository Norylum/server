mod api;
mod app;
mod components;
mod pages;

fn main() {
    leptos::mount_to_body(app::App);
}
