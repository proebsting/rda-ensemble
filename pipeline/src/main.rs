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
mod diagnostics;
mod dra;
mod manifest;
mod run;
mod scoring;
mod settings;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use artifacts::Artifact;

/// Which chamber a plan is for. These are the three rdarust knows a
/// statutory district count for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Chamber {
    /// The state's delegation to the US House.
    Congress,
    /// The upper house of the state legislature, usually the senate.
    Upper,
    /// The lower house, where the state has one.
    Lower,
}

impl Chamber {
    pub fn as_str(self) -> &'static str {
        match self {
            Chamber::Congress => "congress",
            Chamber::Upper => "upper",
            Chamber::Lower => "lower",
        }
    }
}

/// Where precinct adjacency comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Adjacency {
    /// DRA's graph when one is present, the geometry otherwise. Compares
    /// the two when both are available.
    Auto,
    /// DRA's published graph. Fails when there is none.
    Dra,
    /// Derived from the precinct shapes, ignoring any published graph.
    Geometry,
}

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
    /// Non-reversible; cut-edge pairs, trees by random minimum weight
    /// (ReCom-A).
    CutEdgesMst,
    /// Non-reversible; random pairs, trees by random minimum weight
    /// (ReCom-B).
    DistrictPairsMst,
    /// Region-aware: cut-edge pairs, and cuts that keep counties whole are
    /// preferred. Requires --county-weight.
    CutEdgesRegionAware,
    /// Region-aware: random pairs, and cuts that keep counties whole are
    /// preferred. Requires --county-weight.
    DistrictPairsRegionAware,
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
        /// A GeoJSON to read. Left out, --state says which to download.
        #[arg(value_name = "FILE")]
        geojson: Option<PathBuf>,
        /// Two-letter state abbreviation, downloaded from DRA if not cached.
        #[arg(long, value_name = "XX", required_unless_present = "geojson")]
        state: Option<String>,
        /// Which DRA version. The newest is used when this is left out.
        #[arg(long = "dra-version", value_name = "vNN")]
        dra_version: Option<String>,
        /// Where packages are kept.
        #[arg(long, value_name = "DIR")]
        cache: Option<PathBuf>,
    },
    /// List the states DRA publishes data for, and which versions exist.
    States {
        /// Just this state.
        #[arg(long, value_name = "XX")]
        state: Option<String>,
        /// Where the listing is cached. Re-read for a day before asking
        /// GitHub again.
        #[arg(long, value_name = "DIR")]
        cache: Option<PathBuf>,
    },
    /// Download a state's data from DRA and unpack it.
    Fetch {
        /// Two-letter state abbreviation.
        #[arg(long, value_name = "XX")]
        state: String,
        /// Which DRA version. The newest is used when this is left out.
        #[arg(long = "dra-version", value_name = "vNN")]
        dra_version: Option<String>,
        /// Where packages are kept. Defaults to a per-user cache, so the
        /// same state is downloaded once however many ensembles you build.
        #[arg(long, value_name = "DIR")]
        cache: Option<PathBuf>,
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
    /// The DRA GeoJSON for one state. Left out, the state's data is
    /// downloaded from DRA and kept in --cache.
    #[arg(long, value_name = "FILE", help_heading = "Input")]
    pub geojson: Option<PathBuf>,

    /// Which DRA version to download. The newest is used when this is left
    /// out. Ignored when --geojson names a file.
    #[arg(long = "dra-version", value_name = "vNN", help_heading = "Input")]
    pub dra_version: Option<String>,

    /// Where downloaded packages are kept, so a second run does not
    /// download again.
    #[arg(long, value_name = "DIR", help_heading = "Input")]
    pub cache: Option<PathBuf>,

    /// Where precinct adjacency comes from. DRA ships a graph beside each
    /// GeoJSON; `auto` uses it when there is one and derives from the
    /// geometry when there is not, reporting any disagreement.
    #[arg(long, value_enum, default_value_t = Adjacency::Auto, help_heading = "Input")]
    pub adjacency: Adjacency,

    /// Two-letter state abbreviation, e.g. NC.
    #[arg(long, value_name = "XX", help_heading = "Input")]
    pub state: String,

    /// Which chamber the plan is for. Fixes how many districts to draw,
    /// unless --districts says otherwise.
    #[arg(long = "plan-type", value_enum, value_name = "NAME", help_heading = "Input")]
    pub chamber: Chamber,

    /// How many districts to draw. Defaults to the number this state's
    /// chamber actually has, so most runs leave it out.
    #[arg(long, value_name = "N", help_heading = "Input")]
    pub districts: Option<usize>,

    // ---- which data to pull out of it -------------------------------------
    /// Census cycle, e.g. 2020. Picks the population, voting-age and
    /// citizen voting-age datasets for that year out of the GeoJSON, so
    /// most runs need nothing else here.
    #[arg(
        long,
        value_name = "YEAR",
        required_unless_present_all = ["census", "vap", "cvap"],
        help_heading = "Datasets"
    )]
    pub cycle: Option<i64>,

    /// Census dataset name, overriding --cycle's choice. e.g. T_20_CENS.
    #[arg(long, value_name = "NAME", help_heading = "Datasets")]
    pub census: Option<String>,

    /// Voting-age population dataset, overriding --cycle's choice.
    #[arg(long, value_name = "NAME", help_heading = "Datasets")]
    pub vap: Option<String>,

    /// Citizen voting-age population dataset, overriding --cycle's choice.
    #[arg(long, value_name = "NAME", help_heading = "Datasets")]
    pub cvap: Option<String>,

    /// Election datasets to score, comma separated. Use `all` for every
    /// election the GeoJSON carries.
    #[arg(
        long,
        value_name = "LIST",
        value_delimiter = ',',
        required = true,
        help_heading = "Datasets"
    )]
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
    /// Chain steps to run, counting rejected proposals. One of --steps or
    /// --plans; --plans is usually what you mean.
    #[arg(
        long,
        value_name = "N",
        required_unless_present = "plans",
        conflicts_with = "plans",
        help_heading = "Chain"
    )]
    pub steps: Option<u64>,

    /// How many plans you want in the ensemble. The chain is run for as
    /// many steps as that needs: --plans 10000 --sample-every 200 runs two
    /// million steps.
    #[arg(long, value_name = "N", help_heading = "Chain")]
    pub plans: Option<u64>,

    /// Which ReCom variant to run.
    #[arg(long, value_enum, help_heading = "Chain")]
    pub variant: Variant,

    /// How far a district's population may sit from ideal during the chain,
    /// as a fraction. Usually looser than --seed-tolerance.
    #[arg(long, value_name = "FRACTION", help_heading = "Chain")]
    pub tolerance: f64,

    /// How many independent chains to run, each from its own starting plan
    /// and its own derived seed. More than one lets R-hat say whether they
    /// agree about the distribution, which one chain cannot. Four is the
    /// usual choice. They run in parallel.
    #[arg(long, value_name = "N", default_value_t = 1, help_heading = "Chain")]
    pub chains: usize,

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

    /// Prefer cuts that keep a region whole, as `COLUMN=WEIGHT`. Repeatable
    /// and comma-separated; higher weights matter more. `COUNTY=1.0` is the
    /// usual one, and COUNTY is the column the dual graph always carries.
    /// Required by, and only used by, the region-aware variants.
    #[arg(
        long,
        value_name = "COL=W",
        value_delimiter = ',',
        required_if_eq_any([
            ("variant", "cut-edges-region-aware"),
            ("variant", "district-pairs-region-aware"),
        ]),
        help_heading = "Chain"
    )]
    pub region_weights: Vec<String>,

    /// Ideal population per district. Defaults to the total divided by the
    /// district count, which is what you want unless you are deliberately
    /// balancing against something else.
    #[arg(long, value_name = "N", help_heading = "Chain")]
    pub target_pop: Option<u64>,

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

    /// Never show the progress bar. It is shown by default when stderr is
    /// a terminal, because a long chain is otherwise indistinguishable from
    /// a hung one.
    #[arg(long, help_heading = "Output")]
    pub no_progress: bool,

    /// Work out every setting, print them, and stop without running the
    /// chain. Reads the GeoJSON, so it also catches a bad dataset name.
    #[arg(long, help_heading = "Output")]
    pub dry_run: bool,
}

impl Variant {
    /// The spelling `--variant` accepts, so the manifest can be pasted back
    /// into a command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Reversible => "reversible",
            Variant::CutEdgesUst => "cut-edges-ust",
            Variant::DistrictPairsUst => "district-pairs-ust",
            Variant::CutEdgesMst => "cut-edges-mst",
            Variant::DistrictPairsMst => "district-pairs-mst",
            Variant::CutEdgesRegionAware => "cut-edges-region-aware",
            Variant::DistrictPairsRegionAware => "district-pairs-region-aware",
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
        Command::Datasets { geojson, state, dra_version, cache } => {
            let path = match geojson {
                Some(p) => p,
                None => {
                    let state = state.expect("clap requires one or the other");
                    let cache = cache.unwrap_or_else(dra::default_cache);
                    dra::resolve(&cache, &state, dra_version.as_deref())?.geojson
                }
            };
            datasets::list(&path)
        }
        Command::States { state, cache } => {
            let inv = dra::Inventory::load(&cache.unwrap_or_else(dra::default_cache))?;
            dra::list(&inv, state.as_deref())
        }
        Command::Fetch { state, dra_version, cache } => {
            let cache = cache.unwrap_or_else(dra::default_cache);
            let pkg = dra::resolve(&cache, &state, dra_version.as_deref())?;
            eprintln!("  {}", pkg.geojson.display());
            if let Some(g) = &pkg.graph {
                eprintln!("  {}", g.display());
            }
            Ok(())
        }
        Command::Run(args) => {
            let keep = KeepArg::expand(&args.keep);
            run::run(&args, &keep)
        }
    }
}
