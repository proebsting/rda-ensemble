//! A run, as a file you can hand to someone else.
//!
//! Everything that decides what an ensemble *is*, and nothing that decides
//! where it lands on one particular machine. A settings file carries no
//! GeoJSON path, no cache directory and no output directory: those differ
//! between people, and a file that carried them would not survive being
//! emailed. What it carries instead is the state and the DRA version, which
//! is enough for anyone to fetch the same data.
//!
//! Two things can be read back. A settings file, obviously. But also a
//! `manifest.json` from someone's output directory -- which is what people
//! will actually have when they want to repeat a colleague's run, since it
//! is written beside the results whether or not anyone thought to save
//! settings. Both land in the same struct.
//!
//! This is also the shape a front end holds while the user fills a form:
//! [`Settings`] in, [`crate::RunArgs`] out, so the file cannot drift from
//! what the form collects.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Adjacency, Chamber, KeepArg, RunArgs, Variant};

/// Bumped only if an older file would be read wrongly rather than merely
/// incompletely. A new optional field does not need it.
pub const VERSION: u32 = 1;

/// Segment length a form starts with, so a run can always be stopped.
///
/// A chain can only be interrupted between segments, so a form that left this
/// blank would hand people a Stop button that never worked. The command line
/// keeps its own default of none: changing that would alter every ensemble
/// ever produced by it.
///
/// Five thousand is chosen for the *slowest* configuration rather than the
/// average. Throughput depends on how many precincts a merged pair spans,
/// which is roughly twice the precincts per district -- so fewer districts
/// run slower, not faster. Measured on an M1 Pro: Michigan at 4 districts
/// manages about 1,200 steps a second, Illinois at 17 about 1,700, Michigan
/// at 13 about 3,400. Five thousand steps is therefore four seconds at worst
/// and usually less. Segmenting itself costs nothing measurable: the same
/// fifty thousand steps took 42.0s chopped and 42.3s whole.
///
/// It is a fixed number rather than one derived from timing on purpose. The
/// segment length changes which plans come out, so a value measured from the
/// machine would make the same settings produce different ensembles on
/// different computers. A constant travels with the settings file.
pub const FORM_SEGMENT_STEPS: u64 = 5_000;

/// A decision the run needs and has not been given.
///
/// Reported rather than merely refused, so a form can say what is still
/// wanted before anyone presses anything. `field` names the setting; the
/// front end decides whether that means a red outline or a sentence.
#[derive(Debug, Clone, Serialize)]
pub struct Missing {
    pub field: String,
    pub says: String,
}

fn want(field: &str, says: &str) -> Missing {
    Missing { field: field.into(), says: says.into() }
}

/// Everything a run decides, portable between machines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Format version of this file.
    #[serde(default = "default_version")]
    pub version: u32,

    // ---- what to read ---------------------------------------------------
    /// Two-letter state abbreviation.
    pub state: String,
    /// `congress`, `upper` or `lower`.
    pub plan_type: String,
    /// How many districts, when not the chamber's statutory count.
    #[serde(default)]
    pub districts: Option<usize>,
    /// Which DRA package. Null means whatever is newest, which is not
    /// reproducible -- a settings file worth sharing names one.
    #[serde(default)]
    pub dra_version: Option<String>,
    /// `auto`, `dra` or `geometry`.
    #[serde(default = "default_adjacency")]
    pub adjacency: String,

    // ---- which data -----------------------------------------------------
    #[serde(default)]
    pub cycle: Option<i64>,
    #[serde(default)]
    pub census: Option<String>,
    #[serde(default)]
    pub vap: Option<String>,
    #[serde(default)]
    pub cvap: Option<String>,
    pub elections: Vec<String>,
    #[serde(default)]
    pub expand_composites: bool,

    // ---- the chain ------------------------------------------------------
    pub seed_tolerance: f64,
    #[serde(default)]
    pub steps: Option<u64>,
    #[serde(default)]
    pub plans: Option<u64>,
    pub sample_every: u64,
    pub variant: String,
    pub tolerance: f64,
    pub rng_seed: u64,
    #[serde(default = "default_chains")]
    pub chains: usize,
    #[serde(default)]
    pub balance_ub: Option<u32>,
    #[serde(default)]
    pub region_weights: Vec<String>,
    #[serde(default)]
    pub target_pop: Option<u64>,
    /// Load-bearing: each segment draws its own derived seed, so two runs
    /// that differ only here are different chains.
    #[serde(default)]
    pub segment_steps: Option<u64>,
    #[serde(default = "default_threads")]
    pub threads: usize,
    #[serde(default = "default_threads")]
    pub batch_size: usize,

    // ---- what to write --------------------------------------------------
    /// Intermediates to keep: `data-map`, `graph`, `data`, `recom-graph`,
    /// `seed-plan`, `plans`, or `all`.
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub prefixes: bool,
    #[serde(default)]
    pub reverse_weight_splitting: bool,
}

fn default_version() -> u32 {
    VERSION
}
fn default_adjacency() -> String {
    "auto".into()
}
fn default_chains() -> usize {
    1
}
fn default_threads() -> usize {
    1
}

impl Settings {
    /// The values a form should start with.
    ///
    /// Read out of clap rather than written again in whatever language the
    /// form is in, so a front end and the command line cannot drift apart
    /// about what `chains` or `sample_every` default to. Only the genuinely
    /// defaulted fields are meaningful; the rest are placeholders the form
    /// is expected to overwrite, and `into_args` refuses them if it does not.
    pub fn starting_point() -> Settings {
        use clap::Parser;
        let argv = [
            "rda-ensemble", "run", "--state", "", "--plan-type", "congress",
            "--elections", "", "--variant", "cut-edges-ust", "--steps", "0",
            "--tolerance", "0", "--seed-tolerance", "0", "--rng-seed", "0",
            "--cycle", "0", "--out", ".",
        ];
        match crate::Cli::parse_from(argv).command {
            crate::Command::Run(args) => {
                let mut s = Settings::from_args(&args);
                // Placeholders, not choices: blank them so a form cannot
                // present them as if they had been decided. The chamber and
                // the variant go too -- those are only what the argv above
                // had to say to parse, not defaults anyone chose.
                s.cycle = None;
                s.steps = None;
                s.elections = Vec::new();
                s.plan_type = String::new();
                s.variant = String::new();
                // Not a clap default, and deliberately so: see the constant.
                s.segment_steps = Some(FORM_SEGMENT_STEPS);
                s
            }
            _ => unreachable!("that argv is a run"),
        }
    }

    /// The settings a set of run parameters represents, dropping the three
    /// paths that are specific to one machine.
    pub fn from_args(a: &RunArgs) -> Settings {
        Settings {
            version: VERSION,
            state: a.state.clone(),
            plan_type: a.chamber.as_str().to_string(),
            districts: a.districts,
            dra_version: a.dra_version.clone(),
            adjacency: adjacency_str(a.adjacency).to_string(),
            cycle: a.cycle,
            census: a.census.clone(),
            vap: a.vap.clone(),
            cvap: a.cvap.clone(),
            elections: a.elections.clone(),
            expand_composites: a.expand_composites,
            seed_tolerance: a.seed_tolerance,
            steps: a.steps,
            plans: a.plans,
            sample_every: a.sample_every,
            variant: a.variant.as_str().to_string(),
            tolerance: a.tolerance,
            rng_seed: a.rng_seed,
            chains: a.chains,
            balance_ub: a.balance_ub,
            region_weights: a.region_weights.clone(),
            target_pop: a.target_pop,
            segment_steps: a.segment_steps,
            threads: a.threads,
            batch_size: a.batch_size,
            keep: a.keep.iter().map(|k| k.as_str().to_string()).collect(),
            prefixes: a.prefixes,
            reverse_weight_splitting: a.reverse_weight_splitting,
        }
    }

    /// Every decision still wanted, in the order a form presents them.
    ///
    /// This is the one place the rules live. [`Settings::into_args`] refuses
    /// a run by calling it, and a front end shows the same list as it is
    /// filled in, so what the form asks for and what the run requires cannot
    /// drift apart.
    ///
    /// Note what is *not* here. `rng_seed` of zero is a perfectly good seed,
    /// so nothing in the data distinguishes "unset" from "zero"; a form that
    /// wants to insist on a deliberate choice has to notice its own blank
    /// field. The same goes for an output directory, which these settings do
    /// not carry at all.
    pub fn missing(&self) -> Vec<Missing> {
        let mut out = Vec::new();
        if self.state.trim().is_empty() {
            out.push(want("state", "Which state the plan is for."));
        }
        // Congressional, upper or lower is a real choice with no obviously
        // right answer, so nothing should pick it for you.
        if !["congress", "upper", "lower"].contains(&self.plan_type.as_str()) {
            out.push(want("plan_type", "Which chamber the plan is for."));
        }
        if self.cycle.is_none()
            && (self.census.is_none() || self.vap.is_none() || self.cvap.is_none())
        {
            out.push(want(
                "cycle",
                "A census cycle, or all three of census, voting-age and citizen \
                 voting-age datasets.",
            ));
        }
        if self.elections.is_empty() {
            out.push(want("elections", "At least one election to score."));
        }
        if self.seed_tolerance <= 0.0 {
            out.push(want(
                "seed_tolerance",
                "How balanced the starting plan must be, as a fraction above zero.",
            ));
        }
        if self.steps.is_none() && self.plans.is_none() {
            out.push(want("plans", "How many plans you want, or how many steps to run."));
        }
        // Seven variants, no default. cut-edges-ust is what most ensembles
        // use, but "most" is not "obviously", and which one ran changes what
        // the ensemble means.
        if !VARIANTS.contains(&self.variant.as_str()) {
            out.push(want(
                "variant",
                "How the chain proposes changes. cut-edges-ust is the usual choice.",
            ));
        }
        if self.tolerance <= 0.0 {
            out.push(want(
                "tolerance",
                "How far a district's population may sit from ideal during the \
                 chain, as a fraction above zero.",
            ));
        }
        if self.sample_every == 0 {
            out.push(want("sample_every", "Sampling interval, at least 1."));
        }
        // Two variants need a parameter nothing else does, and the run fails
        // deep inside rustrecom without it.
        if self.variant == "reversible" && self.balance_ub.is_none() {
            out.push(want(
                "balance_ub",
                "Reversible ReCom needs a balance upper bound.",
            ));
        }
        if self.variant.ends_with("region-aware") && self.region_weights.is_empty() {
            out.push(want(
                "region_weights",
                "A region-aware variant needs a weight, such as COUNTY=1.0.",
            ));
        }
        out
    }

    /// Run parameters for these settings, writing into `out`.
    ///
    /// `out` and `cache` come from the caller because they are the two
    /// things the file deliberately does not carry.
    pub fn into_args(self, out: PathBuf, cache: Option<PathBuf>) -> Result<RunArgs> {
        if self.version > VERSION {
            bail!(
                "these settings are version {} and this is rda-ensemble {}, \
                 which understands up to version {VERSION}. Use a newer build.",
                self.version,
                env!("CARGO_PKG_VERSION"),
            );
        }
        let wanted = self.missing();
        if !wanted.is_empty() {
            bail!(
                "these settings are incomplete:\n{}",
                wanted
                    .iter()
                    .map(|m| format!("  {}: {}", m.field, m.says))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        Ok(RunArgs {
            geojson: None,
            dra_version: self.dra_version,
            cache,
            adjacency: parse_adjacency(&self.adjacency)?,
            state: self.state,
            chamber: parse_chamber(&self.plan_type)?,
            districts: self.districts,
            cycle: self.cycle,
            census: self.census,
            vap: self.vap,
            cvap: self.cvap,
            elections: self.elections,
            expand_composites: self.expand_composites,
            seed_tolerance: self.seed_tolerance,
            steps: self.steps,
            plans: self.plans,
            variant: parse_variant(&self.variant)?,
            tolerance: self.tolerance,
            chains: self.chains,
            rng_seed: self.rng_seed,
            balance_ub: self.balance_ub,
            region_weights: self.region_weights,
            target_pop: self.target_pop,
            segment_steps: self.segment_steps,
            threads: self.threads,
            batch_size: self.batch_size,
            sample_every: self.sample_every,
            out,
            keep: self
                .keep
                .iter()
                .map(|k| parse_keep(k))
                .collect::<Result<Vec<_>>>()?,
            prefixes: self.prefixes,
            reverse_weight_splitting: self.reverse_weight_splitting,
            no_progress: false,
            dry_run: false,
        })
    }

    /// Settings from a `manifest.json`, so a run can be repeated from
    /// someone's output directory rather than from a file they remembered
    /// to save.
    ///
    /// The manifest records what was *resolved*, so this is stricter than
    /// the command line that produced it: the datasets are named outright
    /// rather than left to `cycle`, and the elections are the expanded list
    /// rather than `all`. That is the point -- it pins the run.
    pub fn from_manifest(doc: &Value) -> Result<Settings> {
        let input = doc
            .get("input")
            .ok_or_else(|| anyhow!("no `input` section; is this an rda-ensemble manifest?"))?;
        let chain = doc
            .get("chain")
            .ok_or_else(|| anyhow!("no `chain` section; is this an rda-ensemble manifest?"))?;
        let output = doc.get("output").unwrap_or(&Value::Null);

        let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let u = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_u64());
        let f = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_f64());
        let b = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);

        let state = s(input, "state")
            .ok_or_else(|| anyhow!("the manifest does not name a state"))?;
        let plan_type = s(input, "plan_type")
            .ok_or_else(|| anyhow!("the manifest does not name a plan type"))?;

        Ok(Settings {
            version: VERSION,
            state,
            plan_type,
            districts: u(input, "districts").map(|n| n as usize),
            // "local" means the run read a file rather than a package, so
            // there is no version to pin and the reader must choose.
            dra_version: s(input, "dra_version").filter(|v| v != "local"),
            adjacency: s(input, "adjacency").unwrap_or_else(default_adjacency),
            cycle: input.get("cycle").and_then(|x| x.as_i64()),
            census: s(input, "census"),
            vap: s(input, "vap"),
            cvap: s(input, "cvap"),
            elections: input
                .get("elections")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            expand_composites: b(input, "expand_composites"),
            seed_tolerance: f(chain, "seed_tolerance")
                .ok_or_else(|| anyhow!("the manifest does not record seed_tolerance"))?,
            steps: u(chain, "steps_requested"),
            plans: None,
            sample_every: u(output, "sample_every").unwrap_or(1),
            variant: s(chain, "variant")
                .ok_or_else(|| anyhow!("the manifest does not record a variant"))?,
            tolerance: f(chain, "tolerance")
                .ok_or_else(|| anyhow!("the manifest does not record a tolerance"))?,
            rng_seed: u(chain, "rng_seed")
                .ok_or_else(|| anyhow!("the manifest does not record an rng_seed"))?,
            // Chains are not in the manifest: each chain writes its own
            // directory and the manifest describes the run as a whole.
            chains: 1,
            balance_ub: u(chain, "balance_ub").map(|n| n as u32),
            region_weights: chain
                .get("region_weights")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            target_pop: u(chain, "target_pop"),
            segment_steps: u(chain, "segment_steps"),
            threads: u(chain, "threads").unwrap_or(1) as usize,
            batch_size: u(chain, "batch_size").unwrap_or(1) as usize,
            keep: output
                .get("kept")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            prefixes: b(output, "prefixed_columns"),
            reverse_weight_splitting: b(output, "reverse_weight_splitting"),
        })
    }

    /// Read a settings file, or a manifest, whichever it is.
    ///
    /// A user handed a JSON file by a colleague should not have to know
    /// which of the two it is, and the two are told apart by their shape
    /// rather than by their name.
    pub fn read(path: &std::path::Path) -> Result<Settings> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Settings::parse(&text)
            .with_context(|| format!("reading {}", path.display()))
    }

    /// As [`Settings::read`], from text already in hand.
    pub fn parse(text: &str) -> Result<Settings> {
        let doc: Value = serde_json::from_str(text).context("this is not JSON")?;
        if doc.get("tool").and_then(|t| t.as_str()) == Some("rda-ensemble")
            && doc.get("input").is_some()
        {
            return Settings::from_manifest(&doc);
        }
        serde_json::from_value(doc).context(
            "this is neither an rda-ensemble settings file nor a manifest.json",
        )
    }

    /// Pretty JSON, newline-terminated, as the file is written.
    pub fn to_json(&self) -> String {
        let mut out = serde_json::to_string_pretty(self).expect("settings serialise");
        out.push('\n');
        out
    }

    /// Anything that would stop this being reproducible by someone else.
    ///
    /// These are warnings rather than errors: a run is still a run. But a
    /// file meant for sharing should say so before it is shared.
    pub fn portability_warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dra_version.is_none() {
            out.push(
                "No DRA version: whoever runs this gets whatever is newest, \
                 which may not be what you used."
                    .into(),
            );
        }
        if self.threads > 1 {
            out.push(format!(
                "threads is {}: above 1 the chain is not reproducible, even \
                 with the same seed.",
                self.threads
            ));
        }
        if self.cycle.is_some()
            && (self.census.is_none() || self.vap.is_none() || self.cvap.is_none())
        {
            out.push(
                "The datasets come from `cycle` rather than being named. A \
                 future DRA package could resolve it differently."
                    .into(),
            );
        }
        out
    }
}

fn adjacency_str(a: Adjacency) -> &'static str {
    match a {
        Adjacency::Auto => "auto",
        Adjacency::Dra => "dra",
        Adjacency::Geometry => "geometry",
    }
}

fn parse_adjacency(s: &str) -> Result<Adjacency> {
    Ok(match s {
        "auto" => Adjacency::Auto,
        "dra" => Adjacency::Dra,
        "geometry" => Adjacency::Geometry,
        other => bail!("unknown adjacency {other:?}; expected auto, dra or geometry"),
    })
}

fn parse_chamber(s: &str) -> Result<Chamber> {
    Ok(match s {
        "congress" => Chamber::Congress,
        "upper" => Chamber::Upper,
        "lower" => Chamber::Lower,
        other => bail!("unknown plan type {other:?}; expected congress, upper or lower"),
    })
}

/// Every variant `variant` accepts, in the order the help lists them.
pub const VARIANTS: [&str; 7] = [
    "cut-edges-ust",
    "cut-edges-region-aware",
    "district-pairs-ust",
    "district-pairs-region-aware",
    "cut-edges-mst",
    "district-pairs-mst",
    "reversible",
];

fn parse_variant(s: &str) -> Result<Variant> {
    Ok(match s {
        "reversible" => Variant::Reversible,
        "cut-edges-ust" => Variant::CutEdgesUst,
        "district-pairs-ust" => Variant::DistrictPairsUst,
        "cut-edges-mst" => Variant::CutEdgesMst,
        "district-pairs-mst" => Variant::DistrictPairsMst,
        "cut-edges-region-aware" => Variant::CutEdgesRegionAware,
        "district-pairs-region-aware" => Variant::DistrictPairsRegionAware,
        other => bail!("unknown variant {other:?}"),
    })
}

fn parse_keep(s: &str) -> Result<KeepArg> {
    Ok(match s {
        "all" => KeepArg::All,
        "data-map" => KeepArg::DataMap,
        "graph" => KeepArg::Graph,
        "data" => KeepArg::Data,
        "recom-graph" => KeepArg::RecomGraph,
        "seed-plan" => KeepArg::SeedPlan,
        "plans" => KeepArg::Plans,
        other => bail!("unknown artifact {other:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// `Settings::missing` for a set of run parameters.
    trait PipeMissing {
        fn pipe_missing(&self) -> Vec<Missing>;
    }
    impl PipeMissing for RunArgs {
        fn pipe_missing(&self) -> Vec<Missing> {
            Settings::from_args(self).missing()
        }
    }

    fn args(extra: &[&str]) -> RunArgs {
        let mut argv = vec![
            "rda-ensemble", "run", "--state", "MI", "--plan-type", "congress",
            "--cycle", "2020", "--elections", "E_16-20_COMP", "--variant",
            "cut-edges-ust", "--steps", "100", "--sample-every", "1",
            "--tolerance", "0.05", "--seed-tolerance", "0.01", "--rng-seed",
            "11", "--out", "/tmp/out",
        ];
        argv.extend_from_slice(extra);
        match crate::Cli::parse_from(argv).command {
            crate::Command::Run(a) => *a,
            _ => unreachable!(),
        }
    }

    /// The point of the whole module: what goes out comes back.
    #[test]
    fn settings_round_trip_through_json() {
        let original = args(&[
            "--districts", "4", "--chains", "3", "--segment-steps", "250",
            "--dra-version", "v06", "--keep", "plans", "--keep", "seed-plan",
            "--prefixes", "--adjacency", "dra",
        ]);
        let settings = Settings::from_args(&original);
        let text = settings.to_json();
        let back = Settings::parse(&text).expect("re-read");
        assert_eq!(settings, back);

        let rebuilt = back.into_args("/tmp/out".into(), None).expect("to args");
        // Everything that decides the ensemble must survive the trip.
        assert_eq!(rebuilt.state, original.state);
        assert_eq!(rebuilt.districts, original.districts);
        assert_eq!(rebuilt.chains, original.chains);
        assert_eq!(rebuilt.segment_steps, original.segment_steps);
        assert_eq!(rebuilt.rng_seed, original.rng_seed);
        assert_eq!(rebuilt.variant.as_str(), original.variant.as_str());
        assert_eq!(rebuilt.tolerance, original.tolerance);
        assert_eq!(rebuilt.seed_tolerance, original.seed_tolerance);
        assert_eq!(rebuilt.elections, original.elections);
        assert_eq!(rebuilt.keep.len(), original.keep.len());
        assert_eq!(rebuilt.prefixes, original.prefixes);
        assert_eq!(rebuilt.dra_version, original.dra_version);
    }

    /// A settings file must not carry one machine's paths, or it cannot be
    /// shared, which is the only reason it exists.
    #[test]
    fn no_local_paths_are_written() {
        let a = args(&["--cache", "/home/someone/.cache/rda"]);
        let text = Settings::from_args(&a).to_json();
        for leak in ["/tmp/out", "/home/someone", "geojson", "cache", "out"] {
            assert!(
                !text.contains(leak),
                "a settings file must not mention {leak}:\n{text}"
            );
        }
    }

    /// The realistic sharing case: a colleague has the output directory, not
    /// a settings file anyone remembered to save.
    #[test]
    fn a_manifest_reads_back_as_settings() {
        let manifest = serde_json::json!({
            "tool": "rda-ensemble",
            "input": {
                "state": "IL", "plan_type": "upper", "districts": 39,
                "dra_version": "v07", "adjacency": "dra", "cycle": 2020,
                "census": "T_20_CENS", "vap": "V_20_VAP", "cvap": "V_20_CVAP",
                "elections": ["E_16-20_COMP"], "expand_composites": false
            },
            "chain": {
                "steps_requested": 100, "variant": "cut-edges-ust",
                "tolerance": 0.05, "seed_tolerance": 0.01, "rng_seed": 11,
                "balance_ub": null, "region_weights": [], "target_pop": null,
                "segment_steps": null, "threads": 1, "batch_size": 1
            },
            "output": { "sample_every": 1, "prefixed_columns": false,
                        "kept": ["plans"] }
        });
        let s = Settings::parse(&manifest.to_string()).expect("manifest reads");
        assert_eq!(s.state, "IL");
        assert_eq!(s.districts, Some(39));
        assert_eq!(s.dra_version.as_deref(), Some("v07"));
        // Resolved outright, so the run is pinned rather than re-resolved.
        assert_eq!(s.census.as_deref(), Some("T_20_CENS"));
        assert_eq!(s.elections, vec!["E_16-20_COMP"]);
        // Which intermediates to write, so a replay produces the same files.
        assert_eq!(s.keep, vec!["plans"]);
        s.clone().into_args("/tmp/o".into(), None).expect("usable");
    }

    /// A manifest from a run that read a local file has no version to pin,
    /// and must not claim one.
    #[test]
    fn a_local_run_yields_no_dra_version() {
        let manifest = serde_json::json!({
            "tool": "rda-ensemble",
            "input": { "state": "MI", "plan_type": "congress",
                       "dra_version": "local", "elections": [], "cycle": 2020 },
            "chain": { "variant": "cut-edges-ust", "tolerance": 0.05,
                       "seed_tolerance": 0.01, "rng_seed": 1, "steps_requested": 10 },
            "output": { "sample_every": 1 }
        });
        let s = Settings::parse(&manifest.to_string()).expect("reads");
        assert_eq!(s.dra_version, None);
        assert!(s
            .portability_warnings()
            .iter()
            .any(|w| w.contains("No DRA version")));
    }

    #[test]
    fn a_future_version_is_refused_rather_than_misread() {
        let mut s = Settings::from_args(&args(&[]));
        s.version = VERSION + 1;
        let e = s.into_args("/tmp/o".into(), None).unwrap_err().to_string();
        assert!(e.contains("newer build"), "{e}");
    }

    /// The form asks for exactly what the run requires, because both read
    /// this list.
    #[test]
    fn missing_names_each_undecided_thing() {
        let s = Settings::starting_point();
        let wanted = s.missing();
        let fields: Vec<&str> = wanted.iter().map(|m| m.field.as_str()).collect();
        assert_eq!(
            fields,
            ["state", "plan_type", "cycle", "elections", "seed_tolerance", "plans",
             "variant", "tolerance"],
            "a blank form should want exactly these"
        );
        // Nothing is wanted once they are given.
        assert!(args(&[]).pipe_missing().is_empty());
    }

    /// Two variants need a parameter the others do not, and without it the
    /// run fails deep inside rustrecom instead of at the form.
    #[test]
    fn variant_specific_requirements_are_reported() {
        let mut s = Settings::from_args(&args(&[]));
        s.variant = "reversible".into();
        assert!(s.missing().iter().any(|m| m.field == "balance_ub"));
        s.balance_ub = Some(100);
        assert!(s.missing().is_empty());

        let mut s = Settings::from_args(&args(&[]));
        s.variant = "cut-edges-region-aware".into();
        assert!(s.missing().iter().any(|m| m.field == "region_weights"));
        s.region_weights = vec!["COUNTY=1.0".into()];
        assert!(s.missing().is_empty());
    }

    /// A name that is not one of the seven is reported at the form, not
    /// left to fail later in `parse_variant`.
    #[test]
    fn an_unknown_variant_is_reported() {
        let mut s = Settings::from_args(&args(&[]));
        s.variant = "cut-edges-usd".into();
        assert!(s.missing().iter().any(|m| m.field == "variant"));
        for name in VARIANTS {
            s.variant = name.into();
            assert!(s.missing().iter().all(|m| m.field != "variant"), "{name}");
        }
    }

    /// Zero is a real seed, so nothing in the data can call it undecided.
    /// A form has to notice its own empty field; this records why.
    #[test]
    fn a_zero_seed_is_not_missing() {
        let mut s = Settings::from_args(&args(&[]));
        s.rng_seed = 0;
        assert!(s.missing().is_empty());
    }

    #[test]
    fn settings_without_steps_or_plans_are_refused() {
        let mut s = Settings::from_args(&args(&[]));
        s.steps = None;
        s.plans = None;
        let e = s.into_args("/tmp/o".into(), None).unwrap_err().to_string();
        assert!(e.contains("steps") && e.contains("plans"), "{e}");
    }

    #[test]
    fn nonsense_is_rejected_with_a_readable_reason() {
        let e = Settings::parse("{\"not\": \"ours\"}").unwrap_err();
        assert!(format!("{e:#}").contains("neither"), "{e:#}");
        let e = Settings::parse("not json at all").unwrap_err();
        assert!(format!("{e:#}").contains("not JSON"), "{e:#}");
    }

    /// Reproducibility hazards are reported before the file is shared.
    /// A form's starting values must be the command line's, or the two
    /// front ends quietly disagree about what a default is.
    #[test]
    fn the_starting_point_carries_claps_defaults() {
        let s = Settings::starting_point();
        assert_eq!(s.chains, 1);
        assert_eq!(s.threads, 1);
        assert_eq!(s.batch_size, 1);
        assert_eq!(s.sample_every, 1);
        assert_eq!(s.adjacency, "auto");
        // And the placeholders are blank rather than pretending to be
        // decisions.
        assert!(s.state.is_empty());
        assert!(s.plan_type.is_empty(), "the chamber is nobody's default");
        assert!(s.variant.is_empty(), "nor is the variant");
        // The one value a form supplies that the command line does not: a
        // Stop button needs a boundary to stop at.
        assert_eq!(s.segment_steps, Some(FORM_SEGMENT_STEPS));
        assert_eq!(s.cycle, None);
        assert_eq!(s.steps, None);
        assert!(s.elections.is_empty());
        // So it cannot be run as-is.
        assert!(s.into_args("/tmp/o".into(), None).is_err());
    }

    #[test]
    fn portability_warnings_name_the_hazards() {
        let a = args(&["--threads", "4"]);
        let w = Settings::from_args(&a).portability_warnings();
        assert!(w.iter().any(|w| w.contains("threads is 4")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("No DRA version")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("come from `cycle`")), "{w:?}");
    }
}
