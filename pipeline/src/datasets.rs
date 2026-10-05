//! Listing what a GeoJSON carries.
//!
//! The four dataset options are the ones a newcomer cannot guess, because the
//! names live inside the file. DRA labels every dataset with a type, a title
//! and usually a description, and composite elections name their members, so
//! the file can answer the question itself.

use std::path::Path;

use anyhow::{bail, Context as _, Result};
use serde_json::Value;

/// Which option a dataset belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Census,
    Vap,
    Cvap,
    Election,
}

impl Kind {
    /// The heading this kind sits under, and the option it fills.
    pub fn heading(self) -> (&'static str, &'static str) {
        match self {
            Kind::Census => ("Total population", "--census"),
            Kind::Vap => ("Voting-age population", "--vap"),
            Kind::Cvap => ("Citizen voting-age population", "--cvap"),
            Kind::Election => ("Elections", "--elections"),
        }
    }
}

/// Which option a dataset belongs to, from DRA's own labelling.
///
/// `votingAge` separates census from VAP. DRA carries no flag for citizenship,
/// so the name is the signal -- `V_20_CVAP` against `V_20_VAP` -- which is
/// also how the titles read ("Citizen VAP 2020").
pub fn classify(name: &str, entry: &Value) -> Option<Kind> {
    match entry.get("type").and_then(|t| t.as_str())? {
        "election" => Some(Kind::Election),
        "demographic" => {
            let voting_age =
                entry.get("votingAge").and_then(|v| v.as_bool()).unwrap_or(false);
            if !voting_age {
                Some(Kind::Census)
            } else if name.contains("CVAP") {
                Some(Kind::Cvap)
            } else {
                Some(Kind::Vap)
            }
        }
        _ => None,
    }
}

/// One dataset a GeoJSON carries.
///
/// What a caller needs to offer it as a choice: the name to pass back, what
/// the name is for, and -- for a composite election -- what it averages.
#[derive(serde::Serialize)]
pub struct Dataset {
    pub kind: Kind,
    pub name: String,
    pub title: String,
    /// The elections a composite averages, empty for everything else. Which
    /// ones is the thing that decides whether a composite is the right pick.
    pub members: Vec<String>,
}

/// Every dataset in a GeoJSON, grouped by the option it belongs to and
/// newest first within a group.
///
/// Returns them rather than printing: this is what fills a list of choices,
/// whether that is [`render`] writing lines or a caller building a menu.
pub fn read(path: &Path) -> Result<Vec<Dataset>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let doc: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    let Some(datasets) = doc.get("datasets").and_then(|d| d.as_object()) else {
        bail!("{} has no datasets object; is this a DRA export?", path.display());
    };

    let mut rows: Vec<(Kind, &str, &Value)> = datasets
        .iter()
        .filter_map(|(name, entry)| {
            classify(name, entry).map(|kind| (kind, name.as_str(), entry))
        })
        .collect();
    // Within a kind, newest first: that is almost always what is wanted.
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(year(b.2).cmp(&year(a.2)))
            .then(a.1.cmp(b.1))
    });

    Ok(rows
        .into_iter()
        .map(|(kind, name, entry)| Dataset {
            kind,
            name: name.to_string(),
            title: entry
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            members: members_of(entry),
        })
        .collect())
}

/// A composite election's members, in the order DRA numbers them.
fn members_of(entry: &Value) -> Vec<String> {
    let Some(members) = entry.get("members").and_then(|m| m.as_object()) else {
        return Vec::new();
    };
    let mut keys: Vec<&str> = members.keys().map(|k| k.as_str()).collect();
    keys.sort_by_key(|k| k.parse::<u32>().unwrap_or(u32::MAX));
    keys.iter()
        .filter_map(|k| members[*k].as_str().map(str::to_string))
        .collect()
}

/// The dataset list as lines of text, for a terminal.
pub fn render(path: &Path, rows: &[Dataset]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "{}\n", path.display());
    let width = rows.iter().map(|d| d.name.len()).max().unwrap_or(12).max(12);

    let mut current: Option<Kind> = None;
    for d in rows {
        if current != Some(d.kind) {
            if current.is_some() {
                let _ = writeln!(out);
            }
            let (heading, flag) = d.kind.heading();
            let _ = writeln!(out, "{heading}  ({flag})");
            current = Some(d.kind);
        }
        let _ = writeln!(out, "  {:<width$}  {}", d.name, d.title);
        if !d.members.is_empty() {
            let _ = writeln!(out, "  {:<width$}  averages {}", "", d.members.join(", "));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Pass any of these names to the matching option of `rda-ensemble run`,"
    );
    let _ = writeln!(out, "or `--elections all` to score every election at once.");
    out
}

fn year(entry: &Value) -> i64 {
    entry.get("year").and_then(|y| y.as_i64()).unwrap_or(0)
}

/// The three demographic datasets for a census cycle.
pub struct Cycle {
    pub census: String,
    pub vap: String,
    pub cvap: String,
}

/// Pick the demographic datasets a cycle wants, from the file's own labels.
///
/// DRA tags every dataset with the year it describes, so nothing here has to
/// know that the 2020 census is called `T_20_CENS`: it is the demographic
/// dataset for 2020 that is not voting-age. That keeps the choice driven by
/// the data rather than by a naming convention which a future export could
/// perfectly well change.
pub fn for_cycle(
    doc: &Value,
    year: i64,
    given: [Option<&str>; 3],
) -> Result<Cycle> {
    let Some(datasets) = doc.get("datasets").and_then(|d| d.as_object()) else {
        bail!("the GeoJSON has no datasets object; is this a DRA export?");
    };

    // Several datasets can describe the same year: 2020 has both the
    // decennial count and an ACS estimate of total population, and both a
    // plain VAP and a non-Hispanic-alone breakdown. DRA marks the variants
    // -- `nhAlone` on one, an ACS `description` on the other -- and leaves
    // the headline dataset unqualified, so prefer the unqualified one.
    let plain = |entry: &Value| {
        entry.get("nhAlone").is_none() && entry.get("description").is_none()
    };

    let candidates = |want: Kind| -> Vec<String> {
        let all: Vec<(&String, &Value)> = datasets
            .iter()
            .filter(|(name, entry)| {
                classify(name, entry) == Some(want) && self::year(entry) == year
            })
            .collect();
        // CVAP only ever comes from the ACS, so narrowing would empty the
        // set; keep everything when that happens.
        let narrowed: Vec<_> = all.iter().filter(|(_, e)| plain(e)).collect();
        if narrowed.is_empty() {
            all.iter().map(|(n, _)| (*n).clone()).collect()
        } else {
            narrowed.iter().map(|(n, _)| (*n).clone()).collect()
        }
    };

    let pick = |want: Kind, flag: &str| -> Result<String> {
        let mut found = candidates(want);
        match found.len() {
            1 => Ok(found.remove(0)),
            // Never guess between equals: the choice changes every score.
            _ => {
                found.sort();
                bail!(
                    "{} dataset for {year} in this GeoJSON: {}.\n\
                     Name the one you want with {flag}.",
                    if found.is_empty() { "no".to_string() } else { format!("{} candidates for the", found.len()) },
                    if found.is_empty() { "none".to_string() } else { found.join(", ") }
                )
            }
        }
    };

    // Only work out what the command line did not already say. Naming
    // --census explicitly should settle the question for census, even where
    // the year has two candidates and the cycle alone could not choose.
    let [want_census, want_vap, want_cvap] = given;
    let mut missing = Vec::new();
    for (kind, flag, given) in [
        (Kind::Census, "--census", want_census),
        (Kind::Vap, "--vap", want_vap),
        (Kind::Cvap, "--cvap", want_cvap),
    ] {
        if given.is_none() && candidates(kind).is_empty() {
            missing.push(flag);
        }
    }
    if !missing.is_empty() {
        let mut years: Vec<i64> = datasets
            .iter()
            .filter(|(n, e)| classify(n, e).is_some_and(|k| k != Kind::Election))
            .map(|(_, e)| self::year(e))
            .collect();
        years.sort_unstable();
        years.dedup();
        bail!(
            "no {} dataset for {year} in this GeoJSON.\n\
             Demographic years it carries: {}\n\
             Pick a different --cycle, or name the datasets with {} directly.",
            missing.join(" or "),
            years.iter().map(|y| y.to_string()).collect::<Vec<_>>().join(", "),
            missing.join(", ")
        );
    }

    let resolve = |kind: Kind, flag: &str, given: Option<&str>| -> Result<String> {
        match given {
            Some(name) => Ok(name.to_string()),
            None => pick(kind, flag),
        }
    };
    Ok(Cycle {
        census: resolve(Kind::Census, "--census", want_census)?,
        vap: resolve(Kind::Vap, "--vap", want_vap)?,
        cvap: resolve(Kind::Cvap, "--cvap", want_cvap)?,
    })
}
