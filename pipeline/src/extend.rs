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
//! # The caveat
//!
//! The chain resumes from the last plan that was *written*, which at
//! `--sample-every 200` is up to 199 steps behind where the earlier run
//! actually stopped. Those steps happened and were counted, but the plans
//! were not kept, so they cannot be carried on from. The extension says
//! which step it is resuming from rather than leaving that to be inferred.

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
}

pub fn extend(
    from: &Path,
    out: &Path,
    plans: Option<u64>,
    steps: Option<u64>,
    segment_steps: Option<u64>,
    cache: Option<PathBuf>,
    ev: &Sink,
) -> Result<()> {
    let earlier = read_earlier(from)?;

    let more = match (steps, plans) {
        (Some(s), _) => s,
        (None, Some(p)) => p
            .checked_mul(earlier.settings.sample_every)
            .ok_or_else(|| anyhow!("--plans {p} overflows at this sampling rate"))?,
        (None, None) => bail!("give --plans or --steps: how much longer to run"),
    };

    ev.status(&format!(
        "extending {} by {more} steps, sampling every {}",
        from.display(),
        earlier.settings.sample_every
    ));

    copy_results(from, out)?;

    let mut args = earlier
        .settings
        .clone()
        .into_args(out.to_path_buf(), cache)?;
    // What to run now, as opposed to what the earlier run ran.
    args.steps = Some(more);
    args.plans = None;
    args.segment_steps = segment_steps;
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
    run::run_extended(&args, &keep, &Cancel::default(), ev, Some(&extension))
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
    if !plans.exists() {
        bail!(
            "{} has no plans.jsonl, so there is no plan to carry on from. \
             An ensemble has to be made with `--keep plans` to be extendable.",
            dir.display()
        );
    }
    let (name, plan) = last_plan(&plans)?;
    let from_step: u64 = name
        .parse()
        .with_context(|| format!("the last plan is named {name:?}, which is not a step number"))?;

    Ok(Earlier {
        settings,
        plan,
        from_step,
        segment_base: segments_taken(dir),
    })
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

/// The last record of a plans file: its name, and geoid to district.
fn last_plan(path: &Path) -> Result<(String, HashMap<String, u32>)> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut last = None;
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            last = Some(line);
        }
    }
    let line = last.ok_or_else(|| anyhow!("{} holds no plans", path.display()))?;
    let doc: Value = serde_json::from_str(&line)
        .with_context(|| format!("the last record of {} is not JSON", path.display()))?;
    let name = doc
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow!("the last plan has no name"))?
        .to_string();
    let plan = doc
        .get("plan")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow!("the last record is not a plan"))?;
    let mut out = HashMap::with_capacity(plan.len());
    for (geoid, district) in plan {
        let d = district
            .as_u64()
            .ok_or_else(|| anyhow!("district for {geoid} is not a number"))?;
        out.insert(geoid.clone(), d as u32);
    }
    Ok((name, out))
}

/// Copy the earlier results into the new directory, so what is appended to
/// is a copy and the original is left exactly as it was.
fn copy_results(from: &Path, out: &Path) -> Result<()> {
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
        if src.exists() {
            std::fs::copy(&src, out.join(name))
                .with_context(|| format!("copying {} to {}", src.display(), out.display()))?;
        }
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

    /// The chain resumes from the *last* plan, which is what decides where
    /// the extension picks up.
    #[test]
    fn the_last_plan_is_the_one_carried_over() {
        let dir = scratch("last");
        let path = dir.join("plans.jsonl");
        std::fs::write(
            &path,
            "{\"_tag_\":\"plan\",\"name\":\"000000\",\"plan\":{\"a\":1,\"b\":1}}\n\
             {\"_tag_\":\"plan\",\"name\":\"000290\",\"plan\":{\"a\":2,\"b\":1}}\n\n",
        )
        .unwrap();
        let (name, plan) = last_plan(&path).expect("reads");
        assert_eq!(name, "000290");
        assert_eq!(plan.get("a"), Some(&2));
        assert_eq!(plan.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_plans_file_is_refused() {
        let dir = scratch("empty");
        let path = dir.join("plans.jsonl");
        std::fs::write(&path, "\n\n").unwrap();
        assert!(last_plan(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Extending in place would append to the file being read and leave
    /// nothing to fall back on.
    #[test]
    fn extending_into_the_source_is_refused() {
        let dir = scratch("same");
        let e = copy_results(&dir, &dir).unwrap_err().to_string();
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

    #[test]
    fn a_directory_without_a_record_of_the_run_is_refused() {
        let dir = scratch("bare");
        let e = settings_path(&dir).unwrap_err().to_string();
        assert!(e.contains("neither settings.json nor manifest.json"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
