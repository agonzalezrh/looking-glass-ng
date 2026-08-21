//! Performance measurement and logging for the compositor pipeline.
//!
//! Timestamps:
//!   T0 = guest frame generated (not available without host instrumentation)
//!   T2 = KVMFR frame received by FrameProducer (update returns Updated)
//!   T3 = texture uploaded to GPU
//!   T5 = rendering begins (bind)
//!   T6 = GPU submission (submit)
//!
//! Derived metrics:
//!   frame_time    = T6 - T5 (how long rendering + submitting takes)
//!   texture_time  = T3 - T2 (how long texture import takes)
//!   fps           = frames per second (rolling average)
//!   dropped       = consecutive Unchanged results from producers

use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PerfStats {
    pub frame_count: u64,
    pub last_log: Instant,
    pub frame_start: Option<Instant>,

    // Rolling accumulators (reset every LOG_INTERVAL frames)
    pub total_frame_time_ns: u64,
    pub total_texture_time_ns: u64,
    pub total_bind_time_ns: u64,
    pub total_submit_time_ns: u64,
    pub frame_count_since_log: u64,

    // Frame drop tracking
    pub consecutive_drops: u64,
    pub total_drops: u64,
}

const LOG_INTERVAL: u64 = 60;

impl PerfStats {
    pub fn new() -> Self {
        PerfStats {
            frame_count: 0,
            last_log: Instant::now(),
            frame_start: None,
            total_frame_time_ns: 0,
            total_texture_time_ns: 0,
            total_bind_time_ns: 0,
            total_submit_time_ns: 0,
            frame_count_since_log: 0,
            consecutive_drops: 0,
            total_drops: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame_count += 1;
        self.frame_start = Some(Instant::now());
    }

    pub fn record_texture(&mut self, elapsed_ns: u64) {
        self.total_texture_time_ns += elapsed_ns;
    }

    pub fn record_bind(&mut self, elapsed_ns: u64) {
        self.total_bind_time_ns += elapsed_ns;
    }

    pub fn record_submit(&mut self, elapsed_ns: u64) {
        self.total_submit_time_ns += elapsed_ns;
    }

    pub fn record_dropped(&mut self) {
        self.consecutive_drops += 1;
        self.total_drops += 1;
    }

    pub fn record_frame(&mut self) {
        self.consecutive_drops = 0;
        if let Some(start) = self.frame_start {
            let elapsed = start.elapsed().as_nanos() as u64;
            self.total_frame_time_ns += elapsed;
            self.frame_count_since_log += 1;

            tracing::debug!(
                frame = %self.frame_count,
                frame_time_us = %(elapsed / 1000),
                "RENDER"
            );

            if self.frame_count_since_log >= LOG_INTERVAL {
                self.log();
                self.frame_count_since_log = 0;
                self.total_frame_time_ns = 0;
                self.total_texture_time_ns = 0;
                self.total_bind_time_ns = 0;
                self.total_submit_time_ns = 0;
                self.last_log = Instant::now();
            }
        }
    }

    fn log(&self) {
        let n = self.frame_count_since_log.max(1);
        let avg_frame = self.total_frame_time_ns / n;
        let avg_tex = self.total_texture_time_ns / n;
        let avg_submit = self.total_submit_time_ns / n;

        let avg_frame_ms = avg_frame as f64 / 1_000_000.0;
        let avg_fps = 1_000_000_000.0 / avg_frame as f64;
        let avg_tex_ms = avg_tex as f64 / 1_000_000.0;
        let avg_submit_ms = avg_submit as f64 / 1_000_000.0;

        tracing::info!(
            frames = %n,
            total_frames = %self.frame_count,
            avg_fps = format!("{:.1}", avg_fps),
            avg_frame_ms = format!("{:.3}", avg_frame_ms),
            avg_texture_ms = format!("{:.3}", avg_tex_ms),
            avg_submit_ms = format!("{:.3}", avg_submit_ms),
            consecutive_drops = %self.consecutive_drops,
            total_drops = %self.total_drops,
            "PERF"
        );
    }
}
