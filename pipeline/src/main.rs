//! Generate and score a redistricting ensemble from a DRA GeoJSON.
//!
//! One binary, one process, one command. It links rdarust for extraction and
//! scoring, partigraph for the seed plan, and rustrecom for the chain, and
//! passes data structures between them rather than files.
//!
//! Options have defaults only where a default is unarguable. Dataset names,
//! district counts, tolerances and chain parameters all change the answer, so
//! the run has to say what it wants.

mod artifacts;
mod datasets;
mod manifest;
mod run;
mod scoring;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use artifacts::Artifact;

/// Which ReCom variant to run. The names match rustrecom's `--variant`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Variant {
    /// Reversible ReCom. Samples the intended distribution exactly, at the
    /// cost of rejecting most proposals. Requires `--balance-ub`.
    Reversible,
    /// Non-reversible; district pairs chosen by a random cut edge, spanning
    /// trees uniform. The usual choice when reversibility is not needed.
    CutEdgesUst,
    /// Non-reversible; district pairs chosen at random until adjacent.
    DistrictPairsUst,
    /// Non-reversible; cut-edge pairs, trees by random minimum weight.
    CutEdgesRmst,
    /// Non-reversible; random pairs, trees by random minimum weight.
    DistrictPairsRmst,
}

/// Redistricting ensembles from a Dave's Redistricting GeoJSON.
#[derive(Debug, Parser)]
#[command(name = "rda-ensemble", version, max_term_width = 100)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// List the datasets a GeoJSON carries, with what each one is.
    ///
    /// Start here. The names this prints are what `run` wants for
    /// --census, --vap, --cvap and --elections.
    Datasets {
        /// The DRA GeoJSON for one state.
        #[arg(value_name = "FILE")]
        geojson: PathBuf,
    },
    /// Generate an ensemble and score every plan.
    ///
    /// Reads one GeoJSON, builds the precinct graph, draws a
    /// population-balanced starting plan, runs a ReCom chain, and scores
    /// plans as the chain produces them. Nothing is written between stages
    /// unless --keep asks for it.
    Run(Box<RunArgs>),
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    // ---- what to read -----------------------------------------------------
    /// The DRA GeoJSON for one state.
    #[arg(long, value_name = "FILE", help_heading = "Input")]
    pub geojson: PathBuf,

    /// Two-letter state abbreviation, e.g. NC.
    #[arg(long, value_name = "XX", help_heading = "Input")]
    pub state: String,

    /// Which chamber the plan is for. Sets the statutory district count
    /// rdarust scores against.
    #[arg(long, value_name = "NAME", help_heading = "Input")]
    pub plan_type: String,

    /// How many districts to draw.
    #[arg(long, value_name = "N", help_heading = "Input")]
    pub districts: usize,

    // ---- which data to pull out of it -------------------------------------
    /// Census dataset name in the GeoJSON, e.g. T_20_CENS.
    #[arg(long, value_name = "NAME", help_heading = "Datasets")]
    pub census: String,

    /// Voting-age population dataset, e.g. V_20_VAP.
    #[arg(long, value_name = "NAME", help_heading = "Datasets")]
    pub vap: String,

    /// Citizen voting-age population dataset, e.g. V_20_CVAP.
    #[arg(long, value_name = "NAME", help_heading = "Datasets")]
    pub cvap: String,

    /// Election datasets to score, comma separated. Use `all` for every
    /// election the GeoJSON carries.
    #[arg(long, value_name = "LIST", value_delimiter = ',', help_heading = "Datasets")]
    pub elections: Vec<String>,

    /// Also score each election that a composite averages, on its own.
    #[arg(long, help_heading = "Datasets")]
    pub expand_composites: bool,

    // ---- the starting plan ------------------------------------------------
    /// How far a district's population may sit from ideal when drawing the
    /// starting plan, as a fraction. 0.01 means within one percent.
    #[arg(long, value_name = "FRACTION", help_heading = "Starting plan")]
    pub seed_tolerance: f64,

    // ---- the chain --------------------------------------------------------
    /// Chain steps to run, counting rejected proposals.
    #[arg(long, value_name = "N", help_heading = "Chain")]
    pub steps: u64,

    /// Which ReCom variant to run.
    #[arg(long, value_enum, help_heading = "Chain")]
    pub variant: Variant,

    /// How far a district's population may sit from ideal during the chain,
    /// as a fraction. Usually looser than --seed-tolerance.
    #[arg(long, value_name = "FRACTION", help_heading = "Chain")]
    pub tolerance: f64,

    /// Seed for the random number generator. The same seed, GeoJSON and
    /// parameters reproduce the same ensemble.
    #[arg(long, value_name = "N", help_heading = "Chain")]
    pub rng_seed: u64,

    /// Normalizing constant M for reversible ReCom. Required for, and only
    /// used by, --variant reversible.
    #[arg(
        long,
        value_name = "N",
        required_if_eq("variant", "reversible"),
        help_heading = "Chain"
    )]
    pub balance_ub: Option<u32>,

    /// Worker threads for the chain. More than one is faster but changes
    /// which plans a given --rng-seed produces, so a reproducible run keeps
    /// this at 1.
    #[arg(long, value_name = "N", default_value_t = 1, help_heading = "Chain")]
    pub threads: usize,

    /// Chain steps per unit of threaded work. Only matters with --threads
    /// above 1; large batches suit variants that reject most proposals.
    #[arg(long, value_name = "N", default_value_t = 1, help_heading = "Chain")]
    pub batch_size: usize,

    // ---- what to keep -----------------------------------------------------
    /// Score every Nth chain step. A step that rejects its proposal leaves
    /// the plan unchanged and is sampled again, which is what keeps the
    /// ensemble's distribution right.
    #[arg(long, value_name = "N", default_value_t = 1, help_heading = "Output")]
    pub sample_every: u64,

    /// Directory for the output. Created if it does not exist.
    #[arg(long, value_name = "DIR", help_heading = "Output")]
    pub out: PathBuf,

    /// Also write an intermediate that would otherwise stay in memory.
    /// Repeatable; `--keep all` writes every one.
    #[arg(long, value_name = "WHAT", help_heading = "Output")]
    pub keep: Vec<KeepArg>,

    /// Prefix every score column with the dataset it came from.
    #[arg(long, help_heading = "Output")]
    pub prefixes: bool,

    /// Also report county splitting under Don Leake's reverse weighting,
    /// as two extra columns.
    #[arg(long, help_heading = "Output")]
    pub reverse_weight_splitting: bool,

    /// Report chain progress to stderr.
    #[arg(long, help_heading = "Output")]
    pub progress: bool,
}

impl Variant {
    /// The spelling `--variant` accepts, so the manifest can be pasted back
    /// into a command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Reversible => "reversible",
            Variant::CutEdgesUst => "cut-edges-ust",
            Variant::DistrictPairsUst => "district-pairs-ust",
            Variant::CutEdgesRmst => "cut-edges-rmst",
            Variant::DistrictPairsRmst => "district-pairs-rmst",
        }
    }
}

/// `--keep` takes any one artifact, or `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum KeepArg {
    All,
    DataMap,
    Graph,
    Data,
    RecomGraph,
    SeedPlan,
    Plans,
}

impl KeepArg {
    fn expand(args: &[KeepArg]) -> Vec<Artifact> {
        if args.contains(&KeepArg::All) {
            return Artifact::ALL.to_vec();
        }
        args.iter()
            .filter_map(|a| match a {
                KeepArg::All => None,
                KeepArg::DataMap => Some(Artifact::DataMap),
                KeepArg::Graph => Some(Artifact::Graph),
                KeepArg::Data => Some(Artifact::Data),
                KeepArg::RecomGraph => Some(Artifact::RecomGraph),
                KeepArg::SeedPlan => Some(Artifact::SeedPlan),
                KeepArg::Plans => Some(Artifact::Plans),
            })
            .collect()
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = real_main(cli) {
        eprintln!("rda-ensemble: {e:#}");
        std::process::exit(1);
    }
}

fn real_main(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Datasets { geojson } => datasets::list(&geojson),
        Command::Run(args) => {
            let keep = KeepArg::expand(&args.keep);
            run::run(&args, &keep)
        }
    }
}
