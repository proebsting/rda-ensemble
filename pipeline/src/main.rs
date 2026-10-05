//! The command line. Everything it does lives in the library beside it, so
//! another front end can do the same things without a terminal.

use std::sync::Arc;

use clap::Parser;
use rda_ensemble::events::{Sink, Term};
use rda_ensemble::{real_main, Cli, Command};

fn main() {
    let cli = Cli::parse();
    // The bar is drawn for a single chain at a terminal; several chains
    // writing one line would interleave into nonsense.
    let progress = match &cli.command {
        Command::Run(args) => !args.no_progress && args.chains == 1,
        _ => false,
    };
    let events: Sink = Arc::new(Term::new(progress));
    if let Err(e) = real_main(cli, &events) {
        events.progress_done();
        eprintln!("rda-ensemble: {e:#}");
        std::process::exit(1);
    }
}
