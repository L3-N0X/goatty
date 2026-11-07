//! Frame statistics (`GOATTY_STATS=1`): the element accumulates, the view
//! prints one line per second to stderr.

use std::time::Instant;

#[derive(Default)]
pub struct Stats {
    pub frames: u32,
    pub prepaint_us: u64,
    pub paint_us: u64,
    pub max_frame_us: u64,
    pub shaped_segments: u64,
    pub snapshot_us: u64,
    pub window_start: Option<Instant>,
}

impl Stats {
    /// Print the stats line for the window since the last report and reset.
    /// `bytes_in` is the session's cumulative input byte count.
    pub fn report(&mut self, bytes_in: u64, last_bytes: &mut u64) {
        let now = Instant::now();
        let secs = self.window_start.map(|t| (now - t).as_secs_f64()).unwrap_or(1.0);
        let mbps = (bytes_in - *last_bytes) as f64 / 1e6 / secs;
        *last_bytes = bytes_in;
        if self.frames > 0 {
            let f = self.frames as u64;
            eprintln!(
                "[stats] {:5.1} fps | prepaint {:6.2} ms avg (lock+snapshot {:5.2}) | paint {:5.2} ms avg | worst {:6.2} ms | {:5} segs/frame | in {:7.2} MB/s",
                self.frames as f64 / secs,
                self.prepaint_us as f64 / f as f64 / 1e3,
                self.snapshot_us as f64 / f as f64 / 1e3,
                self.paint_us as f64 / f as f64 / 1e3,
                self.max_frame_us as f64 / 1e3,
                self.shaped_segments / f,
                mbps
            );
        } else if mbps > 0.0 {
            eprintln!("[stats] idle frames | in {mbps:7.2} MB/s");
        }
        *self = Stats { window_start: Some(now), ..Default::default() };
    }
}
