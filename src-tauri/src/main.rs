// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Taken first: TTFP is measured from here (spec 0001).
    let start = std::time::Instant::now();
    zed_term_lib::run(start);
}
