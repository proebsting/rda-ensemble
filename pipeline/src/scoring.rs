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
//!
//! # Why the state is shared
//!
//! A chain may be run in segments, so that it can be stopped between them.
//! rustrecom takes ownership of the writer and closes it at the end of each
//! call, but the output files, the step cursor and the plan the chain is
//! sitting on all have to outlive a segment. They live in [`ChainState`],
//! behind the handle that each segment's writer holds. Without that, every
//! segment boundary would lose the self-loops straddling it and re-score the
//! plan the previous segment ended on.

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
    /// Whether the run was asked to stop before it had taken every step.
    /// A reader should not have to infer this by comparing the steps asked
    /// for against the steps taken.
    pub stopped: bool,
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

/// Everything about one chain that outlives a single segment.
pub struct ChainState {
    /// Reused across plans: aggregation is the bulk of scoring, and this
    /// keeps it from reallocating every step.
    aggs: Aggregates,

    csv: ScoresCsv<BufWriter<File>>,
    by_district: BufWriter<File>,
    plans: Option<BufWriter<File>>,

    /// The plan the chain is sitting on, in rustrecom's node order.
    previous: Vec<u32>,
    /// The last absolute step accounted for. Absolute, not per segment.
    last_step: u64,
    /// Whether step 0 -- the seed plan -- has been scored. Only the first
    /// segment scores it; for every later one, `init` is handed the plan the
    /// previous segment ended on, which is already in the output.
    started: bool,

    /// Drawn from the step numbers below. rustrecom's own bar is per call
    /// and has no callback, so this is the one the user sees.
    progress: crate::run::Progress,

    pub summary: Summary,
}

impl ChainState {
    pub fn new(
        ctx: &Context,
        scores: File,
        by_district: File,
        plans: Option<File>,
        progress: crate::run::Progress,
    ) -> ChainState {
        ChainState {
            aggs: Aggregates::new(ctx),
            csv: ScoresCsv::new(BufWriter::new(scores)),
            by_district: BufWriter::new(by_district),
            plans: plans.map(BufWriter::new),
            previous: Vec::new(),
            last_step: 0,
            started: false,
            progress,
            summary: Summary::default(),
        }
    }

    /// The plan the chain ended on, to start the next segment from.
    pub fn current_plan(&self) -> &[u32] {
        &self.previous
    }

    /// Clear the progress line, so the closing report starts clean.
    pub fn finish(&mut self) {
        self.progress.finish();
    }
}

/// One segment's view of the chain. Cheap to make: everything mutable is
/// behind the shared handle, and everything here is read-only configuration.
pub struct ScoringWriter {
    ctx: Arc<Context>,
    /// Precinct index for each ReCom node, so a partition maps back without
    /// a geoid lookup per precinct.
    order: Arc<Vec<u32>>,
    opts: ScoreOptions,
    mode: Mode,
    prefixes: bool,
    interval: u64,

    /// The absolute step this segment's local step 0 corresponds to.
    /// rustrecom numbers each call's steps from zero; the ensemble numbers
    /// them from the start of the chain.
    offset: u64,

    state: Arc<Mutex<ChainState>>,
}

impl ScoringWriter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: Arc<Context>,
        order: Arc<Vec<u32>>,
        opts: ScoreOptions,
        prefixes: bool,
        interval: u64,
        offset: u64,
        state: Arc<Mutex<ChainState>>,
    ) -> ScoringWriter {
        let mode = *opts.mode;
        ScoringWriter {
            ctx,
            order,
            opts,
            mode,
            prefixes,
            interval,
            offset,
            state,
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

    fn score(
        &self,
        st: &mut ChainState,
        step: u64,
        assignments: &[u32],
    ) -> anyhow::Result<()> {
        // A failure earlier in the run means the outputs are already
        // incomplete; do not pile more onto them.
        if st.summary.error.is_some() {
            return Ok(());
        }
        let name = format!("{step:06}");
        let plan = self.plan_of(assignments);

        let card = match self.ctx.score_into(&plan, &self.opts, &mut st.aggs) {
            Ok(card) => card,
            Err(e) => {
                st.summary.error = Some(format!("step {step}: {e}"));
                return Ok(());
            }
        };
        let scores = scorecard_to_value(&card, &self.ctx.keys, self.mode);
        let by_district = scored_aggregates_to_value(&st.aggs, &self.ctx, self.mode);

        st.csv.write(&name, &scores, self.prefixes)?;
        self.record(st, &scores);

        let mut rec = Map::new();
        rec.insert("_tag_".into(), json!("by-district"));
        rec.insert("name".into(), json!(name));
        rec.insert("by-district".into(), by_district);
        write_record_sorted(&mut st.by_district, &Value::Object(rec))?;

        if let Some(plans) = &mut st.plans {
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

        st.summary.scored += 1;
        st.summary.steps = st.summary.steps.max(step);
        Ok(())
    }

    /// Keep each numeric score for the diagnostics.
    fn record(&self, st: &mut ChainState, scores: &Value) {
        let flat = flatten_scores(scores, self.prefixes);
        for (name, value) in flat {
            if let Some(v) = value.as_f64() {
                st.summary.series.entry(name).or_default().push(v);
            }
        }
    }

    /// Emit the plan that was already in place for every sampled step in
    /// `start..=end`. A self-looped step is a real sample of the chain.
    fn replay(&self, st: &mut ChainState, start: u64, end: u64) -> anyhow::Result<()> {
        let steps: Vec<u64> = sampled_steps(start, end, self.interval).collect();
        if steps.is_empty() {
            return Ok(());
        }
        let previous = std::mem::take(&mut st.previous);
        for step in steps {
            self.score(st, step, &previous)?;
        }
        st.previous = previous;
        Ok(())
    }
}

impl StatsWriter for ScoringWriter {
    fn init(&mut self, _graph: &Graph, partition: &Partition) -> std::io::Result<()> {
        let mut st = self.state.lock().expect("chain state");
        st.previous = partition.assignments.clone();
        // Only the first segment has a seed plan to score. A later segment is
        // handed the plan the previous one ended on, which was scored there;
        // scoring it again would duplicate a row and double-count a step.
        if st.started {
            return Ok(());
        }
        st.started = true;
        let seed = st.previous.clone();
        // Step 0 is the seed plan, always sampled, as rustrecom does.
        self.score(&mut st, 0, &seed).map_err(to_io)?;
        st.last_step = 0;
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
        let mut st = self.state.lock().expect("chain state");
        let step = self.offset + step;
        // Steps between the last accepted one and this were self-loops.
        let from = st.last_step + 1;
        self.replay(&mut st, from, step.saturating_sub(1)).map_err(to_io)?;
        st.previous = partition.assignments.clone();
        if self.interval != 0 && step % self.interval == 0 {
            let now = st.previous.clone();
            self.score(&mut st, step, &now).map_err(to_io)?;
        }
        st.last_step = step;
        st.progress.advance(step);
        Ok(())
    }

    fn self_loop(
        &mut self,
        step: u64,
        _graph: &Graph,
        _partition: &Partition,
        _counts: &SelfLoopCounts,
    ) -> std::io::Result<()> {
        let mut st = self.state.lock().expect("chain state");
        let step = self.offset + step;
        let from = st.last_step + 1;
        self.replay(&mut st, from, step).map_err(to_io)?;
        st.last_step = step;
        st.progress.advance(step);
        Ok(())
    }

    /// Called at the end of every segment, not only the last. Flushing here
    /// is what leaves a cancelled run with usable output on disk.
    fn close(&mut self) -> std::io::Result<()> {
        let mut st = self.state.lock().expect("chain state");
        st.csv.flush()?;
        st.by_district.flush()?;
        if let Some(plans) = &mut st.plans {
            plans.flush()?;
        }
        let last = st.last_step;
        st.summary.steps = last;
        Ok(())
    }
}

fn to_io(e: anyhow::Error) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_grid_is_absolute_not_per_segment() {
        // A segment starting at absolute 97, sampling every 50: the next
        // sampled step is 100, not 97 + 50. This is what the offset buys.
        let got: Vec<u64> = sampled_steps(98, 160, 50).collect();
        assert_eq!(got, vec![100, 150]);
    }

    #[test]
    fn a_segment_with_no_sampled_steps_emits_nothing() {
        let got: Vec<u64> = sampled_steps(101, 149, 50).collect();
        assert!(got.is_empty());
    }
}
