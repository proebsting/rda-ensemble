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
use crate::{Cli, Variant};

/// Node attribute names on the ReCom graph. Fixed, because nothing outside
/// this binary reads them: the graph is built and consumed in one process.
const POP_COL: &str = "TOTAL_POP";
const ASSIGNMENT_COL: &str = "INITIAL";

pub fn run(cli: &Cli, keep: &[Artifact]) -> Result<()> {
    check(cli)?;
    let started = Instant::now();
    std::fs::create_dir_all(&cli.out)
        .with_context(|| format!("creating {}", cli.out.display()))?;
    let artifacts = Artifacts::new(&cli.out, keep);

    let ctx = Arc::new(read_state(cli, &artifacts)?);
    let (order, seed_assignments) = seed_plan(cli, &ctx, &artifacts)?;
    let order = Arc::new(order);
    chain(cli, &artifacts, ctx.clone(), order, &seed_assignments, started)
}

/// Parameter checks that would otherwise fail deep inside a library, or
/// worse, not fail at all.
fn check(cli: &Cli) -> Result<()> {
    if cli.districts < 2 {
        bail!("--districts must be at least 2; got {}", cli.districts);
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
    if cli.elections.is_empty() {
        bail!("--elections needs at least one dataset name, or `all`");
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
fn read_state(cli: &Cli, artifacts: &Artifacts) -> Result<Context> {
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
    // Check the dataset names before anything else reads them. A name that
    // is not in the file would otherwise sail through extraction and fail
    // much later, inside a scoring formula, where the message means nothing.
    check_datasets(&doc, cli, &elections)?;

    let mut warnings = Vec::new();
    let data_map = rdarust_io::map_data(
        &doc,
        &DataMapSpec {
            census: &cli.census,
            vap: &cli.vap,
            cvap: &cli.cvap,
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
        .into_context(&cli.state, &cli.plan_type, Some(cli.districts as u32))
        .map_err(|e| anyhow!("{e}"))?;
    Ok(ctx)
}

/// Fail on a dataset name the GeoJSON does not carry, and say what it does.
fn check_datasets(doc: &Value, cli: &Cli, elections: &[String]) -> Result<()> {
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

    for (flag, name, prefix) in [
        ("--census", &cli.census, "T_"),
        ("--vap", &cli.vap, "V_"),
        ("--cvap", &cli.cvap, "V_"),
    ] {
        if !available.contains_key(name.as_str()) {
            bail!(
                "{flag} {name} is not in the GeoJSON.\nAvailable: {}",
                listing(prefix)
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
                 Or pass `--elections all` to score every one.",
                missing.join(", "),
                listing("E_")
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
    cli: &Cli,
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

    eprintln!("drawing a starting plan with {} districts", cli.districts);
    let balanced = partigraph::partition_balanced(
        &graph,
        &weights,
        &partigraph::BalanceParams {
            parts: cli.districts,
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
    cli: &Cli,
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
    let names = RecomNames {
        geoid: "GEOID",
        pop: POP_COL,
        county: "COUNTY",
        assignment: ASSIGNMENT_COL,
    };
    let built = build_graph(&ctx, names, Some(&seed)).map_err(|e| anyhow!("{e}"))?;
    artifacts.write_json(Artifact::RecomGraph, &built.doc)?;

    let (graph, partition) = rustrecom::init::from_networkx_value(
        built.doc,
        POP_COL,
        ASSIGNMENT_COL,
        vec![],
        vec![],
        vec![],
    )
    .map_err(|e| anyhow!("handing the graph to rustrecom: {e}"))?;

    let ideal = graph.total_pop as f64 / partition.num_dists as f64;
    let params = RecomParams {
        min_pop: ((1.0 - cli.tolerance) * ideal).ceil() as u32,
        max_pop: ((1.0 + cli.tolerance) * ideal).floor() as u32,
        balance_ub: cli.balance_ub.unwrap_or(0),
        num_steps: cli.steps,
        rng_seed: cli.rng_seed,
        variant: variant_of(cli.variant),
        region_weights: None,
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
    manifest::write(cli, &ctx, &summary, started, &cli.out.join("manifest.json"))?;

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
        Variant::CutEdgesRmst => RecomVariant::CutEdgesRMST,
        Variant::DistrictPairsRmst => RecomVariant::DistrictPairsRMST,
    }
}
