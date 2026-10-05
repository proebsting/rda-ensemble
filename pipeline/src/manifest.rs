//! What the run was, written beside what it produced.
//!
//! An ensemble without its parameters cannot be checked or regenerated, and
//! in a single-command tool there is no shell history to fall back on. The
//! manifest is the record, so it is always written, not an option.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context as _, Result};
use rdarust_core::context::Context;
use serde_json::{json, Map, Value};

use crate::scoring::Summary;
use crate::RunArgs;

/// A cheap, stable fingerprint of the input file.
///
/// FNV-1a over the bytes. Not a cryptographic hash and not claimed to be one:
/// it answers "is this the same file I ran on", which is what a reader needs.
fn fingerprint(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}

#[allow(clippy::too_many_arguments)]
pub fn write(
    cli: &RunArgs,
    districts: usize,
    steps: u64,
    package: &crate::dra::Package,
    ctx: &Context,
    summary: &Summary,
    started: Instant,
    path: &Path,
) -> Result<()> {
    let mut input = Map::new();
    input.insert("geojson".into(), json!(package.geojson.display().to_string()));
    input.insert(
        "geojson_fingerprint".into(),
        match fingerprint(&package.geojson) {
            Ok(f) => json!(f),
            Err(_) => Value::Null,
        },
    );
    // Which DRA package this came from, and where adjacency came from --
    // neither is recoverable from the file path alone.
    input.insert("dra_version".into(), json!(package.version));
    input.insert(
        "adjacency".into(),
        json!(match (cli.adjacency, &package.graph) {
            (crate::Adjacency::Geometry, _) => "geometry",
            (_, Some(_)) => "dra",
            (_, None) => "geometry",
        }),
    );
    input.insert("state".into(), json!(cli.state));
    input.insert("plan_type".into(), json!(cli.chamber.as_str()));
    input.insert("districts".into(), json!(districts));
    // Whether the count came from the statutory table or the command line,
    // because a reader cannot tell them apart from the number alone.
    input.insert(
        "districts_from".into(),
        json!(if cli.districts.is_some() { "--districts" } else { "statutory" }),
    );
    input.insert("precincts".into(), json!(ctx.n_precincts()));
    // The datasets actually used, not the flags asked for: --cycle leaves
    // the flags unset, and a manifest that cannot say which data produced
    // an ensemble is not worth writing.
    input.insert("cycle".into(), match cli.cycle {
        Some(year) => json!(year),
        None => Value::Null,
    });
    input.insert("census".into(), json!(ctx.keys.census));
    input.insert("vap".into(), json!(ctx.keys.vap));
    input.insert("cvap".into(), json!(ctx.keys.cvap));
    input.insert("elections".into(), json!(ctx.keys.elections));
    // rdapy labels the shape section of a data map this way; the geometry
    // itself comes from the GeoJSON, not from a dataset of that name.
    input.insert("shapes_label".into(), json!(ctx.keys.shapes));
    input.insert("expand_composites".into(), json!(cli.expand_composites));

    let mut chain = Map::new();
    chain.insert("steps_requested".into(), json!(steps));
    chain.insert("plans_requested".into(), match cli.plans {
        Some(p) => json!(p),
        None => Value::Null,
    });
    chain.insert("steps_taken".into(), json!(summary.steps));
    // Said outright, because an ensemble that stopped early is not the
    // ensemble that was asked for and nothing else in the file says so.
    chain.insert("stopped_early".into(), json!(summary.stopped));
    chain.insert("variant".into(), json!(cli.variant.as_str()));
    chain.insert("tolerance".into(), json!(cli.tolerance));
    chain.insert("seed_tolerance".into(), json!(cli.seed_tolerance));
    chain.insert("rng_seed".into(), json!(cli.rng_seed));
    chain.insert("balance_ub".into(), match cli.balance_ub {
        Some(m) => json!(m),
        None => Value::Null,
    });
    chain.insert("region_weights".into(), json!(cli.region_weights));
    chain.insert("target_pop".into(), match cli.target_pop {
        Some(p) => json!(p),
        None => Value::Null,
    });
    // Load-bearing for reproduction: each segment draws its own derived
    // seed, so the segment length is part of what produced this ensemble.
    chain.insert("segment_steps".into(), match cli.segment_steps {
        Some(n) => json!(n),
        None => Value::Null,
    });
    chain.insert("threads".into(), json!(cli.threads));
    chain.insert("batch_size".into(), json!(cli.batch_size));

    let mut output = Map::new();
    output.insert("sample_every".into(), json!(cli.sample_every));
    output.insert("plans_scored".into(), json!(summary.scored));
    output.insert("prefixed_columns".into(), json!(cli.prefixes));
    // Scoring choices the command line does not have to mention. Without
    // them the manifest cannot say which columns an ensemble has.
    output.insert("majority_minority".into(), json!(true));
    output.insert(
        "reverse_weight_splitting".into(),
        json!(cli.reverse_weight_splitting),
    );
    output.insert("score_mode".into(), json!("all"));
    output.insert("seconds".into(), json!(started.elapsed().as_secs_f64()));

    let mut versions = Map::new();
    versions.insert("rda-ensemble".into(), json!(env!("CARGO_PKG_VERSION")));
    versions.insert("rdarust".into(), json!(rdarust_version()));

    let mut doc = Map::new();
    doc.insert("tool".into(), json!("rda-ensemble"));
    doc.insert("input".into(), Value::Object(input));
    doc.insert("chain".into(), Value::Object(chain));
    doc.insert("output".into(), Value::Object(output));
    doc.insert("versions".into(), Value::Object(versions));

    let mut file =
        File::create(path).with_context(|| format!("creating {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, &Value::Object(doc))?;
    file.write_all(b"\n")?;
    Ok(())
}

/// rdarust's version, as this binary was built against it.
fn rdarust_version() -> &'static str {
    // The path dependency has no version of its own at runtime; this is the
    // one the workspace declares.
    "0.1.0"
}
