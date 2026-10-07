//! The run: GeoJSON in, scored ensemble out.
//!
//! Each stage produces a value, offers it to [`Artifacts`] in case the run
//! asked to keep it, and hands it to the next stage. Keeping an intermediate
//! writes the value the stage already holds, so it costs a serialisation and
//! nothing else, and the file is what the stage-by-stage `rdarust` CLI would
//! have written.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, bail, Context as _, Result};
use rdarust_core::context::Context;
use rdarust_core::graph::{is_connected, islands};
use rdarust_core::score::{ModeOpt, ScoreOptions};
use rdarust_io::extract::DataMapSpec;
use rdarust_io::{build_graph, RecomNames};
use rustrecom::partition::Partition;
use rustrecom::recom::{RecomParams, RecomVariant};
use serde_json::Value;

use crate::artifacts::{Artifact, Artifacts};
use crate::events::Sink;
use crate::manifest;
use crate::scoring::{ChainState, ScoringWriter, Summary};
use crate::resolved::Resolved;
use crate::dra;
use crate::{Adjacency, RunArgs, Variant};

/// Node attribute names on the ReCom graph. Fixed, because nothing outside
/// this binary reads them: the graph is built and consumed in one process.
const POP_COL: &str = "TOTAL_POP";
const ASSIGNMENT_COL: &str = "INITIAL";

/// A request to stop the run.
///
/// rustrecom runs a chain to completion and offers no way out from inside a
/// writer, so a run stops between segments and nowhere else. How soon that
/// is depends on `--segment-steps`: with no segmenting there is one segment,
/// and this is never read. Set it from a signal handler, a GUI's stop
/// button, or anything else outside the chain.
#[derive(Clone, Default)]
pub struct Cancel(Arc<std::sync::atomic::AtomicBool>);

impl Cancel {
    pub fn stop(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Carrying on from a chain that already ran.
///
/// A ReCom chain is Markov, so continuing from the plan an earlier run
/// ended on is the same chain, not a new one that happens to start
/// somewhere plausible. What has to come across is the plan itself, the
/// step it was at, and how many segments were already drawn -- without that
/// last one the extension would re-use the earlier run's random stream.
pub struct Extension {
    /// The plan to carry on from, geoid to district, districts from 1.
    pub plan: HashMap<String, u32>,
    /// The absolute step that plan sat at.
    pub from_step: u64,
    /// Segments the earlier run consumed, so new seeds cannot replay old
    /// ones.
    pub segment_base: u64,
}

pub fn run(cli: &RunArgs, keep: &[Artifact], cancel: &Cancel, ev: &Sink) -> Result<()> {
    run_extended(cli, keep, cancel, ev, None)
}

/// A run, optionally continuing an earlier one.
///
/// With `extend`, `cli.steps` is how many *more* steps to take, the output
/// directory is expected to already hold the earlier run's files, and
/// everything written is appended to them.
pub fn run_extended(
    cli: &RunArgs,
    keep: &[Artifact],
    cancel: &Cancel,
    ev: &Sink,
    extend: Option<&Extension>,
) -> Result<()> {
    let districts = districts(cli, ev)?;
    let steps = steps(cli)?;
    check(cli, districts, steps)?;
    let started = Instant::now();
    std::fs::create_dir_all(&cli.out)
        .with_context(|| format!("creating {}", cli.out.display()))?;
    let artifacts = Artifacts::new(&cli.out, keep);

    let package = resolve_input(cli, ev)?;
    let Some(ctx) = read_state(cli, ev, districts, steps, &package, &artifacts)? else {
        return Ok(());
    };
    let ctx = Arc::new(ctx);

    // Each chain gets its own seed, so its own starting plan as well as its
    // own path. R-hat asks whether chains that began somewhere different
    // ended up agreeing, which needs them to have begun somewhere different.
    let summaries: Vec<Summary> = std::thread::scope(|scope| -> Result<Vec<Summary>> {
        let mut handles = Vec::new();
        for i in 0..cli.chains {
            let ctx = ctx.clone();
            let artifacts = &artifacts;
            handles.push(scope.spawn(move || {
                one_chain(cli, districts, steps, artifacts, ctx, i, cancel, ev, extend)
            }));
        }
        handles.into_iter().map(|h| h.join().expect("a chain panicked")).collect()
    })?;

    report_convergence(
        cli, districts, steps, &package, &ctx, &artifacts, &summaries, started, ev, extend,
    )
}

/// Where a chain's own output goes: straight into --out for a single chain,
/// and a numbered directory each when there are several.
fn chain_dir(cli: &RunArgs, index: usize) -> std::path::PathBuf {
    if cli.chains <= 1 {
        cli.out.clone()
    } else {
        cli.out.join(format!("chain_{}", index + 1))
    }
}

/// Seeds are derived rather than asked for, so `--rng-seed 1 --chains 4`
/// still names the whole run with one number. Segment 0 is the starting
/// plan's and the first segment's; see [`crate::seed`] for why this is a
/// mix rather than an addition.
fn chain_seed(cli: &RunArgs, index: usize) -> u64 {
    crate::seed::derive(cli.rng_seed, index as u64, 0)
}

/// One chain: draw a starting plan, run it, score as it goes.
#[allow(clippy::too_many_arguments)]
fn one_chain(
    cli: &RunArgs,
    districts: usize,
    steps: u64,
    artifacts: &Artifacts,
    ctx: Arc<Context>,
    index: usize,
    cancel: &Cancel,
    ev: &Sink,
    extend: Option<&Extension>,
) -> Result<Summary> {
    let dir = chain_dir(cli, index);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // The state-level artifacts -- data map, graph, precinct data -- are the
    // same for every chain and were written once. These are not.
    let mine = artifacts.relocated(&dir);

    let (order, seed_assignments) = match extend {
        // An extension already has its starting plan; drawing a fresh one
        // would start a different chain wearing the same settings.
        Some(e) => {
            let order = ctx.sorted_precinct_order();
            let assignments = carry_over(e, &order, &ctx)?;
            ev.status(&format!(
                "carrying on from step {} of the earlier run",
                e.from_step
            ));
            (order, assignments)
        }
        None => seed_plan(cli, ev, districts, &ctx, &mine, chain_seed(cli, index), index)?,
    };
    chain(
        cli,
        ev,
        steps,
        &mine,
        ctx,
        Arc::new(order),
        &seed_assignments,
        index,
        &dir,
        cancel,
        extend,
    )
}

/// How many districts to draw.
///
/// The statutory count for the state and chamber, unless --districts asks
/// for something else. Most runs want the statutory one, which is why
/// --districts is optional: a user who knows they want NC congressional
/// districts should not also have to know there are 14.
pub fn districts(cli: &RunArgs, ev: &Sink) -> Result<usize> {
    let statutory =
        rdarust_core::states::districts_for(&cli.state, cli.chamber.as_str());
    match (cli.districts, statutory) {
        (Some(asked), Some(known)) if asked as u32 != known => {
            // Far more often a typo than a deliberate hypothetical, but the
            // hypothetical is legitimate, so say so and carry on.
            ev.warn(&format!(
                "{} {} has {known} districts, and --districts says {asked}. \
                 Drawing {asked}.",
                cli.state,
                cli.chamber.as_str()
            ));
            Ok(asked)
        }
        (Some(asked), _) => Ok(asked),
        (None, Some(known)) => Ok(known as usize),
        (None, None) => bail!(
            "no statutory district count for {} {}; pass --districts to say how many",
            cli.state,
            cli.chamber.as_str()
        ),
    }
}

/// How many chain steps to run.
///
/// People think in plans and the chain counts steps, and at an interval of
/// 200 those differ by a factor of 200. Asking for `--steps 10000
/// --sample-every 200` when 10,000 plans were wanted gives fifty, with no
/// error -- so `--plans` does the arithmetic.
pub fn steps(cli: &RunArgs) -> Result<u64> {
    match (cli.steps, cli.plans) {
        (Some(steps), _) => Ok(steps),
        (None, Some(plans)) => plans
            .checked_mul(cli.sample_every)
            .ok_or_else(|| anyhow!("--plans {plans} at --sample-every {} overflows", cli.sample_every)),
        (None, None) => bail!("give --plans (how many you want) or --steps (how long to run)"),
    }
}

/// Parameter checks that would otherwise fail deep inside a library, or
/// worse, not fail at all.
fn check(cli: &RunArgs, districts: usize, steps: u64) -> Result<()> {
    if districts < 2 {
        // Six states elect a single at-large representative, so asking for
        // a congressional ensemble there is a reasonable thing to try and a
        // confusing thing to be told about in terms of --districts, which
        // the user very likely did not pass.
        if cli.districts.is_none() {
            bail!(
                "{} {} is a single district, so there is nothing to partition \
                 and no ensemble to build. Try --plan-type upper or lower.",
                cli.state,
                cli.chamber.as_str()
            );
        }
        bail!("--districts must be at least 2; got {districts}");
    }
    if steps == 0 {
        bail!("--steps and --plans must be at least 1");
    }
    if cli.sample_every == 0 {
        bail!("--sample-every must be at least 1");
    }
    for (name, value) in [("--tolerance", cli.tolerance), ("--seed-tolerance", cli.seed_tolerance)]
    {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            bail!("{name} must be a fraction between 0 and 1; got {value}");
        }
    }
    // rustrecom panics rather than returns for these combinations, and a
    // panic inside a library call takes the whole process with it.
    let region_aware = matches!(
        cli.variant,
        crate::Variant::CutEdgesRegionAware | crate::Variant::DistrictPairsRegionAware
    );
    if !cli.region_weights.is_empty() && !region_aware {
        bail!(
            "--region-weights only does anything for --variant cut-edges-region-aware \
             or district-pairs-region-aware; {} ignores it",
            cli.variant.as_str()
        );
    }
    parse_region_weights(&cli.region_weights)?;
    if let Some(pop) = cli.target_pop {
        if pop == 0 {
            bail!("--target-pop must be greater than zero");
        }
    }
    if cli.threads == 0 {
        bail!("--threads must be at least 1");
    }
    if cli.batch_size == 0 {
        bail!("--batch-size must be at least 1");
    }
    Ok(())
}

/// The GeoJSON to read, downloading it if the command line named no file.
fn resolve_input(cli: &RunArgs, ev: &Sink) -> Result<dra::Package> {
    if let Some(path) = &cli.geojson {
        // DRA ships its graph beside the GeoJSON; look for it there.
        let graph = path.parent().and_then(|dir| {
            std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
                let p = e.path();
                p.file_name()?.to_str()?.ends_with("_graph.json").then_some(p)
            })
        });
        return Ok(dra::Package { geojson: path.clone(), graph, version: "local".into() });
    }

    // One implementation, in dra::resolve: a second copy here drifted from
    // it and lost the offline fallback.
    let cache = cli.cache.clone().unwrap_or_else(dra::default_cache);
    dra::resolve(&cache, &cli.state, cli.dra_version.as_deref(), ev)
}

/// Precinct adjacency, from DRA's graph or from the shapes.
///
/// They have agreed exactly on every state checked -- NC, IL and MI, node
/// for node -- but DRA's is the published artefact, so `auto` prefers it and
/// says so when the two disagree rather than letting a difference pass.
fn adjacency(
    cli: &RunArgs,
    ev: &Sink,
    package: &dra::Package,
    geoids: &[String],
    geometries: &[rdarust_geo::Geometry],
) -> Result<Vec<(String, Vec<String>)>> {
    let published = match (cli.adjacency, &package.graph) {
        (Adjacency::Geometry, _) => None,
        (_, Some(path)) => Some(
            rdarust_io::load_graph(path)
                .map_err(|e| anyhow!("{e}"))
                .with_context(|| format!("reading {}", path.display()))?,
        ),
        (Adjacency::Dra, None) => bail!(
            "--adjacency dra, but no *_graph.json sits beside the GeoJSON. \
             DRA ships one in each package; `--adjacency geometry` derives it instead."
        ),
        (Adjacency::Auto, None) => None,
    };

    let Some(published) = published else {
        ev.status("  adjacency derived from the precinct shapes");
        return Ok(rdarust_io::extract_graph(geoids, geometries));
    };

    if cli.adjacency == Adjacency::Dra {
        ev.status(&format!(
            "  adjacency from {}",
            package.graph.as_ref().expect("checked").display()
        ));
        return Ok(published);
    }

    // Auto: the geometry graph costs little, since extraction builds the
    // same coverage anyway, so compare rather than trust.
    let derived = rdarust_io::extract_graph(geoids, geometries);
    let name = package.graph.as_ref().expect("checked").display();
    match disagreements(&published, &derived) {
        0 => ev.status(&format!("  adjacency from {name}, matching the shapes exactly")),
        n => ev.warn(&format!(
            "{name} and the precinct shapes disagree about {n} node(s). \
             Using the published graph; --adjacency geometry uses the shapes."
        )),
    }
    Ok(published)
}

/// Nodes whose neighbour sets differ between two graphs.
fn disagreements(a: &[(String, Vec<String>)], b: &[(String, Vec<String>)]) -> usize {
    use std::collections::{BTreeMap, BTreeSet};
    let index = |g: &[(String, Vec<String>)]| -> BTreeMap<String, BTreeSet<String>> {
        g.iter().map(|(k, v)| (k.clone(), v.iter().cloned().collect())).collect()
    };
    let (a, b) = (index(a), index(b));
    a.keys()
        .chain(b.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .count()
}

/// Read the GeoJSON and turn it into a scoring context.
///
/// This is rdarust's `map-data`, `extract-graph` and `extract-data` in one
/// pass, with the records handed straight to the context rather than written
/// as JSONL and read back.
fn read_state(
    cli: &RunArgs,
    ev: &Sink,
    districts: usize,
    steps: u64,
    package: &dra::Package,
    artifacts: &Artifacts,
) -> Result<Option<Context>> {
    let path = &package.geojson;
    ev.status(&format!("reading {}", path.display()));
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let doc: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;

    let elections: Vec<String> = if cli.elections.iter().any(|e| e == "all") {
        vec!["__all__".to_string()]
    } else {
        cli.elections.clone()
    };
    // --cycle picks the three demographic datasets out of the file; naming
    // one explicitly overrides that choice.
    let picked = match cli.cycle {
        Some(year) => Some(crate::datasets::for_cycle(
            &doc,
            year,
            [cli.census.as_deref(), cli.vap.as_deref(), cli.cvap.as_deref()],
        )?),
        None => None,
    };
    let census = resolve("--census", cli.census.as_deref(), picked.as_ref().map(|c| &c.census))?;
    let vap = resolve("--vap", cli.vap.as_deref(), picked.as_ref().map(|c| &c.vap))?;
    let cvap = resolve("--cvap", cli.cvap.as_deref(), picked.as_ref().map(|c| &c.cvap))?;
    // Check the dataset names before anything else reads them. A name that
    // is not in the file would otherwise sail through extraction and fail
    // much later, inside a scoring formula, where the message means nothing.
    check_datasets(&doc, path, &census, &vap, &cvap, &elections)?;

    let mut warnings = Vec::new();
    let data_map = rdarust_io::map_data(
        &doc,
        &DataMapSpec {
            census: &census,
            vap: &vap,
            cvap: &cvap,
            elections: &elections,
            expand_composites: cli.expand_composites,
            version: None,
            directory: &path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            file: &path
                .file_name()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        },
        &mut warnings,
    )
    .map_err(|e| anyhow!("{e}"))?;
    for w in &warnings {
        ev.warn(w);
    }
    let n_elections = data_map
        .get("election")
        .and_then(|e| e.get("datasets"))
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if n_elections == 0 {
        bail!(
            "no election dataset matched --elections; the GeoJSON's datasets are the \
             names to choose from"
        );
    }
    artifacts.write_json_pretty(Artifact::DataMap, &data_map)?;

    report(cli, ev, districts, steps, &census, &vap, &cvap, &data_map_elections(&data_map));
    if cli.dry_run {
        ev.status("\n--dry-run: nothing was written");
        return Ok(None);
    }
    ev.status("");

    let features = rdarust_io::geojson::features_of(&doc).map_err(|e| anyhow!("{e}"))?;
    drop(text);
    let geoid_field = data_map.get("geoid").and_then(|g| g.as_str()).unwrap_or("id");
    let geoids: Vec<String> = features
        .iter()
        .map(|f| {
            f.properties
                .get(geoid_field)
                .and_then(|g| g.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let geometries: Vec<_> = features.iter().map(|f| f.geometry.clone()).collect();

    let graph = adjacency(cli, ev, package, &geoids, &geometries)?;
    if artifacts.wants(Artifact::Graph) {
        let mut obj = serde_json::Map::new();
        for (geoid, neighbours) in &graph {
            obj.insert(geoid.clone(), serde_json::json!(neighbours));
        }
        artifacts.write_json_pretty(Artifact::Graph, &Value::Object(obj))?;
    }

    let precincts =
        rdarust_io::extract_data(&features, &data_map, &graph).map_err(|e| anyhow!("{e}"))?;
    ev.status(&format!("  {} precincts, {n_elections} election(s)", precincts.len()));

    // The precinct stream opens with the data map, which is what tells the
    // reader which column holds which figure. `extract-data` writes the same
    // record first, so a kept `precinct_data.jsonl` matches it.
    let mut records = Vec::with_capacity(precincts.len() + 1);
    let mut metadata = serde_json::Map::new();
    metadata.insert("_tag_".into(), serde_json::json!("metadata"));
    metadata.insert("properties".into(), data_map);
    records.push(Value::Object(metadata));
    records.extend(precincts);
    artifacts.write_records(Artifact::Data, &records)?;

    let ctx = rdarust_io::load_input_data_from_records(records)
        .map_err(|e| anyhow!("{e}"))?
        .with_graph(graph)
        .into_context(&cli.state, cli.chamber.as_str(), Some(districts as u32))
        .map_err(|e| anyhow!("{e}"))?;
    for w in &ctx.warnings {
        ev.warn(w);
    }
    Ok(Some(ctx))
}

/// Fail on a dataset name the GeoJSON does not carry, and say what it does.
/// The elections the data map ended up naming, which `all` and
/// `--expand-composites` both change.
fn data_map_elections(data_map: &Value) -> Vec<String> {
    data_map
        .get("election")
        .and_then(|e| e.get("datasets"))
        .and_then(|d| d.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Print every resolved setting, marking anything the command line did not
/// say outright.
#[allow(clippy::too_many_arguments)]
fn report(
    cli: &RunArgs,
    ev: &Sink,
    districts: usize,
    steps: u64,
    census: &str,
    vap: &str,
    cvap: &str,
    elections: &[String],
) {
    let mut s = Resolved::default();
    let from_cycle = |given: &Option<String>| match (given, cli.cycle) {
        (Some(_), _) => None,
        (None, Some(year)) => Some(format!("--cycle {year}")),
        (None, None) => None,
    };

    s.section("Input").given("state", &cli.state).given("chamber", cli.chamber.as_str());
    match cli.districts {
        Some(_) => s.given("districts", districts),
        None => s.derived(
            "districts",
            districts,
            format!("statutory, {} {}", cli.state, cli.chamber.as_str()),
        ),
    };

    s.section("Data");
    for (name, value, given) in [
        ("census", census, &cli.census),
        ("voting-age pop", vap, &cli.vap),
        ("citizen VAP", cvap, &cli.cvap),
    ] {
        match from_cycle(given) {
            Some(from) => s.derived(name, value, from),
            None => s.given(name, value),
        };
    }
    // Name them rather than counting them: `all` and `--expand-composites`
    // can turn one argument into twenty, and "22 of them" does not tell
    // anybody what they scored.
    let asked_for_all = cli.elections.iter().any(|e| e == "all");
    let source = match (asked_for_all, cli.expand_composites) {
        (true, _) => Some("--elections all"),
        (false, true) => Some("--expand-composites"),
        (false, false) => None,
    };
    let mut lines = wrap(elections, 56);
    let first = if lines.is_empty() { String::new() } else { lines.remove(0) };
    match source {
        Some(from) => s.derived("elections", first, format!("{from}, {} in all", elections.len())),
        None => s.given("elections", first),
    };
    for line in lines {
        s.note(line);
    }

    s.section("Starting plan").given("population tolerance", cli.seed_tolerance);

    s.section("Chain").given("variant", cli.variant.as_str());
    match cli.plans {
        Some(p) => s.derived("steps", steps, format!("--plans {p} x --sample-every {}", cli.sample_every)),
        None => s.given("steps", steps),
    };
    s
        .given("population tolerance", cli.tolerance)
        .given("rng seed", cli.rng_seed);
    // Beside the seed, not beside --threads: the segment length is part of
    // what produced this ensemble, not a performance setting.
    match cli.segment_steps {
        Some(n) => s.given(
            "segment steps",
            format!("{n}, in {} segment(s)", segments(steps, Some(n)).len()),
        ),
        None => s.derived("segment steps", "one segment", "not given"),
    };
    if cli.variant == crate::Variant::Reversible {
        s.given("balance upper bound", cli.balance_ub.unwrap_or(0));
    }
    for (col, weight) in parse_region_weights(&cli.region_weights).unwrap_or_default() {
        s.given("region weight", format!("{col} = {weight}"));
    }
    match cli.target_pop {
        Some(pop) => s.given("target population", pop),
        None => s.derived("target population", "total / districts", "not given"),
    };
    s.given("threads", cli.threads).given("batch size", cli.batch_size);

    s.section("Scoring")
        .given("sample every", format!("{} step(s)", cli.sample_every))
        .derived("majority-minority", "counted", "always on")
        .given(
            "reverse-weight splitting",
            if cli.reverse_weight_splitting { "reported" } else { "not reported" },
        )
        .given("column names", if cli.prefixes { "dataset-prefixed" } else { "plain" });

    ev.status(s.render().trim_end_matches('\n'));
}

/// `COLUMN=WEIGHT` pairs, highest weight first as rustrecom orders them.
fn parse_region_weights(args: &[String]) -> Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    for arg in args {
        let (col, weight) = arg.split_once('=').ok_or_else(|| {
            anyhow!("--region-weights takes COLUMN=WEIGHT, e.g. COUNTY=1.0; got `{arg}`")
        })?;
        let weight: f64 = weight.trim().parse().map_err(|_| {
            anyhow!("--region-weights: `{weight}` in `{arg}` is not a number")
        })?;
        if !weight.is_finite() || weight <= 0.0 {
            bail!("--region-weights: the weight in `{arg}` must be positive");
        }
        out.push((col.trim().to_string(), weight));
    }
    // rustrecom orders them by descending weight, meaning most important
    // first; match that so a run here and a run there agree.
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(out)
}

/// Comma-separated names, broken into lines of at most `width` characters.
fn wrap(names: &[String], width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for name in names {
        match lines.last_mut() {
            Some(line) if line.len() + 2 + name.len() <= width => {
                line.push_str(", ");
                line.push_str(name);
            }
            _ => lines.push(name.clone()),
        }
    }
    lines
}

/// A dataset name: the one given, else the one --cycle picked.
fn resolve(flag: &str, given: Option<&str>, picked: Option<&String>) -> Result<String> {
    match (given, picked) {
        (Some(name), _) => Ok(name.to_string()),
        (None, Some(name)) => Ok(name.clone()),
        (None, None) => bail!("give {flag}, or --cycle to pick it from the GeoJSON"),
    }
}

fn check_datasets(
    doc: &Value,
    geojson: &std::path::Path,
    census: &str,
    vap: &str,
    cvap: &str,
    elections: &[String],
) -> Result<()> {
    let Some(available) = doc.get("datasets").and_then(|d| d.as_object()) else {
        bail!("the GeoJSON has no datasets object; is this a DRA export?");
    };

    let listing = |prefix: &str| -> String {
        let mut names: Vec<&str> = available
            .keys()
            .filter(|k| k.starts_with(prefix))
            .map(|k| k.as_str())
            .collect();
        names.sort_unstable();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    };

    for (flag, name, prefix) in
        [("--census", census, "T_"), ("--vap", vap, "V_"), ("--cvap", cvap, "V_")]
    {
        if !available.contains_key(name) {
            bail!(
                "{flag} {name} is not in the GeoJSON.\nAvailable: {}\n\
                 `rda-ensemble datasets {}` describes each one.",
                listing(prefix),
                geojson.display()
            );
        }
    }

    if elections != ["__all__"] {
        let missing: Vec<&str> = elections
            .iter()
            .filter(|e| !available.contains_key(e.as_str()))
            .map(|e| e.as_str())
            .collect();
        if !missing.is_empty() {
            bail!(
                "--elections names {} that the GeoJSON does not carry.\nAvailable: {}\n\
                 `rda-ensemble datasets {}` describes each one, and \
                 `--elections all` takes them all.",
                missing.join(", "),
                listing("E_"),
                geojson.display()
            );
        }
    }
    Ok(())
}

/// Draw a population-balanced starting plan.
///
/// Returns the precinct index of each ReCom node, and the part each node
/// landed in. partigraph numbers parts from 0.
fn seed_plan(
    cli: &RunArgs,
    ev: &Sink,
    districts: usize,
    ctx: &Context,
    artifacts: &Artifacts,
    seed: u64,
    index: usize,
) -> Result<(Vec<u32>, Vec<u32>)> {
    let order = ctx.sorted_precinct_order();
    let mut position = vec![u32::MAX; ctx.n_precincts()];
    for (node, &precinct) in order.iter().enumerate() {
        position[precinct as usize] = node as u32;
    }

    // ReCom walks the dual graph looking for balanced cuts, so a graph in
    // pieces cannot work. Say so here, where the advice is actionable.
    if !is_connected(&order, &ctx.adjacency, ctx.out_of_state) {
        let pieces = islands(&order, &ctx.adjacency, ctx.out_of_state);
        bail!(
            "the precinct graph is in {} pieces, so no chain can reach every precinct. \
             Islands and water crossings cause this; `rdarust contiguity-mods` proposes \
             the edges that would join them.",
            pieces.len()
        );
    }

    let edges: Vec<(u32, u32)> = order
        .iter()
        .enumerate()
        .flat_map(|(node, &precinct)| {
            let position = &position;
            ctx.adjacency[precinct as usize]
                .iter()
                .filter(move |&&nb| Some(nb) != ctx.out_of_state)
                .map(move |&nb| (node as u32, position[nb as usize]))
                .filter(|&(a, b)| a < b && b != u32::MAX)
        })
        .collect();
    let graph = partigraph::Graph::from_edges(order.len(), edges)
        .map_err(|e| anyhow!("building the dual graph: {e}"))?;
    let weights: Vec<f64> = order.iter().map(|&i| ctx.pop[i as usize] as f64).collect();

    if cli.chains <= 1 {
        ev.status(&format!("drawing a starting plan with {districts} districts"));
    } else {
        ev.status(&format!(
            "chain {}: drawing a starting plan with {districts} districts",
            index + 1
        ));
    }
    let balanced = partigraph::partition_balanced(
        &graph,
        &weights,
        &partigraph::BalanceParams {
            parts: districts,
            epsilon: cli.seed_tolerance,
            seed,
            ..Default::default()
        },
    )
    .map_err(|e| {
        anyhow!(
            "{e}\nA tighter --seed-tolerance is harder to satisfy; try loosening it."
        )
    })?;
    ev.status(&format!(
        "  worst district is {:.3}% from ideal",
        balanced.worst_deviation() * 100.0
    ));

    let assignments = balanced.partition.assignments().to_vec();
    if artifacts.wants(Artifact::SeedPlan) {
        let mut w = artifacts.writer(Artifact::SeedPlan)?.expect("asked for");
        writeln!(w, "GEOID,District")?;
        for (node, &part) in assignments.iter().enumerate() {
            writeln!(w, "{},{}", ctx.geoids[order[node] as usize], part + 1)?;
        }
        w.flush()?;
    }
    Ok((order, assignments))
}

/// Run the chain, scoring plans as they arrive.
#[allow(clippy::too_many_arguments)]
fn chain(
    cli: &RunArgs,
    ev: &Sink,
    steps: u64,
    artifacts: &Artifacts,
    ctx: Arc<Context>,
    order: Arc<Vec<u32>>,
    seed_assignments: &[u32],
    index: usize,
    dir: &std::path::Path,
    cancel: &Cancel,
    extend: Option<&Extension>,
) -> Result<Summary> {
    // rustrecom builds its own graph, so it owns its own invariants. The
    // document below is the only interchange structure in the run, and it is
    // built once, in memory.
    let seed: HashMap<String, u32> = order
        .iter()
        .zip(seed_assignments)
        .map(|(&precinct, &part)| (ctx.geoids[precinct as usize].clone(), part))
        .collect();
    // COUNTY carries each precinct's FIPS code. The region-aware variants
    // compare it between the ends of a candidate cut and prefer cuts whose
    // ends differ, which is to say cuts that leave counties whole.
    let names = RecomNames {
        geoid: "GEOID",
        pop: POP_COL,
        county: "COUNTY",
        assignment: ASSIGNMENT_COL,
    };
    let built = build_graph(&ctx, names, Some(&seed)).map_err(|e| anyhow!("{e}"))?;
    artifacts.write_json(Artifact::RecomGraph, &built.doc)?;

    // Region-aware sampling reads the county column out of `graph.attr`, so
    // it has to be loaded; nothing else needs a node attribute, and loading
    // one costs a string per precinct.
    let region_weights = parse_region_weights(&cli.region_weights)?;
    let columns: Vec<String> = region_weights.iter().map(|(col, _)| col.clone()).collect();
    let (graph, partition) = rustrecom::init::from_networkx_value(
        built.doc,
        POP_COL,
        ASSIGNMENT_COL,
        columns,
        vec![],
        vec![],
    )
    .map_err(|e| anyhow!("handing the graph to rustrecom: {e}"))?;

    let ideal = match cli.target_pop {
        Some(pop) => pop as f64,
        None => graph.total_pop as f64 / partition.num_dists as f64,
    };
    let mut params = RecomParams {
        min_pop: ((1.0 - cli.tolerance) * ideal).ceil() as u32,
        max_pop: ((1.0 + cli.tolerance) * ideal).floor() as u32,
        balance_ub: cli.balance_ub.unwrap_or(0),
        num_steps: steps,
        rng_seed: chain_seed(cli, index),
        variant: variant_of(cli.variant),
        region_weights: (!region_weights.is_empty()).then_some(region_weights),
        edge_weight_keys: Vec::new(),
    };

    // An extension continues the files it was handed; a fresh run starts
    // them. Opening for append on a fresh run would silently double an
    // ensemble if --out happened to hold one already.
    let open = |path: std::path::PathBuf| -> Result<File> {
        let f = match extend {
            Some(_) => std::fs::OpenOptions::new().append(true).open(&path),
            None => File::create(&path),
        };
        f.with_context(|| format!("opening {}", path.display()))
    };
    let scores = open(dir.join("scores.csv"))?;
    let by_district = open(dir.join("by_district.jsonl"))?;
    let plans = if artifacts.wants(Artifact::Plans) {
        Some(open(artifacts.path(Artifact::Plans))?)
    } else {
        None
    };
    // Outlives the segments: rustrecom takes the writer and closes it at the
    // end of each call, but the files and the step cursor carry on.
    // Absolute step numbering continues across an extension, so the
    // combined scores.csv reads as one chain rather than two restarts.
    let step_base = extend.map_or(0, |e| e.from_step);
    let state = Arc::new(Mutex::new(ChainState::new(
        &ctx,
        scores,
        by_district,
        plans,
        ev.clone(),
        step_base + steps,
        extend.map(|e| e.from_step),
    )));

    let opts = ScoreOptions {
        mode: ModeOpt::default(),
        // On, as rdapy scores normally. `ScoreOptions::default()` has it
        // off only because rdapy's legacy tests do.
        mmd_scoring: true,
        reverse_weight_splitting: cli.reverse_weight_splitting,
        ..Default::default()
    };

    let plan = segments(steps, cli.segment_steps);
    match cli.segment_steps {
        Some(n) => ev.status(&format!(
            "running {steps} steps as {} segment(s) of up to {n}, scoring every {}",
            plan.len(),
            cli.sample_every
        )),
        None => ev.status(&format!(
            "running {steps} steps, scoring every {}",
            cli.sample_every
        )),
    }

    let mut partition = partition;
    let mut stopped = false;
    for (segment, &(offset, num_steps)) in plan.iter().enumerate() {
        params.num_steps = num_steps;
        // Past the earlier run's segments, so an extension cannot draw a
        // stream that run already drew.
        let segment_number = extend.map_or(0, |e| e.segment_base) + segment as u64;
        params.rng_seed = crate::seed::derive(cli.rng_seed, index as u64, segment_number);
        let writer = ScoringWriter::new(
            ctx.clone(),
            order.clone(),
            opts.clone(),
            cli.prefixes,
            cli.sample_every,
            step_base + offset,
            state.clone(),
        );
        rustrecom::recom::run::multi_chain(
            &graph,
            &partition,
            Box::new(writer),
            &params,
            cli.threads,
            cli.batch_size,
            // Never rustrecom's own bar. It is sized to one call, so a
            // segmented run would redraw it per segment; the writer draws
            // ours from the step numbers it is handed either way.
            false,
        )
        .map_err(|e| anyhow!("the chain stopped: {e}"))?;

        if let Some(e) = &state.lock().expect("chain state").summary.error {
            bail!("scoring failed at {e}");
        }
        if segment + 1 == plan.len() {
            break;
        }
        if cancel.stopped() {
            stopped = true;
            break;
        }
        // Carry on from where the last segment stopped. ReCom is Markov:
        // the next step depends on this partition and nothing else, so a
        // segmented chain is the same chain, not an approximation of one.
        let assignments = state.lock().expect("chain state").current_plan().to_vec();
        partition = Partition::from_assignments(&graph, &assignments)
            .map_err(|e| anyhow!("resuming the chain after segment {}: {e:?}", segment + 1))?;
    }

    ev.progress_done();
    let mut summary = Arc::try_unwrap(state)
        .map_err(|_| anyhow!("the chain outlived its writer"))?
        .into_inner()
        .expect("chain state")
        .summary;
    summary.stopped = stopped;
    summary.segments = extend.map_or(0, |e| e.segment_base) + plan.len() as u64;
    if stopped {
        // "scored" would read as the size of the ensemble, which for an
        // extension it is not: the rows it was appended to are not counted
        // here. The closing report gives the total.
        ev.status(&format!(
            "  stopped early at step {}, having {} {} plans",
            summary.steps,
            if extend.is_some() { "added" } else { "scored" },
            summary.scored
        ));
    } else if cli.chains > 1 {
        ev.status(&format!("chain {}: scored {} plans", index + 1, summary.scored));
    }
    Ok(summary)
}

/// Every numeric column of a scores.csv, in file order.
///
/// Only an extension needs this: a fresh run already has the numbers in
/// memory. Reading them back is what lets the diagnostics describe the
/// combined ensemble rather than the part added today.
fn series_from_csv(path: &std::path::Path) -> Result<std::collections::BTreeMap<String, Vec<f64>>> {
    use std::io::BufRead;
    let file = File::open(path)
        .with_context(|| format!("reading {} for the diagnostics", path.display()))?;
    let mut lines = std::io::BufReader::new(file).lines();
    let header = lines
        .next()
        .transpose()?
        .ok_or_else(|| anyhow!("{} is empty", path.display()))?;
    let names: Vec<String> = header.trim().split(',').map(str::to_string).collect();
    let mut out: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // Skip the first column. It holds the plan's name, which is a
        // zero-padded step number and so parses perfectly well as a float --
        // diagnosing it reported the step counter as the worst-mixing score
        // in the ensemble. A blank elsewhere is a score that does not apply
        // to that plan, and is skipped by failing to parse.
        for (name, field) in names.iter().zip(line.trim().split(',')).skip(1) {
            if let Ok(v) = field.parse::<f64>() {
                out.entry(name.clone()).or_default().push(v);
            }
        }
    }
    Ok(out)
}

/// The earlier run's final plan, in this run's node order.
///
/// The plan arrives as geoids because that is what survives between runs;
/// node numbering is an artefact of one process. Districts come back to
/// ReCom's zero-based numbering here.
fn carry_over(e: &Extension, order: &[u32], ctx: &Context) -> Result<Vec<u32>> {
    let mut out = Vec::with_capacity(order.len());
    for (node, &precinct) in order.iter().enumerate() {
        let geoid = &ctx.geoids[precinct as usize];
        let district = e.plan.get(geoid).ok_or_else(|| {
            anyhow!(
                "the earlier plan does not cover precinct {geoid}. The two runs \
                 are not on the same data: check the state and --dra-version."
            )
        })?;
        if *district == 0 {
            bail!("precinct {geoid} is unassigned in the earlier plan");
        }
        out.push(district - 1);
        debug_assert!(node < order.len());
    }
    Ok(out)
}

/// How the chain's steps divide into segments.
///
/// Each entry is `(absolute offset, num_steps)` for one call to rustrecom,
/// which numbers its own steps from zero. A segment's local step 0 is the
/// plan the previous segment ended on -- already scored there -- so each
/// segment advances the chain by `num_steps - 1`.
///
/// With no `--segment-steps` this is a single call, exactly as before.
pub fn segments(steps: u64, segment: Option<u64>) -> Vec<(u64, u64)> {
    let advance = match segment {
        Some(n) if n > 0 => n,
        // One segment covering everything. The `max(1)` keeps `steps == 1`,
        // which is just the seed plan, from looping forever.
        _ => steps.saturating_sub(1).max(1),
    };
    let last = steps.saturating_sub(1);
    let mut out = Vec::new();
    let mut offset = 0;
    loop {
        let take = last.saturating_sub(offset).min(advance);
        out.push((offset, take + 1));
        if take == 0 {
            break;
        }
        offset += take;
        if offset >= last {
            break;
        }
    }
    out
}

/// Report what the run says about itself, and write the manifest.
#[allow(clippy::too_many_arguments)]
fn report_convergence(
    cli: &RunArgs,
    districts: usize,
    steps: u64,
    package: &dra::Package,
    ctx: &Context,
    artifacts: &Artifacts,
    summaries: &[Summary],
    started: Instant,
    ev: &Sink,
    extend: Option<&Extension>,
) -> Result<()> {
    // An extension's diagnostics are about the whole ensemble, not the part
    // added today. By now the new rows have been appended, so the file on
    // disk is the whole thing and is the honest source -- the chains hold
    // only what they themselves scored.
    // An extension was asked for however many steps it had left to run,
    // but the files describe the whole ensemble, and so must the record of
    // it: a settings.json saying 300 beside an 800-step ensemble would
    // replay as something else entirely.
    let steps = extend.map_or(steps, |e| e.from_step + steps);

    let series: Vec<_> = match extend {
        Some(_) => vec![series_from_csv(&cli.out.join("scores.csv"))?],
        None => summaries.iter().map(|s| s.series.clone()).collect(),
    };
    let report = crate::diagnostics::Report::build(&series);
    // `series` already holds the earlier plans as well as the new ones, so
    // its length is the size of the ensemble that now exists on disk.
    let scored: u64 = series
        .iter()
        .map(|c| c.values().next().map_or(0, |v| v.len() as u64))
        .sum();
    let taken = summaries.iter().map(|s| s.steps).max().unwrap_or(0);

    ev.status(&format!(
        "\nscored {scored} plans over {} chain(s), to step {taken}, in {:.1}s",
        summaries.len(),
        started.elapsed().as_secs_f64()
    ));
    ev.status(report.render().trim_end_matches('\n'));

    let combined = Summary {
        steps: taken,
        scored,
        segments: summaries.iter().map(|s| s.segments).max().unwrap_or(0),
        error: None,
        // Any chain stopping early makes the whole ensemble short of what
        // was asked for, so the manifest says so for the run as a whole.
        stopped: summaries.iter().any(|s| s.stopped),
        series: Default::default(),
    };
    manifest::write(
        cli,
        districts,
        steps,
        package,
        ctx,
        &combined,
        started,
        &cli.out.join("manifest.json"),
    )?;

    // The same decisions as the manifest, but as the file that feeds them
    // back in: no outcomes, no local paths, ready to hand to someone else.
    // Written every time, because the person who will want it is usually not
    // the person who ran this, and nobody remembers to ask for it.
    let mut settings = crate::settings::Settings::from_args(cli);
    settings.steps = Some(steps);
    settings.plans = None;
    std::fs::write(cli.out.join("settings.json"), settings.to_json())
        .with_context(|| format!("writing {}", cli.out.join("settings.json").display()))?;

    let path = cli.out.join("diagnostics.json");
    let mut file = File::create(&path)
        .with_context(|| format!("creating {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, &report.to_value())?;
    file.write_all(b"\n")?;

    ev.status(&format!("\nwrote {}", cli.out.display()));
    let inside = if cli.chains > 1 {
        ev.status(&format!(
            "  chain_1/ .. chain_{}/  one directory per chain, each holding:",
            cli.chains
        ));
        "    "
    } else {
        ""
    };
    ev.status(&format!(
        "  {inside}scores.csv          one row per plan in the ensemble"
    ));
    ev.status(&format!("  {inside}by_district.jsonl   the same plans, district by district"));
    ev.status("  manifest.json       what this run was, so it can be repeated");
    ev.status("  settings.json       the same decisions, to hand to someone else");
    ev.status("  diagnostics.json    R-hat and effective sample size, per score");
    for what in Artifact::ALL {
        if artifacts.wants(what) {
            ev.status(&format!("  {:<20}{}", what.file_name(), what.describe()));
        }
    }
    Ok(())
}

fn variant_of(v: Variant) -> RecomVariant {
    match v {
        Variant::Reversible => RecomVariant::Reversible,
        Variant::CutEdgesUst => RecomVariant::CutEdgesUST,
        Variant::DistrictPairsUst => RecomVariant::DistrictPairsUST,
        Variant::CutEdgesMst => RecomVariant::CutEdgesRMST,
        Variant::DistrictPairsMst => RecomVariant::DistrictPairsRMST,
        Variant::CutEdgesRegionAware => RecomVariant::CutEdgesRegionAware,
        Variant::DistrictPairsRegionAware => RecomVariant::DistrictPairsRegionAware,
    }
}

#[cfg(test)]
mod segment_tests {
    use super::segments;

    /// Without --segment-steps the chain is one call, exactly as before.
    #[test]
    fn unsegmented_is_a_single_call() {
        assert_eq!(segments(100, None), vec![(0, 100)]);
        assert_eq!(segments(1, None), vec![(0, 1)]);
    }

    /// Every segment after the first re-presents the plan the previous one
    /// ended on as its own local step 0, so the offsets overlap by one step
    /// and the advances still add up to the steps asked for.
    #[test]
    fn segments_tile_the_chain_exactly() {
        for &(steps, seg) in &[(100u64, 30u64), (100, 7), (10, 1), (1000, 999), (5, 100)] {
            let plan = segments(steps, Some(seg));
            let (last_offset, last_len) = *plan.last().expect("at least one");
            assert_eq!(
                last_offset + last_len - 1,
                steps - 1,
                "steps={steps} seg={seg}: chain must end on the last step"
            );
            // Each segment starts where the previous one ended.
            for pair in plan.windows(2) {
                let (off, len) = pair[0];
                assert_eq!(pair[1].0, off + len - 1, "steps={steps} seg={seg}");
            }
            // No segment advances further than asked.
            for &(_, len) in &plan {
                assert!(len - 1 <= seg, "steps={steps} seg={seg}: segment too long");
            }
        }
    }

    /// A segment longer than the chain is one segment, not a hang.
    #[test]
    fn an_oversized_segment_is_harmless() {
        assert_eq!(segments(10, Some(1000)), vec![(0, 10)]);
        assert_eq!(segments(1, Some(50)), vec![(0, 1)]);
    }

    /// Zero would mean no progress per call; treat it as unsegmented rather
    /// than looping forever.
    #[test]
    fn zero_is_treated_as_unsegmented() {
        assert_eq!(segments(100, Some(0)), segments(100, None));
    }
}

#[cfg(test)]
mod csv_series_tests {
    use super::series_from_csv;

    /// The name column is a zero-padded step number, which parses as a
    /// float. Diagnosing it reported the step counter as the ensemble's
    /// worst-mixing score, which is both meaningless and alarming.
    #[test]
    fn the_plan_name_is_not_a_score() {
        let dir = std::env::temp_dir().join(format!("rda-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scores.csv");
        std::fs::write(
            &path,
            "name,reock,declination\r\n000000,0.4,0.1\r\n000010,0.5,\r\n000020,0.6,0.3\r\n",
        )
        .unwrap();
        let series = series_from_csv(&path).expect("reads");
        assert!(!series.contains_key("name"), "got {:?}", series.keys());
        assert_eq!(series["reock"], vec![0.4, 0.5, 0.6]);
        // A blank is a score that does not apply, not a zero.
        assert_eq!(series["declination"], vec![0.1, 0.3]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
