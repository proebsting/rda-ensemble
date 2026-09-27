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
use crate::Cli;

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

pub fn write(
    cli: &Cli,
    ctx: &Context,
    summary: &Summary,
    started: Instant,
    path: &Path,
) -> Result<()> {
    let mut input = Map::new();
    input.insert("geojson".into(), json!(cli.geojson.display().to_string()));
    input.insert(
        "geojson_fingerprint".into(),
        match fingerprint(&cli.geojson) {
            Ok(f) => json!(f),
            Err(_) => Value::Null,
        },
    );
    input.insert("state".into(), json!(cli.state));
    input.insert("plan_type".into(), json!(cli.plan_type));
    input.insert("districts".into(), json!(cli.districts));
    input.insert("precincts".into(), json!(ctx.n_precincts()));
    input.insert("census".into(), json!(cli.census));
    input.insert("vap".into(), json!(cli.vap));
    input.insert("cvap".into(), json!(cli.cvap));
    input.insert("elections".into(), json!(ctx.keys.elections));
    input.insert("expand_composites".into(), json!(cli.expand_composites));

    let mut chain = Map::new();
    chain.insert("steps_requested".into(), json!(cli.steps));
    chain.insert("steps_taken".into(), json!(summary.steps));
    chain.insert("variant".into(), json!(cli.variant.as_str()));
    chain.insert("tolerance".into(), json!(cli.tolerance));
    chain.insert("seed_tolerance".into(), json!(cli.seed_tolerance));
    chain.insert("rng_seed".into(), json!(cli.rng_seed));
    chain.insert("balance_ub".into(), match cli.balance_ub {
        Some(m) => json!(m),
        None => Value::Null,
    });
    chain.insert("threads".into(), json!(cli.threads));
    chain.insert("batch_size".into(), json!(cli.batch_size));

    let mut output = Map::new();
    output.insert("sample_every".into(), json!(cli.sample_every));
    output.insert("plans_scored".into(), json!(summary.scored));
    output.insert("prefixed_columns".into(), json!(cli.prefixes));
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
