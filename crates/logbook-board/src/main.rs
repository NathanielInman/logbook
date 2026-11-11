//! `logbook-board`: the Logbook drawer. A GTK4 layer-shell window that stays
//! resident while hidden and opens from wherever the bar was clicked.

#![cfg_attr(not(target_os = "linux"), allow(unused))]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("logbook-board is the Linux (Wayland layer-shell) drawer");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
mod anim;
#[cfg(target_os = "linux")]
mod drawer;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod meter;
#[cfg(target_os = "linux")]
mod stage;
#[cfg(target_os = "linux")]
mod theme;

#[cfg(target_os = "linux")]
fn main() -> gtk::glib::ExitCode {
    linux::main()
}
