//! What a front end that is not a terminal needs from this crate.
//!
//! An integration test is compiled as its own crate, so anything reachable
//! here is reachable from a GUI, and anything private is not. That is the
//! whole point of these: they fail if the library stops being usable from
//! outside, which a unit test inside the crate would not notice.

use std::sync::{Arc, Mutex};

use clap::Parser;
use rda_ensemble::events::{Events, Sink, Silent};
use rda_ensemble::run::{self, Cancel};
use rda_ensemble::{Cli, Command, RunArgs};

/// A sink that keeps what it was told, which is roughly what a window does.
#[derive(Default)]
struct Captured {
    lines: Mutex<Vec<String>>,
    progress: Mutex<Vec<(u64, u64)>>,
}

impl Events for Captured {
    fn status(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_string());
    }
    fn warn(&self, line: &str) {
        self.lines.lock().unwrap().push(format!("warning: {line}"));
    }
    fn progress(&self, step: u64, total: u64) {
        self.progress.lock().unwrap().push((step, total));
    }
}

fn args_from(argv: &[&str]) -> RunArgs {
    match Cli::parse_from(argv).command {
        Command::Run(args) => *args,
        _ => panic!("not a run"),
    }
}

/// The parameters a run needs are an ordinary public struct. A caller that
/// never touches a command line can still produce one, and can read back
/// what it decided.
#[test]
fn run_parameters_are_public() {
    let mut args = args_from(&[
        "rda-ensemble", "run", "--state", "NC", "--plan-type", "congress",
        "--cycle", "2020", "--elections", "E_16-20_COMP", "--variant",
        "cut-edges-ust", "--steps", "100", "--tolerance", "0.05",
        "--seed-tolerance", "0.01", "--rng-seed", "11", "--out", "/tmp/x",
    ]);
    assert_eq!(args.state, "NC");
    assert_eq!(args.rng_seed, 11);
    assert_eq!(args.chains, 1);
    assert_eq!(args.segment_steps, None);

    // Writable too, so a front end can set a field without rebuilding an
    // argv and reparsing it.
    args.districts = Some(9);
    args.segment_steps = Some(500);
    assert_eq!(run::steps(&args).unwrap(), 100);
}

/// Narration reaches the caller instead of stderr. This is the seam a GUI
/// needs: the same call that prints at a terminal hands a window its text.
#[test]
fn narration_goes_to_the_sink() {
    let args = args_from(&[
        "rda-ensemble", "run", "--state", "NC", "--plan-type", "congress",
        // NC has 14; asking for 9 is legitimate and must be reported.
        "--districts", "9",
        "--cycle", "2020", "--elections", "E_16-20_COMP", "--variant",
        "cut-edges-ust", "--steps", "100", "--tolerance", "0.05",
        "--seed-tolerance", "0.01", "--rng-seed", "11", "--out", "/tmp/x",
    ]);
    let captured = Arc::new(Captured::default());
    let ev: Sink = captured.clone();

    assert_eq!(run::districts(&args, &ev).unwrap(), 9);

    let lines = captured.lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "expected exactly one warning, got {lines:?}");
    assert!(
        lines[0].contains("has 14 districts") && lines[0].contains("says 9"),
        "the warning should name both counts: {}",
        lines[0]
    );
}

/// A caller that wants only the files supplies [`Silent`] and gets no
/// narration at all.
#[test]
fn a_silent_sink_says_nothing() {
    let args = args_from(&[
        "rda-ensemble", "run", "--state", "NC", "--plan-type", "congress",
        "--districts", "9", "--cycle", "2020", "--elections", "E_16-20_COMP",
        "--variant", "cut-edges-ust", "--steps", "100", "--tolerance", "0.05",
        "--seed-tolerance", "0.01", "--rng-seed", "11", "--out", "/tmp/x",
    ]);
    let ev: Sink = Arc::new(Silent);
    assert_eq!(run::districts(&args, &ev).unwrap(), 9);
}

/// The stop button. Nothing here runs a chain, but the handle a GUI would
/// wire to one has to be reachable and shareable.
#[test]
fn cancellation_is_reachable_from_outside() {
    let cancel = Cancel::default();
    assert!(!cancel.stopped());
    let theirs = cancel.clone();
    std::thread::spawn(move || theirs.stop()).join().unwrap();
    assert!(cancel.stopped(), "a clone must stop the original");
}

/// How a front end fills its menus: the data, not the formatted lines.
#[test]
fn segment_planning_is_public() {
    assert_eq!(run::segments(100, None), vec![(0, 100)]);
    let plan = run::segments(100, Some(30));
    assert_eq!(plan.len(), 4);
    assert_eq!(plan[0], (0, 31));
}
