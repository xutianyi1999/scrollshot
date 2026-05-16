use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "scrollshot",
    version,
    about = "Capture a vertical long screenshot on Windows by scrolling downward."
)]
pub struct Cli {
    /// Match a visible window whose title contains this substring. Uses the foreground window when omitted.
    #[arg(long)]
    pub title: Option<String>,

    /// Let you drag out the capture region on screen before scrolling starts.
    #[arg(long, default_value_t = false)]
    pub select_region: bool,

    /// Output PNG path.
    #[arg(long, default_value = "scrollshot.png")]
    pub output: PathBuf,

    /// Maximum number of downward scroll steps to attempt.
    #[arg(long, default_value_t = 20)]
    pub max_scrolls: usize,

    /// Delay after each downward wheel scroll, in milliseconds.
    #[arg(long, default_value_t = 120)]
    pub settle_ms: u64,

    /// Number of downward wheel notches to send per step.
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(i32).range(1..))]
    pub wheel_notches: i32,
}
