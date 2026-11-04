//! Boot path (spec 0001). The native window and the first pane arrive with
//! T-010; until then this binary only proves the kept modules build.

mod config;
#[allow(dead_code)] // used from T-010 on
mod theme;
#[allow(dead_code)] // used from T-010 on
mod timing;

fn main() {
    // Taken first: TTFP is measured from here (spec 0001).
    let start = std::time::Instant::now();
    let timing = timing::Timing::new(start);
    let loaded = config::default_path()
        .map(|p| config::load(&p))
        .unwrap_or_else(|| config::Loaded {
            config: config::Config::default(),
            notices: vec![],
        });
    timing.mark(timing::Mark::Config);
    for notice in &loaded.notices {
        eprintln!("notice: {notice}");
    }
    eprintln!(
        "zed-term: native UI not implemented yet (see tasks/T-010). Theme background {}",
        theme::DARK.background
    );
}
