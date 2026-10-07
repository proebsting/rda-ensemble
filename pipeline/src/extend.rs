//! Running an existing ensemble's chain for longer.
//!
//! A ReCom chain is Markov: the next plan depends on the current one and
//! nothing else. So picking up from the plan an earlier run ended on
//! continues that chain, rather than starting a second chain that happens
//! to begin somewhere plausible. The ensemble this produces is the one the
//! earlier run would have produced had it been asked for more steps --
//! with one caveat, below.
//!
//! Three things have to come across for that to be true. The plan itself,
//! which is why the earlier run must have kept its plans. The step it sat
//! at, so the combined `scores.csv` numbers its rows as one chain. And how
//! many segments were already drawn, because seeds are derived from
//! `(seed, chain, segment)` and an extension that restarted that count
//! would replay the earlier run's random stream.
//!
//! # Exactly the ensemble a longer run would have produced
//!
//! A segmented run puts its boundaries at exact multiples of the segment
//! length, and each segment's seed is derived from its index. So a 300-step
//! run and a 500-step run share segments 0, 1 and 2 *identically*: segment 2
//! starts at step 200 from the same plan with the same seed in both, and the
//! longer run merely runs it further. The two only diverge where the shorter
//! one stopped in the middle of a segment.
//!
//! So an extension that resumes at a **boundary** -- rather than wherever
//! the earlier run happened to stop -- continues the same random stream, and
//! the combined ensemble is byte-identical to one long run. That means
//! backing up: the steps between the last boundary and where the run stopped
//! are regenerated. Nothing is lost by this, because they regenerate
//! identically; it is a prefix of the same stream.
//!
//! Two conditions. The earlier run has to have been segmented, since an
//! unsegmented chain is one continuous stream with no boundary to rejoin.
//! And the segment length has to be a multiple of the sampling interval, or
//! the plan at a boundary was never written down.
//!
//! When either fails, the extension still continues the chain -- ReCom is
//! Markov, so resuming from the last saved plan is a valid continuation --
//! but it draws a fresh stream and will not match a single longer run. It
//! says which of the two it did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::Value;

use crate::events::Sink;
use crate::run::{self, Cancel, Extension};
use crate::settings::Settings;
use crate::KeepArg;

/// What an earlier run left behind that an extension needs.
struct Earlier {
    settings: Settings,
    plan: HashMap<String, u32>,
    from_step: u64,
    segment_base: u64,
    /// Whether resuming here reproduces what one longer run would have
    /// drawn. True when `from_step` is a segment boundary.
    exact: bool,
    /// Why not, when it is not.
    why_not: Option<String>,
    /// Steps the earlier ensemble covers, counting from zero. What "200
    /// more" is measured against -- not the resume point, which may be
    /// behind it.
    base_steps: u64,
}

#[allow(clippy::too_many_arguments)]
pub fn extend(
    from: &Path,
    out: &Path,
    plans: Option<u64>,
    steps: Option<u64>,
    segment_steps: Option<u64>,
    cache: Option<PathBuf>,
    ev: &Sink,
    cancel: &Cancel,
) -> Result<()> {
    let earlier = read_earlier(from)?;

    let more = match (steps, plans) {
        (Some(s), _) => s,
        (None, Some(p)) => p
            .checked_mul(earlier.settings.sample_every)
            .ok_or_else(|| anyhow!("--plans {p} overflows at this sampling rate"))?,
        (None, None) => bail!("give --plans or --steps: how much longer to run"),
    };
    // What the ensemble should cover when this is done. Measured from what
    // it already covers, not from the resume point: backing up to a boundary
    // is an implementation detail and must not change what "200 more" means.
    let target = earlier.base_steps + more;
    let this_run = target
        .checked_sub(earlier.from_step)
        .filter(|n| *n > 0)
        .ok_or_else(|| anyhow!("that is not longer than the ensemble already is"))?;

    // Changing the segment length moves every boundary, so the seeds stop
    // lining up with what a single longer run would have drawn. Decide this
    // before claiming anything about the result.
    let changed_segments = segment_steps.is_some_and(|n| earlier.settings.segment_steps != Some(n));
    let exact = earlier.exact && !changed_segments;

    ev.status(&format!(
        "extending {} by {more} steps, to {target}, sampling every {}",
        from.display(),
        earlier.settings.sample_every
    ));
    if exact {
        ev.status(&format!(
            "  resuming at step {}, a segment boundary: the result is what one \
             run of {target} steps would have produced",
            earlier.from_step,
        ));
    } else if changed_segments {
        ev.warn(&format!(
            "--segment-steps {} differs from the {} this ensemble was built with, \
             which moves every boundary. Resuming from step {} on a fresh random \
             stream: a valid continuation of the same chain, but not identical to \
             one longer run, and later extensions will not line up with it.",
            segment_steps.expect("changed"),
            earlier
                .settings
                .segment_steps
                .map_or("none".to_string(), |v| v.to_string()),
            earlier.from_step
        ));
    } else if let Some(why) = &earlier.why_not {
        ev.warn(&format!(
            "{why} Resuming from step {} on a fresh random stream: a valid \
             continuation of the same chain, but not identical to one longer run.",
            earlier.from_step
        ));
    }

    // Anything the earlier run wrote past the resume point is regenerated,
    // identically, by the segment this one starts with.
    copy_results(from, out, earlier.from_step)?;

    let mut args = earlier
        .settings
        .clone()
        .into_args(out.to_path_buf(), cache)?;
    // What to run now, as opposed to what the earlier run ran.
    args.steps = Some(this_run);
    args.plans = None;
    // Keeping the earlier run's segmentation is what keeps the boundaries
    // where the next extension will expect them. Overriding it is allowed,
    // and breaks that.
    args.segment_steps = segment_steps.or(earlier.settings.segment_steps);
    // The plans have to keep being written, or this ensemble cannot be
    // extended again.
    if !args.keep.contains(&KeepArg::Plans) && !args.keep.contains(&KeepArg::All) {
        args.keep.push(KeepArg::Plans);
    }

    let extension = Extension {
        plan: earlier.plan,
        from_step: earlier.from_step,
        segment_base: earlier.segment_base,
    };
    let keep = KeepArg::expand(&args.keep);
    run::run_extended(&args, &keep, cancel, ev, Some(&extension))
}

/// Read the settings, the final plan and the segment count out of a
/// finished run.
fn read_earlier(dir: &Path) -> Result<Earlier> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    if dir.join("chain_1").is_dir() {
        bail!(
            "{} holds several chains, and extending one of several is not \
             supported yet: which chain an R-hat was computed over would no \
             longer be a single question. Extend a single-chain run.",
            dir.display()
        );
    }

    let settings = Settings::read(&settings_path(dir)?)?;
    if settings.chains > 1 {
        bail!("that run had {} chains; only single-chain runs extend", settings.chains);
    }

    let plans = dir.join("plans.jsonl");
    if crate::squeeze::locate(&plans).is_none() {
        bail!(
            "{} has no plans.jsonl, so there is no plan to carry on from. \
             An ensemble has to be made with `--keep plans` to be extendable.",
            dir.display()
        );
    }
    // Where to rejoin the stream.
    //
    // Divisibility between the segment length and the sampling interval does
    // not decide this, and an earlier version that tested it got the test
    // backwards: segments of 500 with sampling every 2000 writes a plan at
    // *every* boundary, which is the best case, and was rejected as the
    // worst. What matters is only whether some saved plan sits on a multiple
    // of the segment length, so the honest way to find out is to look.
    let (step, plan, why_not) = match settings.segment_steps {
        None => {
            let (step, plan) = resume_point(&plans, None)?;
            (
                step,
                plan,
                Some(
                    "That ensemble was not segmented, so it has no boundary to \
                     rejoin."
                        .to_string(),
                ),
            )
        }
        Some(length) => match resume_point(&plans, Some(length)) {
            Ok((step, plan)) => (step, plan, None),
            // Step 0 is both a boundary and always saved, so this is close to
            // unreachable; honour it rather than rely on that.
            Err(_) => {
                let (step, plan) = resume_point(&plans, None)?;
                (step, plan, Some(format!(
                    "No saved plan of that ensemble sits on a multiple of its \
                     segment length {length}."
                )))
            }
        },
    };
    let boundary = why_not.is_none().then_some(()).and(settings.segment_steps);

    // Steps the earlier run covers. The manifest records the last step it
    // reached; one more than that is its length.
    let base_steps = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|m| m.get("chain")?.get("steps_taken")?.as_u64())
        .map(|last| last + 1)
        .or(settings.steps)
        .ok_or_else(|| anyhow!(
            "{} does not record how many steps that run took, so there is \
             nothing to measure an extension against",
            dir.display()
        ))?;

    Ok(Earlier {
        base_steps,
        segment_base: match boundary {
            // The segment starting at B is the one a longer run numbers B/L,
            // which is what makes the seeds line up.
            Some(length) => step / length,
            None => segments_taken(dir),
        },
        settings,
        plan,
        from_step: step,
        exact: boundary.is_some(),
        why_not,
    })
}

/// Where a finished run's parameters are written down: its own settings
/// file, or failing that the manifest beside the results.
pub fn record_of(dir: &Path) -> Result<PathBuf> {
    settings_path(dir)
}

fn settings_path(dir: &Path) -> Result<PathBuf> {
    for name in ["settings.json", "manifest.json"] {
        let path = dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    bail!(
        "{} has neither settings.json nor manifest.json, so what that run \
         was is not recorded anywhere",
        dir.display()
    )
}

/// How many segments the earlier run drew, so this one starts past them.
///
/// Absent from an older manifest, in which case one segment is assumed:
/// that is what an unsegmented run used, and assuming too few would reuse
/// a stream while assuming too many only skips ahead.
fn segments_taken(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|m| m.get("chain")?.get("segments_taken")?.as_u64())
        .unwrap_or(1)
}

/// The step to carry on from, and the plan that was there.
///
/// With a segment length, the last saved plan that sits on a boundary;
/// without one, simply the last saved plan.
fn resume_point(path: &Path, segment: Option<u64>) -> Result<(u64, HashMap<String, u32>)> {
    use std::io::BufRead;
    // Compressed or not, under that name or a compressed one.
    let reader = crate::squeeze::open(path)?;
    let mut best: Option<(u64, String)> = None;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(step) = step_of(&line) else { continue };
        if segment.is_some_and(|length| length == 0 || step % length != 0) {
            continue;
        }
        best = Some((step, line));
    }
    let (step, line) = best.ok_or_else(|| match segment {
        Some(length) => anyhow!(
            "{} holds no plan on a segment boundary (a multiple of {length}), \
             so there is nowhere to rejoin the chain's random stream",
            path.display()
        ),
        None => anyhow!("{} holds no plans", path.display()),
    })?;
    let doc: Value = serde_json::from_str(&line)
        .with_context(|| format!("a record of {} is not JSON", path.display()))?;
    let plan = doc
        .get("plan")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow!("that record is not a plan"))?;
    let mut out = HashMap::with_capacity(plan.len());
    for (geoid, district) in plan {
        let d = district
            .as_u64()
            .ok_or_else(|| anyhow!("district for {geoid} is not a number"))?;
        out.insert(geoid.clone(), d as u32);
    }
    Ok((step, out))
}

/// The step number a tagged record carries, if it has one.
fn step_of(line: &str) -> Option<u64> {
    let doc: Value = serde_json::from_str(line).ok()?;
    doc.get("name")?.as_str()?.parse().ok()
}

/// Copy the earlier results into the new directory, keeping rows up to and
/// including `through` and dropping anything after it.
///
/// What is dropped is regenerated by the segment this run starts with, from
/// the same plan with the same seed, so it comes back identical. The copy
/// leaves the original directory exactly as it was.
fn copy_results(from: &Path, out: &Path, through: u64) -> Result<()> {
    use std::io::{BufRead, Write};
    if from == out {
        bail!(
            "--from and --out are the same directory. The extension is \
             written to a copy so the original ensemble survives being \
             extended; name somewhere else."
        );
    }
    std::fs::create_dir_all(out)
        .with_context(|| format!("creating {}", out.display()))?;

    for name in ["scores.csv", "by_district.jsonl", "plans.jsonl"] {
        let src = from.join(name);
        if crate::squeeze::locate(&src).is_none() {
            continue;
        }
        let reader = crate::squeeze::open(&src)?;
        // Written back out plain: the run appends to these, and the final
        // pass compresses them again.
        let dest = out.join(name);
        let mut writer = std::io::BufWriter::new(
            std::fs::File::create(&dest)
                .with_context(|| format!("creating {}", dest.display()))?,
        );
        let csv = name.ends_with(".csv");
        for (n, line) in reader.lines().enumerate() {
            let line = line?;
            // The CSV header has no step of its own and always goes through.
            let keep = if csv && n == 0 {
                true
            } else if csv {
                line.split(',')
                    .next()
                    .and_then(|f| f.trim().parse::<u64>().ok())
                    .is_some_and(|step| step <= through)
            } else if line.trim().is_empty() {
                false
            } else {
                step_of(&line).is_some_and(|step| step <= through)
            };
            if keep {
                writer.write_all(line.as_bytes())?;
                // scores.csv is written with CRLF by the csv crate, and
                // appending to a file with mixed endings makes a mess.
                writer.write_all(if csv { b"\r\n" } else { b"\n" })?;
            }
        }
        writer.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rda-extend-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn plans_file(dir: &Path, steps: &[u64]) -> PathBuf {
        let path = dir.join("plans.jsonl");
        let body: String = steps
            .iter()
            .map(|s| {
                format!("{{\"_tag_\":\"plan\",\"name\":\"{s:06}\",\"plan\":{{\"a\":1,\"b\":2}}}}\n")
            })
            .collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Unsegmented, there is no boundary, so the last saved plan is it.
    #[test]
    fn without_segments_the_last_plan_is_the_one_carried_over() {
        let dir = scratch("last");
        let path = plans_file(&dir, &[0, 100, 200, 290]);
        let (step, plan) = resume_point(&path, None).expect("reads");
        assert_eq!(step, 290);
        assert_eq!(plan.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Segmented, the extension backs up to the last boundary so that the
    /// random stream it rejoins is the one a longer run would have drawn.
    #[test]
    fn with_segments_it_backs_up_to_a_boundary() {
        let dir = scratch("boundary");
        let path = plans_file(&dir, &[0, 100, 200, 290]);
        assert_eq!(resume_point(&path, Some(100)).expect("reads").0, 200);
        // Already on a boundary: nothing to back up over.
        let path = plans_file(&dir, &[0, 100, 200]);
        assert_eq!(resume_point(&path, Some(100)).expect("reads").0, 200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A run too short to have reached a boundary other than its start
    /// rejoins at step 0, which is still correct, just unhelpful.
    #[test]
    fn a_run_shorter_than_one_segment_rejoins_at_the_start() {
        let dir = scratch("short");
        let path = plans_file(&dir, &[0, 10, 20]);
        assert_eq!(resume_point(&path, Some(100)).expect("reads").0, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_plans_file_is_refused() {
        let dir = scratch("empty");
        let path = dir.join("plans.jsonl");
        std::fs::write(&path, "\n\n").unwrap();
        assert!(resume_point(&path, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Dropping what comes after the resume point is what makes the
    /// regenerated steps land where the originals were.
    #[test]
    fn the_copy_stops_at_the_resume_point() {
        let src = scratch("copysrc");
        let dst = scratch("copydst");
        std::fs::write(
            src.join("scores.csv"),
            "name,a\r\n000000,1\r\n000100,2\r\n000200,3\r\n000290,4\r\n",
        )
        .unwrap();
        plans_file(&src, &[0, 100, 200, 290]);
        copy_results(&src, &dst, 200).expect("copies");

        let csv = std::fs::read_to_string(dst.join("scores.csv")).unwrap();
        assert!(csv.starts_with("name,a"), "the header survives: {csv:?}");
        assert!(csv.contains("000200"), "the boundary row stays");
        assert!(!csv.contains("000290"), "later rows go: {csv:?}");
        assert!(csv.ends_with("\r\n"), "CRLF, so appending does not mix endings");

        let plans = std::fs::read_to_string(dst.join("plans.jsonl")).unwrap();
        assert_eq!(plans.lines().count(), 3);
        assert!(!plans.contains("000290"));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    /// Extending in place would append to the file being read and leave
    /// nothing to fall back on.
    #[test]
    fn extending_into_the_source_is_refused() {
        let dir = scratch("same");
        let e = copy_results(&dir, &dir, 0).unwrap_err().to_string();
        assert!(e.contains("same directory"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An older manifest has no segment count. Assuming one segment is the
    /// safe direction: too few would reuse a random stream, too many only
    /// skips ahead.
    #[test]
    fn a_manifest_without_a_segment_count_assumes_one() {
        let dir = scratch("segs");
        assert_eq!(segments_taken(&dir), 1);
        std::fs::write(dir.join("manifest.json"), "{\"chain\":{}}").unwrap();
        assert_eq!(segments_taken(&dir), 1);
        std::fs::write(dir.join("manifest.json"), "{\"chain\":{\"segments_taken\":7}}").unwrap();
        assert_eq!(segments_taken(&dir), 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Segments of 500 with sampling every 2000 writes a plan at every
    /// boundary -- the best case. An earlier version tested divisibility the
    /// other way round and rejected it as the worst.
    #[test]
    fn a_segment_smaller_than_the_sampling_interval_still_lands_on_boundaries() {
        let dir = scratch("smaller");
        let path = plans_file(&dir, &[0, 2000, 4000, 6000, 8000]);
        // Every one of those is a multiple of 500.
        assert_eq!(resume_point(&path, Some(500)).expect("reads").0, 8000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And the other way round: segments of 2000 with sampling every 500
    /// writes a plan at every boundary too, plus three that are not.
    #[test]
    fn a_segment_larger_than_the_sampling_interval_backs_up_to_one() {
        let dir = scratch("larger");
        let path = plans_file(&dir, &[0, 500, 1000, 1500, 2000, 2500]);
        assert_eq!(resume_point(&path, Some(2000)).expect("reads").0, 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Awkward combinations still resume, just further back: with segments of
    /// 300 and sampling every 2000, only multiples of 6000 are both.
    #[test]
    fn coprime_intervals_resume_at_a_common_multiple() {
        let dir = scratch("coprime");
        let path = plans_file(&dir, &[0, 2000, 4000, 6000, 8000]);
        assert_eq!(resume_point(&path, Some(300)).expect("reads").0, 6000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_without_a_record_of_the_run_is_refused() {
        let dir = scratch("bare");
        let e = settings_path(&dir).unwrap_err().to_string();
        assert!(e.contains("neither settings.json nor manifest.json"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
