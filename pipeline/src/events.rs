//! Where a run's narration goes.
//!
//! A run has things to say while it works: what it is reading, what it chose,
//! how far the chain has got, what it wrote. At a terminal those are lines on
//! stderr. Somewhere else -- a window, a log, a test -- they are not.
//!
//! So the run reports through this trait rather than printing. [`Term`] is
//! the terminal implementation and is what the CLI installs; [`Silent`]
//! discards everything, which is what a test wants.
//!
//! Progress is the one event with structure rather than a sentence, because
//! it is the one a caller is most likely to render its own way: a bar, a
//! percentage, a window title. rustrecom cannot help here -- its own bar is a
//! `bool` on `multi_chain` with no callback -- so the numbers come from the
//! step numbers `StatsWriter` is handed.

use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What a run tells whoever started it.
///
/// `Send + Sync` because chains run on their own threads and scoring runs on
/// another again.
pub trait Events: Send + Sync {
    /// A line of narration: what is being read, what was chosen, what was
    /// written.
    fn status(&self, line: &str);

    /// Something worth noticing that does not stop the run.
    fn warn(&self, line: &str);

    /// The chain has reached `step` of `total`. Called often -- hundreds of
    /// thousands of times on a long run -- so an implementation that does
    /// real work should rate-limit itself.
    fn progress(&self, step: u64, total: u64) {
        let _ = (step, total);
    }

    /// No more progress is coming; tidy up anything drawn for it.
    fn progress_done(&self) {}
}

/// A sink that discards everything. For tests, and for a caller that wants
/// only the files.
pub struct Silent;

impl Events for Silent {
    fn status(&self, _line: &str) {}
    fn warn(&self, _line: &str) {}
}

/// Lines on stderr, with a progress bar when stderr is a terminal.
pub struct Term {
    bar: Mutex<Bar>,
}

impl Term {
    /// `progress` is the caller's wish; a bar is drawn only if stderr is also
    /// a terminal, since a redirected bar is line noise in a log file.
    pub fn new(progress: bool) -> Term {
        let now = Instant::now();
        Term {
            bar: Mutex::new(Bar {
                on: progress && std::io::stderr().is_terminal(),
                visible: false,
                started: now,
                last_drawn: now,
            }),
        }
    }

    /// Print a line, taking the bar down first so the two do not collide.
    fn line(&self, text: &str) {
        let mut bar = self.bar.lock().expect("bar");
        bar.clear();
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{text}");
    }
}

impl Events for Term {
    fn status(&self, line: &str) {
        self.line(line);
    }

    fn warn(&self, line: &str) {
        self.line(&format!("warning: {line}"));
    }

    fn progress(&self, step: u64, total: u64) {
        self.bar.lock().expect("bar").draw(step, total);
    }

    fn progress_done(&self) {
        self.bar.lock().expect("bar").clear();
    }
}

/// The one line the bar occupies, and what is needed to redraw it.
struct Bar {
    on: bool,
    visible: bool,
    started: Instant,
    last_drawn: Instant,
}

/// How wide the bar itself is drawn, in characters.
const WIDTH: usize = 40;

impl Bar {
    fn draw(&mut self, step: u64, total: u64) {
        if !self.on || total == 0 {
            return;
        }
        let now = Instant::now();
        let done = (step + 1).min(total);
        // Ten frames a second is plenty to look alive, and a chain reports
        // steps far faster than that; redrawing on each would cost more than
        // the step it reports. The last one always draws, so the bar does not
        // stop short of the end.
        if done < total && now.duration_since(self.last_drawn).as_millis() < 100 {
            return;
        }
        self.last_drawn = now;
        self.visible = true;

        let fraction = done as f64 / total as f64;
        let elapsed = self.started.elapsed().as_secs_f64();
        // Projected from the rate so far. Early on this is a poor estimate,
        // which is why the elapsed time is shown beside it rather than the
        // guess alone.
        let remaining = if fraction > 0.0 { elapsed / fraction - elapsed } else { 0.0 };
        let filled = (fraction * WIDTH as f64).round() as usize;

        let mut err = std::io::stderr().lock();
        let _ = write!(
            err,
            "\r  [{}{}] {:>3.0}%  {done}/{total} steps  {}  eta {}   ",
            "#".repeat(filled),
            "-".repeat(WIDTH - filled),
            fraction * 100.0,
            clock(elapsed),
            clock(remaining),
        );
        let _ = err.flush();
    }

    /// Wipe the line, so whatever is printed next starts clean.
    fn clear(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        let mut err = std::io::stderr().lock();
        // Wide enough for the longest line `draw` writes.
        let _ = write!(err, "\r{}\r", " ".repeat(WIDTH + 50));
        let _ = err.flush();
    }
}

/// Seconds as h:mm:ss, or m:ss below an hour.
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// What the run reports through. An `Arc` because scoring happens on a thread
/// that outlives the call that started it.
pub type Sink = Arc<dyn Events>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_as_a_duration() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(61.0), "1:01");
        assert_eq!(clock(3661.0), "1:01:01");
        // A negative estimate is possible if the clock jumps; show zero
        // rather than a nonsense duration.
        assert_eq!(clock(-5.0), "0:00");
    }

    /// Silent must satisfy the bound the chains need.
    #[test]
    fn a_sink_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Silent>();
        assert_send_sync::<Term>();
        let _: Sink = Arc::new(Silent);
    }
}
