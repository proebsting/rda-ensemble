//! Has the chain run long enough?
//!
//! Two different questions, and the first is the one that misleads.
//!
//! **Are consecutive samples independent?** That is the effective sample
//! size, from the integrated autocorrelation time. A chain stuck in one
//! corner of the space has perfectly uncorrelated samples *from that corner*,
//! so a good ESS on its own says nothing about whether the ensemble is right.
//!
//! **Has the chain seen the whole distribution?** That is Gelman-Rubin
//! R-hat, and it cannot be answered from one chain. Several chains started
//! from different plans either agree about the distribution or they do not.
//!
//! Neither is computed on plans, which are not numbers. Both are computed on
//! scalar summaries of each plan -- exactly the columns of `scores.csv`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// Integrated autocorrelation time, by Geyer's initial positive sequence.
///
/// Adjacent autocorrelation pairs are summed while they stay positive. The
/// naive alternative -- summing to some fixed lag -- accumulates noise from
/// high-lag estimates, which are made from few terms and are mostly noise.
pub fn autocorrelation_time(x: &[f64]) -> Option<f64> {
    let n = x.len();
    if n < 8 {
        return None;
    }
    let mean = x.iter().sum::<f64>() / n as f64;
    let d: Vec<f64> = x.iter().map(|v| v - mean).collect();
    let var = d.iter().map(|v| v * v).sum::<f64>() / n as f64;
    // A score that never moved -- the statewide vote share is the same
    // whatever the districts are -- has nothing to diagnose. Testing against
    // zero is not enough: summing five hundred copies of one number leaves a
    // variance around 1e-34 rather than exactly 0, and the autocorrelation
    // of that rounding noise is meaningless but large.
    if var.sqrt() <= 1e-12 * mean.abs().max(1.0) {
        return None;
    }
    let rho = |k: usize| -> f64 {
        let s: f64 = (0..n - k).map(|i| d[i] * d[i + k]).sum();
        s / ((n - k) as f64 * var)
    };

    let mut total = 0.0;
    let mut k = 1;
    while k + 1 < n / 2 {
        let pair = rho(k) + rho(k + 1);
        if pair <= 0.0 {
            break;
        }
        total += pair;
        k += 2;
    }
    Some(1.0 + 2.0 * total)
}

/// Draws that are worth as much as this many independent ones.
pub fn effective_sample_size(x: &[f64]) -> Option<f64> {
    autocorrelation_time(x).map(|tau| x.len() as f64 / tau.max(1.0))
}

/// Gelman-Rubin R-hat over chains that started from different plans.
///
/// Compares the spread *between* chain means against the spread *within*
/// each chain. Chains that have reached the same distribution have a small
/// between-chain spread, so the ratio tends to one.
pub fn r_hat(chains: &[Vec<f64>]) -> Option<f64> {
    let m = chains.len();
    if m < 2 {
        return None;
    }
    let n = chains.iter().map(Vec::len).min()?;
    if n < 2 {
        return None;
    }
    let means: Vec<f64> =
        chains.iter().map(|c| c[..n].iter().sum::<f64>() / n as f64).collect();
    let grand = means.iter().sum::<f64>() / m as f64;

    let b = n as f64 / (m - 1) as f64
        * means.iter().map(|mu| (mu - grand).powi(2)).sum::<f64>();
    let w = chains
        .iter()
        .zip(&means)
        .map(|(c, mu)| c[..n].iter().map(|v| (v - mu).powi(2)).sum::<f64>() / (n - 1) as f64)
        .sum::<f64>()
        / m as f64;
    if w <= 0.0 {
        return None;
    }
    let var = (n - 1) as f64 / n as f64 * w + b / n as f64;
    Some((var / w).sqrt())
}

/// R-hat above this is conventionally taken as not yet converged.
pub const R_HAT_LIMIT: f64 = 1.01;

/// One score's diagnosis.
pub struct Column {
    pub name: String,
    pub r_hat: Option<f64>,
    pub ess: Option<f64>,
    pub tau: Option<f64>,
}

/// What every score column says about the run.
pub struct Report {
    pub columns: Vec<Column>,
    pub chains: usize,
    pub per_chain: usize,
}

impl Report {
    /// Diagnose each column across the chains' score series.
    pub fn build(series: &[BTreeMap<String, Vec<f64>>]) -> Report {
        let per_chain = series.iter().map(|s| {
            s.values().map(Vec::len).max().unwrap_or(0)
        }).min().unwrap_or(0);

        let mut names: Vec<&String> = series.first().map(|s| s.keys().collect()).unwrap_or_default();
        names.sort();

        let columns = names
            .into_iter()
            .map(|name| {
                let per: Vec<Vec<f64>> = series
                    .iter()
                    .filter_map(|s| s.get(name).cloned())
                    .collect();
                // ESS from the first chain: autocorrelation is a property of
                // one chain's path, not of the set.
                let first = per.first().map(Vec::as_slice).unwrap_or(&[]);
                Column {
                    name: name.clone(),
                    r_hat: r_hat(&per),
                    ess: effective_sample_size(first),
                    tau: autocorrelation_time(first),
                }
            })
            .collect();
        Report { columns, chains: series.len(), per_chain }
    }

    /// The column that converged least, which is the one to judge by.
    pub fn worst_r_hat(&self) -> Option<&Column> {
        self.columns
            .iter()
            .filter(|c| c.r_hat.is_some())
            .max_by(|a, b| a.r_hat.unwrap().total_cmp(&b.r_hat.unwrap()))
    }

    /// The column with the fewest independent draws behind it.
    pub fn lowest_ess(&self) -> Option<&Column> {
        self.columns
            .iter()
            .filter(|c| c.ess.is_some())
            .min_by(|a, b| a.ess.unwrap().total_cmp(&b.ess.unwrap()))
    }

    pub fn to_value(&self) -> Value {
        let mut columns = Map::new();
        for c in &self.columns {
            columns.insert(
                c.name.clone(),
                json!({
                    "r_hat": c.r_hat,
                    "ess": c.ess,
                    "autocorrelation_time": c.tau,
                }),
            );
        }
        json!({
            "chains": self.chains,
            "plans_per_chain": self.per_chain,
            "r_hat_limit": R_HAT_LIMIT,
            "worst_r_hat": self.worst_r_hat().map(|c| json!({"score": c.name, "r_hat": c.r_hat})),
            "lowest_ess": self.lowest_ess().map(|c| json!({"score": c.name, "ess": c.ess})),
            "columns": Value::Object(columns),
        })
    }

    /// What to print when the run ends.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "\nconvergence");
        match self.worst_r_hat() {
            Some(c) => {
                let r = c.r_hat.expect("filtered");
                let _ = writeln!(
                    out,
                    "  R-hat        {r:.3} at worst, on {}   ({} chains of {})",
                    c.name, self.chains, self.per_chain
                );
                if r > R_HAT_LIMIT {
                    let _ = writeln!(
                        out,
                        "               above {R_HAT_LIMIT}: the chains do not yet agree, so run longer"
                    );
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "  R-hat        needs more than one chain; --chains 4 is the usual choice"
                );
            }
        }
        if let Some(c) = self.lowest_ess() {
            let ess = c.ess.expect("filtered");
            let _ = writeln!(
                out,
                "  effective N  {ess:.0} at worst, on {}   (of {} sampled)",
                c.name, self.per_chain
            );
            if ess < self.per_chain as f64 * 0.5 {
                let _ = writeln!(
                    out,
                    "               consecutive plans are correlated; a larger --sample-every helps"
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent draws have no autocorrelation, so tau is about 1 and the
    /// effective sample size is about the real one.
    #[test]
    fn independent_draws_have_tau_near_one() {
        // A deterministic pseudo-random sequence: no crate needed, and the
        // test does not depend on a generator's exact stream.
        let mut s: u64 = 0x2545F491_4F6CDD1D;
        let x: Vec<f64> = (0..4000)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f64 / (1u64 << 53) as f64
            })
            .collect();
        let tau = autocorrelation_time(&x).expect("varies");
        assert!(tau < 1.3, "tau should be near 1 for independent draws, got {tau}");
        let ess = effective_sample_size(&x).expect("varies");
        assert!(ess > 3000.0, "ESS should be near N, got {ess}");
    }

    /// A slow random walk is heavily autocorrelated, so a long series is
    /// worth far fewer independent draws than its length.
    #[test]
    fn a_random_walk_is_worth_far_less_than_its_length() {
        let mut s: u64 = 12345;
        let mut v = 0.0;
        let x: Vec<f64> = (0..4000)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                v += ((s >> 11) as f64 / (1u64 << 53) as f64) - 0.5;
                v
            })
            .collect();
        let tau = autocorrelation_time(&x).expect("varies");
        assert!(tau > 20.0, "a random walk should be strongly correlated, got tau {tau}");
        assert!(effective_sample_size(&x).expect("varies") < 400.0);
    }

    #[test]
    fn r_hat_sees_chains_that_disagree() {
        // Four chains around the same mean: converged.
        let agree: Vec<Vec<f64>> = (0..4)
            .map(|j| (0..200).map(|i| ((i * 7 + j) % 11) as f64).collect())
            .collect();
        let r = r_hat(&agree).expect("varies");
        assert!(r < 1.05, "chains from one distribution should agree, got {r}");

        // The same chains, each shifted somewhere else: not converged.
        let apart: Vec<Vec<f64>> = agree
            .iter()
            .enumerate()
            .map(|(j, c)| c.iter().map(|v| v + j as f64 * 30.0).collect())
            .collect();
        let r = r_hat(&apart).expect("varies");
        assert!(r > 1.5, "chains in different places should not agree, got {r}");
    }

    #[test]
    fn one_chain_cannot_answer_r_hat() {
        assert!(r_hat(&[vec![1.0, 2.0, 3.0]]).is_none());
    }

    #[test]
    fn a_constant_score_is_not_diagnosed() {
        assert!(autocorrelation_time(&[4.0; 100]).is_none());
        assert!(r_hat(&[vec![4.0; 100], vec![4.0; 100]]).is_none());
    }

    /// The statewide vote share does not depend on the districts, so it is
    /// the same in every plan. Its variance is not exactly zero -- summing
    /// hundreds of copies of one number does not cancel exactly -- and the
    /// autocorrelation of what is left is rounding noise. Diagnosing it
    /// reported an effective sample size of 1 out of 500.
    #[test]
    fn a_score_constant_to_within_rounding_is_not_diagnosed() {
        let x = vec![0.5188_f64; 500];
        let mean = x.iter().sum::<f64>() / 500.0;
        assert!(
            x.iter().any(|v| *v != mean),
            "this test is pointless unless the mean really is off by an ulp"
        );
        assert!(autocorrelation_time(&x).is_none());
        assert!(effective_sample_size(&x).is_none());
    }
}
