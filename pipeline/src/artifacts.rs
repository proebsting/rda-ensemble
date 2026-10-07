//! Where a stage's output goes.
//!
//! Every stage produces a value and hands it here. By default the value is
//! dropped after the next stage has used it; with `--keep` the same value is
//! also written to a file, in the format the `rdarust` CLI would have written
//! it. That is what makes the two comparable: an intermediate written here is
//! byte-for-byte what the stage-by-stage pipeline produces.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde_json::Value;

/// One intermediate a run can be asked to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Artifact {
    /// The data map naming which datasets and fields to extract.
    DataMap,
    /// The precinct adjacency graph.
    Graph,
    /// Precinct data: population, votes, demographics, shape summaries.
    Data,
    /// The ReCom dual graph, with the seed plan stamped on its nodes.
    RecomGraph,
    /// The seed plan on its own, as a precinct-assignment CSV.
    SeedPlan,
    /// Every sampled plan, as geoid-to-district records.
    Plans,
}

impl Artifact {
    /// The file this artifact is written to inside the output directory.
    pub fn file_name(self) -> &'static str {
        match self {
            Artifact::DataMap => "data_map.json",
            Artifact::Graph => "graph.json",
            Artifact::Data => "precinct_data.jsonl",
            Artifact::RecomGraph => "recom_graph.json",
            Artifact::SeedPlan => "seed_plan.csv",
            Artifact::Plans => "plans.jsonl",
        }
    }

    /// One line saying what the file holds, for the closing report.
    pub fn describe(self) -> &'static str {
        match self {
            Artifact::DataMap => "which datasets and fields were read",
            Artifact::Graph => "which precincts border which",
            Artifact::Data => "population, votes and shapes per precinct",
            Artifact::RecomGraph => "the chain's dual graph, seed plan included",
            Artifact::SeedPlan => "the starting plan the chain was given",
            Artifact::Plans => "the ensemble itself, as geoid to district",
        }
    }

    pub const ALL: [Artifact; 6] = [
        Artifact::DataMap,
        Artifact::Graph,
        Artifact::Data,
        Artifact::RecomGraph,
        Artifact::SeedPlan,
        Artifact::Plans,
    ];
}

impl fmt::Display for Artifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Artifact::DataMap => "data-map",
            Artifact::Graph => "graph",
            Artifact::Data => "data",
            Artifact::RecomGraph => "recom-graph",
            Artifact::SeedPlan => "seed-plan",
            Artifact::Plans => "plans",
        })
    }
}

/// The output directory, and which intermediates to keep in it.
pub struct Artifacts {
    dir: PathBuf,
    keep: BTreeSet<Artifact>,
}

impl Artifacts {
    pub fn new(dir: &Path, keep: &[Artifact]) -> Artifacts {
        Artifacts { dir: dir.to_path_buf(), keep: keep.iter().copied().collect() }
    }

    /// The same choices, writing into a different directory.
    ///
    /// A run of several chains writes the state-level artifacts once and the
    /// per-chain ones -- seed plan, ReCom graph, plans -- under each chain.
    pub fn relocated(&self, dir: &Path) -> Artifacts {
        Artifacts { dir: dir.to_path_buf(), keep: self.keep.clone() }
    }

    pub fn wants(&self, what: Artifact) -> bool {
        self.keep.contains(&what)
    }

    pub fn path(&self, what: Artifact) -> PathBuf {
        self.dir.join(what.file_name())
    }

    /// Open a writer for an artifact, or `None` if it was not asked for.
    pub fn writer(&self, what: Artifact) -> Result<Option<BufWriter<File>>> {
        if !self.wants(what) {
            return Ok(None);
        }
        let path = self.path(what);
        Ok(Some(BufWriter::new(
            File::create(&path).with_context(|| format!("creating {}", path.display()))?,
        )))
    }

    /// Write a JSON document the way the `rdarust` CLI writes it: Python's
    /// separators and no trailing newline, so the file round-trips through
    /// networkx and compares byte for byte against the CLI's output.
    pub fn write_json(&self, what: Artifact, value: &Value) -> Result<()> {
        let Some(mut w) = self.writer(what)? else { return Ok(()) };
        w.write_all(&rdarust_io::records::to_python_json(value)?)?;
        w.flush()?;
        Ok(())
    }

    /// Write JSON the way rdapy's `write_json` does, which is how the
    /// `rdarust` CLI writes the data map and the adjacency graph: four-space
    /// indent, no trailing newline.
    pub fn write_json_pretty(&self, what: Artifact, value: &Value) -> Result<()> {
        let Some(mut w) = self.writer(what)? else { return Ok(()) };
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut ser = serde_json::Serializer::with_formatter(&mut w, formatter);
        serde::Serialize::serialize(value, &mut ser)?;
        w.flush()?;
        Ok(())
    }

    /// Write one JSON record per line.
    pub fn write_records(&self, what: Artifact, records: &[Value]) -> Result<()> {
        let Some(mut w) = self.writer(what)? else { return Ok(()) };
        for record in records {
            rdarust_io::records::write_record(&mut w, record)?;
        }
        w.flush()?;
        Ok(())
    }
}
