//! Scoring plans as the chain produces them.
//!
//! This is the seam that makes the whole thing one process. rustrecom hands
//! every accepted proposal to a [`StatsWriter`]; this one converts the
//! partition to an rdarust plan and scores it there and then, rather than
//! serialising it for a later pass. No ensemble is ever held in memory.
//!
//! # What "every Nth plan" means
//!
//! A ReCom chain self-loops: a step that rejects its proposal leaves the plan
//! unchanged, and for the chain's stationary distribution to be right that
//! repeat has to be counted again. So sampling is by *chain step*, not by
//! distinct plan, and a self-looped step re-emits the plan that was already
//! there. This mirrors rustrecom's own `--sample-interval` exactly, so
//! `--sample-every N` here and `--sample-interval N` there select the same
//! steps.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};

use rdarust_core::aggregate::{Aggregates, Mode};
use rdarust_core::context::{Context, UNASSIGNED};
use rdarust_core::score::ScoreOptions;
use rdarust_io::{
    flatten_scores, records::write_record_sorted, scorecard_to_value,
    scored_aggregates_to_value, ScoresCsv,
};
use rustrecom::graph::Graph;
use rustrecom::partition::Partition;
use rustrecom::recom::RecomProposal;
use rustrecom::stats::SelfLoopCounts;
use rustrecom::stats::StatsWriter;
use serde_json::{json, Map, Value};

/// What a finished run reports.
#[derive(Debug, Default)]
pub struct Summary {
    /// Chain steps taken, counting self-loops.
    pub steps: u64,
    /// Plans scored, which is the number of sampled steps.
    pub scored: u64,
    /// The first error scoring hit, if any. The chain cannot be stopped from
    /// inside a writer, so the rest of the run is skipped rather than scored.
    pub error: Option<String>,
    /// Every numeric score, in sampling order, for the convergence
    /// diagnostics. Ten thousand plans of forty-odd columns is a few
    /// megabytes, so this is kept rather than re-read from the CSV.
    pub series: BTreeMap<String, Vec<f64>>,
}

/// The steps at or after `start`, up to `end`, that are multiples of
/// `interval`. Mirrors rustrecom's own sampling so the two agree.
fn sampled_steps(start: u64, end: u64, interval: u64) -> impl Iterator<Item = u64> {
    let offset = (interval - start % interval) % interval;
    let first = start.checked_add(offset).filter(move |&step| step <= end);
    std::iter::successors(first, move |&step| {
        step.checked_add(interval).filter(|&next| next <= end)
    })
}

pub struct ScoringWriter {
    ctx: Arc<Context>,
    /// Precinct index for each ReCom node, so a partition maps back without
    /// a geoid lookup per precinct.
    order: Arc<Vec<u32>>,
    opts: ScoreOptions,
    mode: Mode,
    prefixes: bool,
    /// Reused across plans: aggregation is the bulk of scoring, and this
    /// keeps it from reallocating every step.
    aggs: Aggregates,

    interval: u64,
    previous: Vec<u32>,
    last_step: u64,

    csv: ScoresCsv<BufWriter<File>>,
    by_district: BufWriter<File>,
    plans: Option<BufWriter<File>>,
    summary: Arc<Mutex<Summary>>,
}

impl ScoringWriter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: Arc<Context>,
        order: Arc<Vec<u32>>,
        opts: ScoreOptions,
        prefixes: bool,
        interval: u64,
        scores: File,
        by_district: File,
        plans: Option<File>,
        summary: Arc<Mutex<Summary>>,
    ) -> ScoringWriter {
        let aggs = Aggregates::new(&ctx);
        let mode = *opts.mode;
        ScoringWriter {
            ctx,
            order,
            opts,
            mode,
            prefixes,
            aggs,
            interval,
            previous: Vec::new(),
            last_step: 0,
            csv: ScoresCsv::new(BufWriter::new(scores)),
            by_district: BufWriter::new(by_district),
            plans: plans.map(BufWriter::new),
            summary,
        }
    }

    /// A ReCom assignment as an rdarust plan.
    ///
    /// ReCom numbers districts from 0 and indexes nodes by position; rdarust
    /// numbers districts from 1, reserving 0 for unassigned, and indexes by
    /// precinct. `order` carries the one mapping, so this is a pair of
    /// indexed reads rather than a hash lookup per precinct.
    fn plan_of(&self, assignments: &[u32]) -> Vec<u32> {
        let mut plan = vec![UNASSIGNED; self.ctx.adjacency.len()];
        for (node, &district) in assignments.iter().enumerate() {
            plan[self.order[node] as usize] = district + 1;
        }
        plan
    }

    fn score(&mut self, step: u64, assignments: &[u32]) -> anyhow::Result<()> {
        // A failure earlier in the run means the outputs are already
        // incomplete; do not pile more onto them.
        if self.summary.lock().expect("summary").error.is_some() {
            return Ok(());
        }
        let name = format!("{step:06}");
        let plan = self.plan_of(assignments);

        let card = match self.ctx.score_into(&plan, &self.opts, &mut self.aggs) {
            Ok(card) => card,
            Err(e) => {
                self.summary.lock().expect("summary").error = Some(format!("step {step}: {e}"));
                return Ok(());
            }
        };
        let scores = scorecard_to_value(&card, &self.ctx.keys, self.mode);
        let by_district = scored_aggregates_to_value(&self.aggs, &self.ctx, self.mode);

        self.csv.write(&name, &scores, self.prefixes)?;
        self.record(&scores);

        let mut rec = Map::new();
        rec.insert("_tag_".into(), json!("by-district"));
        rec.insert("name".into(), json!(name));
        rec.insert("by-district".into(), by_district);
        write_record_sorted(&mut self.by_district, &Value::Object(rec))?;

        if let Some(plans) = &mut self.plans {
            let mut assigned = Map::new();
            for (node, &district) in assignments.iter().enumerate() {
                let geoid = &self.ctx.geoids[self.order[node] as usize];
                assigned.insert(geoid.clone(), json!(district + 1));
            }
            let mut rec = Map::new();
            rec.insert("_tag_".into(), json!("plan"));
            rec.insert("name".into(), json!(name));
            rec.insert("plan".into(), Value::Object(assigned));
            rdarust_io::records::write_record(plans, &Value::Object(rec))?;
        }

        let mut summary = self.summary.lock().expect("summary");
        summary.scored += 1;
        summary.steps = summary.steps.max(step);
        Ok(())
    }

    /// Keep each numeric score for the diagnostics.
    fn record(&mut self, scores: &Value) {
        let flat = flatten_scores(scores, self.prefixes);
        let mut summary = self.summary.lock().expect("summary");
        for (name, value) in flat {
            if let Some(v) = value.as_f64() {
                summary.series.entry(name).or_default().push(v);
            }
        }
    }

    /// Emit the plan that was already in place for every sampled step in
    /// `start..=end`. A self-looped step is a real sample of the chain.
    fn replay(&mut self, start: u64, end: u64) -> anyhow::Result<()> {
        let steps: Vec<u64> = sampled_steps(start, end, self.interval).collect();
        if steps.is_empty() {
            return Ok(());
        }
        let previous = std::mem::take(&mut self.previous);
        for step in steps {
            self.score(step, &previous)?;
        }
        self.previous = previous;
        Ok(())
    }
}

impl StatsWriter for ScoringWriter {
    fn init(&mut self, _graph: &Graph, partition: &Partition) -> std::io::Result<()> {
        self.previous = partition.assignments.clone();
        let seed = self.previous.clone();
        // Step 0 is the seed plan, always sampled, as rustrecom does.
        self.score(0, &seed).map_err(to_io)?;
        self.last_step = 0;
        Ok(())
    }

    fn step(
        &mut self,
        step: u64,
        _graph: &Graph,
        partition: &Partition,
        _proposal: &RecomProposal,
        _counts: &SelfLoopCounts,
    ) -> std::io::Result<()> {
        // Steps between the last accepted one and this were self-loops.
        self.replay(self.last_step + 1, step.saturating_sub(1)).map_err(to_io)?;
        self.previous = partition.assignments.clone();
        if self.interval != 0 && step % self.interval == 0 {
            let now = self.previous.clone();
            self.score(step, &now).map_err(to_io)?;
        }
        self.last_step = step;
        Ok(())
    }

    fn self_loop(
        &mut self,
        step: u64,
        _graph: &Graph,
        _partition: &Partition,
        _counts: &SelfLoopCounts,
    ) -> std::io::Result<()> {
        self.replay(self.last_step + 1, step).map_err(to_io)?;
        self.last_step = step;
        Ok(())
    }

    fn close(&mut self) -> std::io::Result<()> {
        self.csv.flush()?;
        self.by_district.flush()?;
        if let Some(plans) = &mut self.plans {
            plans.flush()?;
        }
        self.summary.lock().expect("summary").steps = self.last_step;
        Ok(())
    }
}

fn to_io(e: anyhow::Error) -> std::io::Error {
    std::io::Error::other(e.to_string())
}
