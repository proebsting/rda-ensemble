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
use rustrecom::recom::{RecomParams, RecomVariant};
use serde_json::Value;

use crate::artifacts::{Artifact, Artifacts};
use crate::manifest;
use crate::scoring::{ScoringWriter, Summary};
use crate::settings::Settings;
use crate::{RunArgs, Variant};

/// Node attribute names on the ReCom graph. Fixed, because nothing outside
/// this binary reads them: the graph is built and consumed in one process.
const POP_COL: &str = "TOTAL_POP";
const ASSIGNMENT_COL: &str = "INITIAL";

pub fn run(cli: &RunArgs, keep: &[Artifact]) -> Result<()> {
    let districts = districts(cli)?;
    check(cli, districts)?;
    let started = Instant::now();
    std::fs::create_dir_all(&cli.out)
        .with_context(|| format!("creating {}", cli.out.display()))?;
    let artifacts = Artifacts::new(&cli.out, keep);

    let Some(ctx) = read_state(cli, districts, &artifacts)? else {
        return Ok(());
    };
    let ctx = Arc::new(ctx);
    let (order, seed_assignments) = seed_plan(cli, districts, &ctx, &artifacts)?;
    let order = Arc::new(order);
    chain(cli, districts, &artifacts, ctx.clone(), order, &seed_assignments, started)
}

/// How many districts to draw.
///
/// The statutory count for the state and chamber, unless --districts asks
/// for something else. Most runs want the statutory one, which is why
/// --districts is optional: a user who knows they want NC congressional
/// districts should not also have to know there are 14.
pub fn districts(cli: &RunArgs) -> Result<usize> {
    let statutory =
        rdarust_core::states::districts_for(&cli.state, cli.chamber.as_str());
    match (cli.districts, statutory) {
        (Some(asked), Some(known)) if asked as u32 != known => {
            // Far more often a typo than a deliberate hypothetical, but the
            // hypothetical is legitimate, so say so and carry on.
            eprintln!(
                "warning: {} {} has {known} districts, and --districts says {asked}. \
                 Drawing {asked}.",
                cli.state,
                cli.chamber.as_str()
            );
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

/// Parameter checks that would otherwise fail deep inside a library, or
/// worse, not fail at all.
fn check(cli: &RunArgs, districts: usize) -> Result<()> {
    if districts < 2 {
        bail!("--districts must be at least 2; got {districts}");
    }
    if cli.steps == 0 {
        bail!("--steps must be at least 1");
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

/// Read the GeoJSON and turn it into a scoring context.
///
/// This is rdarust's `map-data`, `extract-graph` and `extract-data` in one
/// pass, with the records handed straight to the context rather than written
/// as JSONL and read back.
fn read_state(
    cli: &RunArgs,
    districts: usize,
    artifacts: &Artifacts,
) -> Result<Option<Context>> {
    let path = &cli.geojson;
    eprintln!("reading {}", path.display());
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
        Some(year) => Some(crate::datasets::for_cycle(&doc, year)?),
        None => None,
    };
    let census = resolve("--census", cli.census.as_deref(), picked.as_ref().map(|c| &c.census))?;
    let vap = resolve("--vap", cli.vap.as_deref(), picked.as_ref().map(|c| &c.vap))?;
    let cvap = resolve("--cvap", cli.cvap.as_deref(), picked.as_ref().map(|c| &c.cvap))?;
    // Check the dataset names before anything else reads them. A name that
    // is not in the file would otherwise sail through extraction and fail
    // much later, inside a scoring formula, where the message means nothing.
    check_datasets(&doc, cli, &census, &vap, &cvap, &elections)?;

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
        eprintln!("warning: {w}");
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

    report(cli, districts, &census, &vap, &cvap, &data_map_elections(&data_map));
    if cli.dry_run {
        eprintln!("\n--dry-run: nothing was written");
        return Ok(None);
    }
    eprintln!();

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

    let graph = rdarust_io::extract_graph(&geoids, &geometries);
    if artifacts.wants(Artifact::Graph) {
        let mut obj = serde_json::Map::new();
        for (geoid, neighbours) in &graph {
            obj.insert(geoid.clone(), serde_json::json!(neighbours));
        }
        artifacts.write_json_pretty(Artifact::Graph, &Value::Object(obj))?;
    }

    let precincts =
        rdarust_io::extract_data(&features, &data_map, &graph).map_err(|e| anyhow!("{e}"))?;
    eprintln!("  {} precincts, {n_elections} election(s)", precincts.len());

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
        eprintln!("warning: {w}");
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
fn report(
    cli: &RunArgs,
    districts: usize,
    census: &str,
    vap: &str,
    cvap: &str,
    elections: &[String],
) {
    let mut s = Settings::default();
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

    s.section("Chain")
        .given("variant", cli.variant.as_str())
        .given("steps", cli.steps)
        .given("population tolerance", cli.tolerance)
        .given("rng seed", cli.rng_seed);
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

    eprint!("{}", s.render());
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
    cli: &RunArgs,
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
                cli.geojson.display()
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
                cli.geojson.display()
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
    districts: usize,
    ctx: &Context,
    artifacts: &Artifacts,
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

    eprintln!("drawing a starting plan with {districts} districts");
    let balanced = partigraph::partition_balanced(
        &graph,
        &weights,
        &partigraph::BalanceParams {
            parts: districts,
            epsilon: cli.seed_tolerance,
            seed: cli.rng_seed,
            ..Default::default()
        },
    )
    .map_err(|e| {
        anyhow!(
            "{e}\nA tighter --seed-tolerance is harder to satisfy; try loosening it."
        )
    })?;
    eprintln!(
        "  worst district is {:.3}% from ideal",
        balanced.worst_deviation() * 100.0
    );

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
fn chain(
    cli: &RunArgs,
    districts: usize,
    artifacts: &Artifacts,
    ctx: Arc<Context>,
    order: Arc<Vec<u32>>,
    seed_assignments: &[u32],
    started: Instant,
) -> Result<()> {
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
    let params = RecomParams {
        min_pop: ((1.0 - cli.tolerance) * ideal).ceil() as u32,
        max_pop: ((1.0 + cli.tolerance) * ideal).floor() as u32,
        balance_ub: cli.balance_ub.unwrap_or(0),
        num_steps: cli.steps,
        rng_seed: cli.rng_seed,
        variant: variant_of(cli.variant),
        region_weights: (!region_weights.is_empty()).then_some(region_weights),
        edge_weight_keys: Vec::new(),
    };

    let scores = File::create(cli.out.join("scores.csv"))
        .with_context(|| format!("creating {}", cli.out.join("scores.csv").display()))?;
    let by_district = File::create(cli.out.join("by_district.jsonl"))?;
    let plans = if artifacts.wants(Artifact::Plans) {
        Some(File::create(artifacts.path(Artifact::Plans))?)
    } else {
        None
    };

    let summary = Arc::new(Mutex::new(Summary::default()));
    let writer = ScoringWriter::new(
        ctx.clone(),
        order,
        ScoreOptions {
            mode: ModeOpt::default(),
            // On, as rdapy scores normally. `ScoreOptions::default()` has it
            // off only because rdapy's legacy tests do.
            mmd_scoring: true,
            reverse_weight_splitting: cli.reverse_weight_splitting,
            ..Default::default()
        },
        cli.prefixes,
        cli.sample_every,
        scores,
        by_district,
        plans,
        summary.clone(),
    );

    eprintln!(
        "running {} steps, scoring every {}",
        cli.steps, cli.sample_every
    );
    rustrecom::recom::run::multi_chain(
        &graph,
        &partition,
        Box::new(writer),
        &params,
        cli.threads,
        cli.batch_size,
        cli.progress,
    )
    .map_err(|e| anyhow!("the chain stopped: {e}"))?;

    let summary = summary.lock().expect("summary");
    if let Some(e) = &summary.error {
        bail!("scoring failed at {e}");
    }
    manifest::write(cli, districts, &ctx, &summary, started, &cli.out.join("manifest.json"))?;

    eprintln!(
        "scored {} plans, from steps 0 to {}, in {:.1}s",
        summary.scored,
        summary.steps,
        started.elapsed().as_secs_f64()
    );
    eprintln!("\nwrote {}", cli.out.display());
    eprintln!("  scores.csv          one row per scored plan");
    eprintln!("  by_district.jsonl   the same plans, district by district");
    eprintln!("  manifest.json       what this run was, so it can be repeated");
    for what in Artifact::ALL {
        if artifacts.wants(what) {
            eprintln!("  {:<20}{}", what.file_name(), what.describe());
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
