#[cfg(windows)]
mod app;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod capture_progress;
#[cfg(windows)]
mod cli;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod region;
#[cfg(windows)]
mod screen_rect;
#[cfg(windows)]
mod scroll;
#[cfg(windows)]
mod stitch;

cfg_if::cfg_if! {
    if #[cfg(windows)] {
        fn main() {
            app::run();
        }
    } else {
        fn main() {
            eprintln!("scrollshot can only run on Windows.");
            std::process::exit(1);
        }
    }
}
