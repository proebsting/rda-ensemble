//! Getting data from Dave's Redistricting.
//!
//! DRA publishes one zip per state in the `dra2020/vtd_data` repository,
//! holding a GeoJSON, an adjacency graph, a licence and a README. Which
//! versions exist differs by state -- most are at `v07`, eleven are still at
//! `v06` -- so a tool that made you name a version would be making you guess.
//! [`Inventory`] reads what is actually there.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::Value;

const REPO: &str = "dra2020/vtd_data";
const CYCLE: &str = "2020";

/// The files a state's package unpacks to.
pub struct Package {
    pub geojson: PathBuf,
    /// DRA's own adjacency graph. Always present in their zips, but a
    /// GeoJSON from elsewhere may arrive without one.
    pub graph: Option<PathBuf>,
    pub version: String,
}

/// What DRA publishes, by state.
pub struct Inventory {
    /// Version to compressed size, newest last.
    states: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Inventory {
    /// Read the whole repository tree in one request.
    ///
    /// One call rather than one per state: unauthenticated GitHub allows
    /// sixty an hour, and there are fifty-two states.
    pub fn fetch() -> Result<Inventory> {
        let url = format!("https://api.github.com/repos/{REPO}/git/trees/master?recursive=1");
        let body = ureq::get(&url)
            .set("User-Agent", "rda-ensemble")
            .call()
            .map_err(|e| explain("asking GitHub what DRA publishes", e))?
            .into_string()
            .context("reading the repository listing")?;
        let doc: Value = serde_json::from_str(&body).context("parsing the repository listing")?;

        if doc.get("truncated").and_then(|t| t.as_bool()) == Some(true) {
            bail!("GitHub truncated the repository listing; the inventory would be incomplete");
        }
        let tree = doc
            .get("tree")
            .and_then(|t| t.as_array())
            .ok_or_else(|| anyhow!("the repository listing has no tree"))?;

        let mut states: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        for entry in tree {
            let Some(path) = entry.get("path").and_then(|p| p.as_str()) else { continue };
            let Some((state, version)) = parse_package_path(path) else { continue };
            let size = entry.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            states.entry(state).or_default().insert(version, size);
        }
        if states.is_empty() {
            bail!("found no state packages in {REPO}; has the layout changed?");
        }
        Ok(Inventory { states })
    }

    pub fn states(&self) -> impl Iterator<Item = (&String, &BTreeMap<String, u64>)> {
        self.states.iter()
    }

    /// The newest version of a state's package, and its size.
    pub fn latest(&self, state: &str) -> Result<(String, u64)> {
        let versions = self.states.get(state).ok_or_else(|| {
            anyhow!(
                "DRA publishes nothing for {state}. It has: {}",
                self.states.keys().cloned().collect::<Vec<_>>().join(" ")
            )
        })?;
        // Versions sort as `vNN`, so the last key is the newest.
        let (v, size) = versions.iter().next_back().expect("a state has versions");
        Ok((v.clone(), *size))
    }

    pub fn has(&self, state: &str, version: &str) -> bool {
        self.states.get(state).is_some_and(|v| v.contains_key(version))
    }

    pub fn versions(&self, state: &str) -> Vec<String> {
        self.states.get(state).map(|v| v.keys().cloned().collect()).unwrap_or_default()
    }
}

/// `2020_VTD/IL/Geojson_IL.v07.zip` -> `("IL", "v07")`.
fn parse_package_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix(CYCLE)?.strip_prefix("_VTD/")?;
    let (state, file) = rest.split_once('/')?;
    let version = file
        .strip_prefix(&format!("Geojson_{state}."))?
        .strip_suffix(".zip")?;
    if state.len() != 2 || !version.starts_with('v') {
        return None;
    }
    Some((state.to_string(), version.to_string()))
}

fn package_url(state: &str, version: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{REPO}/master/{CYCLE}_VTD/{state}/Geojson_{state}.{version}.zip"
    )
}

/// Where downloaded packages live by default.
///
/// A per-directory cache would re-download for every new working directory,
/// and the same state is usually wanted for many ensembles, so this is
/// per-user and shared.
pub fn default_cache() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = home.map(|h| h.join("Library/Caches"));
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".cache")));
    base.unwrap_or_else(|| PathBuf::from(".")).join("rda-ensemble")
}

/// Turn a transport failure into something a reader can act on.
///
/// GitHub allows sixty unauthenticated calls an hour, and a bare
/// "status code 403" says neither that it is temporary nor that there is a
/// way around it.
fn explain(what: &str, e: ureq::Error) -> anyhow::Error {
    if let ureq::Error::Status(403 | 429, response) = &e {
        let remaining = response.header("x-ratelimit-remaining");
        if remaining == Some("0") {
            let mins = response
                .header("x-ratelimit-reset")
                .and_then(|r| r.parse::<u64>().ok())
                .and_then(|reset| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_secs();
                    Some(reset.saturating_sub(now).div_ceil(60))
                });
            return anyhow!(
                "GitHub's rate limit for unauthenticated requests is used up{}. \n\
                 It resets on its own; meanwhile `--geojson FILE` skips GitHub \
                 entirely, and an already-downloaded state in the cache still works.",
                match mins {
                    Some(m) => format!(", and resets in about {m} minute(s)"),
                    None => String::new(),
                }
            );
        }
    }
    anyhow!("{what}: {e}")
}
pub fn cache_path(cache: &Path, state: &str, version: &str) -> PathBuf {
    cache.join(format!("{state}_{version}"))
}

/// The package in the cache, if it is already there.
pub fn cached(cache: &Path, state: &str, version: &str) -> Option<Package> {
    let dir = cache_path(cache, state, version);
    let geojson = find(&dir, ".geojson")?;
    Some(Package { graph: find(&dir, "_graph.json"), geojson, version: version.to_string() })
}

/// The newest version of a state already in the cache.
fn newest_cached(cache: &Path, state: &str) -> Option<Package> {
    let prefix = format!("{state}_");
    let mut versions: Vec<String> = std::fs::read_dir(cache)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            name.strip_prefix(&prefix).map(str::to_string)
        })
        .filter(|v| v.starts_with('v'))
        .collect();
    versions.sort();
    cached(cache, state, versions.last()?)
}

fn find(dir: &Path, suffix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        p.file_name()?.to_str()?.ends_with(suffix).then_some(p)
    })
}

/// Download and unpack a state's package, or return the cached copy.
pub fn fetch(cache: &Path, state: &str, version: &str, size: u64) -> Result<Package> {
    if let Some(p) = cached(cache, state, version) {
        eprintln!("  {state} {version} already in {}", cache.display());
        return Ok(p);
    }
    let dir = cache_path(cache, state, version);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;

    let url = package_url(state, version);
    eprintln!("  downloading {state} {version} ({:.1} MB)", size as f64 / 1e6);
    let mut bytes = Vec::with_capacity(size as usize);
    ureq::get(&url)
        .set("User-Agent", "rda-ensemble")
        .call()
        .map_err(|e| explain(&format!("downloading {url}"), e))?
        .into_reader()
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {url}"))?;

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .with_context(|| format!("{state} {version} is not a zip archive"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // Ignore any path the archive carries: everything lands flat in the
        // cache directory, so a crafted archive cannot write outside it.
        let Some(name) = entry.enclosed_name().and_then(|p| {
            p.file_name().map(|f| f.to_string_lossy().into_owned())
        }) else {
            continue;
        };
        if entry.is_dir() || name.is_empty() {
            continue;
        }
        let mut out = std::fs::File::create(dir.join(&name))
            .with_context(|| format!("writing {name}"))?;
        std::io::copy(&mut entry, &mut out).with_context(|| format!("unpacking {name}"))?;
    }

    cached(cache, state, version).ok_or_else(|| {
        anyhow!("{state} {version} unpacked without a .geojson; the package layout may have changed")
    })
}

/// The package for a state: from the cache when it is there, downloaded
/// when it is not.
///
/// Only asks GitHub what exists when it has to, so a cached state keeps
/// working when the rate limit is spent or the network is away.
pub fn resolve(cache: &Path, state: &str, version: Option<&str>) -> Result<Package> {
    if let Some(v) = version {
        if let Some(p) = cached(cache, state, v) {
            eprintln!("  {state} {v} already in {}", cache.display());
            return Ok(p);
        }
    }
    let inventory = match Inventory::fetch() {
        Ok(i) => i,
        // Asking which version is newest needs the network; using one that
        // is already here does not. Building several ensembles for the same
        // state should not depend on GitHub being reachable every time.
        Err(e) => match newest_cached(cache, state) {
            Some(p) => {
                eprintln!("  could not reach DRA's index, using cached {state} {}", p.version);
                return Ok(p);
            }
            None => return Err(e),
        },
    };
    let (version, size) = match version {
        Some(v) if inventory.has(state, v) => (v.to_string(), 0),
        Some(v) => bail!(
            "DRA has no {v} for {state}; it has {}",
            inventory.versions(state).join(", ")
        ),
        None => inventory.latest(state)?,
    };
    fetch(cache, state, &version, size)
}

/// Print what DRA publishes.
pub fn list(inventory: &Inventory, only: Option<&str>) -> Result<()> {
    if let Some(state) = only {
        if inventory.versions(state).is_empty() {
            bail!(
                "DRA publishes nothing for {state}. It has: {}",
                inventory.states().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(" ")
            );
        }
    }
    println!("Dave's Redistricting, {CYCLE} VTD packages\n");
    println!("  {:<7}{:<9}{:>8}   older", "state", "latest", "size");
    for (state, versions) in inventory.states() {
        if only.is_some_and(|s| s != state) {
            continue;
        }
        let (latest, size) = versions.iter().next_back().expect("a state has versions");
        let older: Vec<&str> =
            versions.keys().rev().skip(1).map(String::as_str).collect();
        let older = if older.is_empty() { "-".to_string() } else { older.join(", ") };
        println!("  {state:<7}{latest:<9}{:>7.1}M   {older}", *size as f64 / 1e6);
    }
    println!("\nEach package holds a GeoJSON and DRA's adjacency graph.");
    println!("`rda-ensemble fetch --state XX` downloads the latest; `run --state XX` does it for you.");
    Ok(())
}
