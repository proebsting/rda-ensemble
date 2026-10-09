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

/// How long a cached inventory is trusted.
///
/// DRA publishes a new version a few times a year, so a day is far fresher
/// than the data changes, and it means a session of many runs costs one
/// request rather than one per run.
const INVENTORY_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

impl Inventory {
    /// The inventory, from the cache when it is recent enough.
    ///
    /// Without this every run spends one of GitHub's sixty unauthenticated
    /// requests an hour, so building a few dozen ensembles in an afternoon
    /// would start failing.
    pub fn load(cache: &Path) -> Result<Inventory> {
        let path = cache.join("inventory.json");
        if let Ok(meta) = std::fs::metadata(&path) {
            let fresh = meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|age| age < INVENTORY_TTL);
            if fresh {
                if let Ok(body) = std::fs::read_to_string(&path) {
                    if let Ok(inv) = Self::parse(&body) {
                        return Ok(inv);
                    }
                }
            }
        }
        let body = Self::download()?;
        // Best effort: a cache that cannot be written is not a failure.
        let _ = std::fs::create_dir_all(cache);
        let _ = std::fs::write(&path, &body);
        Self::parse(&body)
    }

    /// Read the whole repository tree in one request.
    ///
    /// One call rather than one per state: unauthenticated GitHub allows
    /// sixty an hour, and there are fifty-two states.
    fn download() -> Result<String> {
        let url = format!("https://api.github.com/repos/{REPO}/git/trees/master?recursive=1");
        ureq::get(&url)
            .set("User-Agent", "rda-ensemble")
            .call()
            .map_err(|e| explain("asking GitHub what DRA publishes", e))?
            .into_string()
            .context("reading the repository listing")
    }

    fn parse(body: &str) -> Result<Inventory> {
        let doc: Value = serde_json::from_str(body).context("parsing the repository listing")?;

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

    /// The compressed size of one published version, in bytes.
    pub fn size(&self, state: &str, version: &str) -> Option<u64> {
        self.states.get(state)?.get(version).copied()
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
pub fn fetch(
    cache: &Path,
    state: &str,
    version: &str,
    size: u64,
    ev: &crate::events::Sink,
) -> Result<Package> {
    if let Some(p) = cached(cache, state, version) {
        ev.status(&format!("  {state} {version} already in {}", cache.display()));
        return Ok(p);
    }
    let dir = cache_path(cache, state, version);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;

    let url = package_url(state, version);
    ev.status(&format!("  downloading {state} {version} ({:.1} MB)", size as f64 / 1e6));
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
pub fn resolve(
    cache: &Path,
    state: &str,
    version: Option<&str>,
    ev: &crate::events::Sink,
) -> Result<Package> {
    if let Some(v) = version {
        if let Some(p) = cached(cache, state, v) {
            ev.status(&format!("  {state} {v} already in {}", cache.display()));
            return Ok(p);
        }
    }
    let inventory = match Inventory::load(cache) {
        Ok(i) => i,
        // Asking which version is newest needs the network; using one that
        // is already here does not. Building several ensembles for the same
        // state should not depend on GitHub being reachable every time.
        Err(e) => match newest_cached(cache, state) {
            Some(p) => {
                ev.status(&format!(
                    "  could not reach DRA's index, using cached {state} {}",
                    p.version
                ));
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
    fetch(cache, state, &version, size, ev)
}

/// What DRA publishes for one state.
#[derive(serde::Serialize)]
pub struct Listing {
    pub state: String,
    pub latest: String,
    /// Bytes, as DRA's index reports them.
    pub size: u64,
    /// Every other version, newest first.
    pub older: Vec<String>,
}

/// What DRA publishes, one row per state, newest version first.
///
/// Returns the rows rather than printing them: this is what fills a state
/// picker, whether that picker is [`render`] or something with a scrollbar.
pub fn listings(inventory: &Inventory, only: Option<&str>) -> Result<Vec<Listing>> {
    if let Some(state) = only {
        if inventory.versions(state).is_empty() {
            bail!(
                "DRA publishes nothing for {state}. It has: {}",
                inventory.states().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(" ")
            );
        }
    }
    Ok(inventory
        .states()
        .filter(|(state, _)| !only.is_some_and(|s| s != state.as_str()))
        .map(|(state, versions)| {
            let (latest, size) = versions.iter().next_back().expect("a state has versions");
            Listing {
                state: state.clone(),
                latest: latest.clone(),
                size: *size,
                older: versions.keys().rev().skip(1).cloned().collect(),
            }
        })
        .collect())
}

/// The listing as lines of text, for a terminal.
pub fn render(rows: &[Listing]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Dave's Redistricting, {CYCLE} VTD packages\n");
    let _ = writeln!(out, "  {:<7}{:<9}{:>8}   older", "state", "latest", "size");
    for r in rows {
        let older = if r.older.is_empty() { "-".to_string() } else { r.older.join(", ") };
        let _ = writeln!(
            out,
            "  {:<7}{:<9}{:>7.1}M   {older}",
            r.state,
            r.latest,
            r.size as f64 / 1e6
        );
    }
    let _ = writeln!(out, "\nEach package holds a GeoJSON and DRA's adjacency graph.");
    let _ = writeln!(
        out,
        "`rda-ensemble fetch --state XX` downloads the latest; `run --state XX` does it for you."
    );
    out
}


// ---------------------------------------------------------------------------
// What is on disk.
//
// A user who has fetched ten states has a few hundred megabytes in a
// directory they have never seen. These make it visible and removable
// without anyone having to know the layout.
// ---------------------------------------------------------------------------

/// One package sitting in the cache.
#[derive(serde::Serialize)]
pub struct Cached {
    pub state: String,
    pub version: String,
    /// Bytes on disk, the files as unpacked.
    pub bytes: u64,
    pub path: PathBuf,
    /// Whether DRA's adjacency graph came with it.
    pub has_graph: bool,
}

/// Every package in the cache, by state and then version.
///
/// A directory that does not parse as `XX_vNN`, or holds no GeoJSON, is not
/// ours and is left alone: people put things in cache directories.
pub fn contents(cache: &Path) -> Vec<Cached> {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    let mut out: Vec<Cached> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let (state, version) = name.split_once('_')?;
            let package = cached(cache, state, version)?;
            Some(Cached {
                state: state.to_string(),
                version: version.to_string(),
                bytes: directory_size(&e.path()),
                path: e.path(),
                has_graph: package.graph.is_some(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.state.cmp(&b.state).then(a.version.cmp(&b.version)));
    out
}

/// Bytes under a directory. Shallow is enough: DRA packages are flat.
fn directory_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .map(|m| if m.is_dir() { 0 } else { m.len() })
        .sum()
}

/// The cached index of what DRA publishes, if it has been fetched.
pub fn index_file(cache: &Path) -> Option<(PathBuf, u64)> {
    let path = cache.join("inventory.json");
    let bytes = std::fs::metadata(&path).ok()?.len();
    Some((path, bytes))
}

/// Delete cached packages, and say which went.
///
/// `state` and `version` narrow what is removed; both absent means all of
/// them. The index is left alone unless `index` asks for it, since it is
/// small and re-fetching it costs a round trip to GitHub.
pub fn forget(
    cache: &Path,
    state: Option<&str>,
    version: Option<&str>,
    index: bool,
) -> Result<Vec<Cached>> {
    let doomed: Vec<Cached> = contents(cache)
        .into_iter()
        .filter(|c| state.map_or(true, |s| s.eq_ignore_ascii_case(&c.state)))
        .filter(|c| version.map_or(true, |v| v == c.version))
        .collect();
    for c in &doomed {
        std::fs::remove_dir_all(&c.path)
            .with_context(|| format!("removing {}", c.path.display()))?;
    }
    if index {
        let path = cache.join("inventory.json");
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(doomed)
}

/// The cache as lines of text, for a terminal.
pub fn render_cache(cache: &Path, rows: &[Cached]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "{}\n", cache.display());
    if rows.is_empty() {
        let _ = writeln!(out, "  empty");
    } else {
        let _ = writeln!(out, "  {:<7}{:<9}{:>9}   adjacency", "state", "version", "size");
        for c in rows {
            let _ = writeln!(
                out,
                "  {:<7}{:<9}{:>8.1}M   {}",
                c.state,
                c.version,
                c.bytes as f64 / 1e6,
                if c.has_graph { "DRA graph" } else { "shapes only" },
            );
        }
        let total: u64 = rows.iter().map(|c| c.bytes).sum();
        let _ = writeln!(
            out,
            "\n  {} package(s), {:.1} MB",
            rows.len(),
            total as f64 / 1e6
        );
    }
    if let Some((_, bytes)) = index_file(cache) {
        let _ = writeln!(out, "  index    {:.0} kB, re-fetched after a day", bytes as f64 / 1e3);
    }
    let _ = writeln!(
        out,
        "\n`rda-ensemble cache --forget XX` removes a state; --forget-all removes every package."
    );
    out
}
