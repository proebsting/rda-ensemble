# rda-ensemble

Generate a redistricting ensemble from a Dave's Redistricting GeoJSON and
score every plan, in one command.

It reads the single file DRA publishes for a state and does everything else
itself: builds the precinct adjacency graph, draws a population-balanced
starting plan, runs a ReCom Markov chain, and scores plans as the chain
produces them. Nothing is written between stages unless you ask for it.

```
NC.geojson  ──►  precinct graph  ──►  starting plan  ──►  chain  ──►  scores.csv
```

One binary. No Python, no shell script, no intermediate files.

## Quick start

Nothing but the binary. It fetches the data itself.

```sh
rda-ensemble run \
  --state NC --plan-type congress --cycle 2020 \
  --elections E_16-20_COMP \
  --seed-tolerance 0.01 \
  --steps 10000 --variant cut-edges-ust --tolerance 0.05 --rng-seed 1 \
  --out results
```

```
no --geojson; asking DRA what it has for NC
  downloading NC v07 (4.6 MB)
  ...
  adjacency from dra-data/NC_v07/NC_2020_graph.json, matching the shapes exactly
  2666 precincts, 1 election(s)
```

The package lands in `dra-data/` and is reused, so a second run does not
download again.

## Getting the data

`rda-ensemble states` lists what DRA publishes — 52 states including DC and
Puerto Rico, with the newest version of each and its size:

```
  state  latest       size   older
  AK     v07          2.6M   v06
  AR     v06          2.9M   -
  IL     v07          7.7M   v06
  NC     v07          4.6M   v06
```

Versions are not uniform: most states are at `v07`, eleven are still at `v06`.
Nothing makes you guess — leave `--dra-version` out and the newest is used.
`rda-ensemble fetch --state IL` downloads without running anything.

## What is in the package

Each DRA zip holds a GeoJSON and an adjacency graph. `--adjacency` says which
the run uses:

| | |
| --- | --- |
| `auto` (default) | DRA's graph when there is one, the shapes otherwise — and it compares the two, reporting any disagreement |
| `dra` | DRA's graph, failing if absent |
| `geometry` | derived from the precinct shapes, ignoring any published graph |

The two have agreed exactly on every state checked — NC, IL and MI, node for
node, including the `OUT_OF_STATE` border node — so `auto` is a guard against
a future divergence rather than a live concern. Deriving from the shapes is
what lets the tool work on a GeoJSON that arrives without a graph.

To use a file you already have, pass `--geojson`; a `*_graph.json` beside it is
picked up automatically.

```sh
rda-ensemble datasets NC.geojson
```

```
Total population  (--census)
  T_20_CENS     Total Population 2020
  T_10_CENS     Total Population 2010
  ...

Elections  (--elections)
  E_16-20_COMP  Composite 2016-2020
                averages E_20_PRES, E_20_GOV, E_20_SEN, E_16_PRES, E_20_AG, E_16_SEN
  E_20_PRES     President 2020
  ...
```

Then pass the names you want:

```sh
rda-ensemble run \
  --geojson NC.geojson \
  --state NC --plan-type congress --cycle 2020 \
  --elections E_16-20_COMP \
  --seed-tolerance 0.01 \
  --steps 10000 --variant cut-edges-ust --tolerance 0.05 --rng-seed 1 \
  --out results
```

That writes three files into `results/`:

| file | what it is |
| --- | --- |
| `scores.csv` | one row per plan, one column per score |
| `by_district.jsonl` | the same plans, broken out district by district |
| `manifest.json` | what this run was, so somebody can repeat it |

North Carolina, 2,666 precincts, 10,000 steps: about two seconds.

## A worked walkthrough

[**WALKTHROUGH.md**](WALKTHROUGH.md) follows somebody who has never run this
before, from two files in a directory to a scored ensemble — including the
attempts that fail and what the errors say. Every command and every output in
it was actually run.

Read that if you would rather see the tool used than read its options. The
rest of this page is the reference.

## Finding the dataset names

The four dataset options are the ones people get stuck on, because the names
live inside the GeoJSON and vary by state and vintage. You never have to go
looking for them: DRA labels every dataset with a title, and composites say
which elections they average, so `rda-ensemble datasets <file>` reads that
back to you, grouped by the option each name belongs to.

As a rule:

- `--census` is a `T_` name. `T_20_CENS` is the 2020 census.
- `--vap` and `--cvap` are `V_` names, voting-age and citizen voting-age.
- `--elections` are `E_` names. A composite such as `E_16-20_COMP` averages
  several statewide races and is the usual choice; `--elections all` takes
  every one, which is thorough and slow.

Get one wrong and the error lists what the file actually carries:

```
rda-ensemble: --census T_20 is not in the GeoJSON.
Available: T_10_CENS, T_19_ACS, T_20_ACS, T_20_CENS, T_22_ACS
`rda-ensemble datasets NC.geojson` describes each one.
```

## Options

Everything below belongs to `rda-ensemble run`. The other command is
`rda-ensemble datasets <FILE>`, which takes nothing else.

Almost nothing has a default. Dataset names, district counts, tolerances and
chain parameters all change the answer, so the run has to say what it wants
rather than inherit a guess. `--help` prints all of this too.

### Input

| option | |
| --- | --- |
| `--geojson <FILE>` | the DRA GeoJSON for one state |
| `--state <XX>` | two-letter abbreviation, e.g. `NC` |
| `--plan-type <NAME>` | `congress`, `upper` or `lower` |
| `--districts <N>` | optional — how many districts to draw. Defaults to the number that chamber actually has |

North Carolina's congressional delegation has 14 seats, so `--plan-type
congress` is enough and `--districts` can be left out. Pass it to draw a
different number — a hypothetical map, or a state and chamber the built-in
table does not cover — and a disagreement with the statutory count is
reported rather than silently accepted.

### Datasets

| option | |
| --- | --- |
| `--cycle <YEAR>` | picks the population, voting-age and citizen voting-age datasets for that year |
| `--census <NAME>` | optional — overrides what `--cycle` picked |
| `--vap <NAME>` | optional — likewise |
| `--cvap <NAME>` | optional — likewise |
| `--elections <LIST>` | comma-separated, or `all` |
| `--expand-composites` | also score each election a composite averages, separately |

`--cycle 2020` reads the year off each dataset in the GeoJSON rather than
guessing from its name, so it does not care what a future export calls
things. Where a year has more than one candidate it takes the unqualified
one — the decennial count rather than the ACS estimate, the plain
voting-age population rather than the non-Hispanic-alone breakdown — and
where that is still ambiguous it lists the candidates and stops rather than
choosing for you.

`--elections` is deliberately not part of this. Which races to analyse is a
judgement, not a consequence of the decade.

### Starting plan

| option | |
| --- | --- |
| `--seed-tolerance <FRACTION>` | how far a district may sit from ideal population when the starting plan is drawn. `0.01` is within one percent |

A tighter tolerance takes longer to satisfy and can fail outright. If it does,
loosen it: the chain will move away from the starting plan anyway.

### Chain

| option | |
| --- | --- |
| `--steps <N>` | chain steps, counting rejected proposals |
| `--plans <N>` | how many plans you want, instead of `--steps`. `--plans 10000 --sample-every 200` runs two million steps |
| `--variant <NAME>` | which ReCom variant — see below |
| `--tolerance <FRACTION>` | population tolerance during the chain, usually looser than `--seed-tolerance` |
| `--target-pop <N>` | optional — ideal population per district. Defaults to the total divided by the district count |
| `--rng-seed <N>` | the seed; same seed and parameters reproduce the ensemble |
| `--region-weights <COL=W>` | required by, and only used by, the region-aware variants |
| `--balance-ub <N>` | required by, and only used by, `--variant reversible` |
| `--chains <N>` | independent chains, each from its own starting plan. Default `1`; `4` lets R-hat work. They run in parallel |
| `--segment-steps <N>` | run the chain in segments of N steps so it can be stopped between them. **Changes the ensemble** — see below |
| `--threads <N>` | default `1` |
| `--batch-size <N>` | default `1`; only matters above one thread |

All seven of rustrecom's variants:

- `cut-edges-ust` — the usual choice. Accepts most proposals, so it explores
  quickly.
- `cut-edges-region-aware` — the same, but prefers cuts that leave a region
  whole. This is how you run a chain that tries not to split counties.
- `reversible` — samples the intended distribution exactly, at the cost of
  rejecting most proposals. Needs `--balance-ub` and a lot more steps: a few
  hundred may accept nothing at all.
- `district-pairs-ust`, `cut-edges-mst`, `district-pairs-mst`,
  `district-pairs-region-aware` — variations on how district pairs and
  spanning trees are chosen.

`--threads` above 1 is faster but changes which plans a given `--rng-seed`
produces. Leave it at 1 for a run anybody needs to reproduce.

#### Stopping a run early

A chain normally runs to the end and the only way out is to kill the process,
which loses everything. `--segment-steps N` runs it in segments of N steps
instead, and Ctrl-C stops it at the end of the current one, keeping every plan
scored so far and writing the manifest and diagnostics as usual. A second
Ctrl-C quits immediately.

```sh
rda-ensemble run ... --steps 2000000 --sample-every 100 --segment-steps 2000
```

This is sound rather than a fudge: ReCom is Markov, so a segment that starts
from the plan the previous one ended on continues the same chain.

Two things to know.

**It changes the ensemble.** Each segment draws its own derived seed, so the
same `--rng-seed` at a different segment length is a different — equally
valid — chain. Quote the segment length alongside the seed when reporting a
result. It is in `manifest.json` either way.

**The progress bar is unaffected.** It is drawn from the step numbers the
scorer is handed, not from the segments, so it moves at the same rate either
way:

```
  [####################--------------------]  50%  30001/60000 steps  0:08  eta 0:08
```

Pick N from how long you are willing to wait to stop — segments of a few
thousand steps are seconds apart on a medium state.

A run that stopped early records `"stopped_early": true` in its manifest, so
a short ensemble cannot be mistaken for a complete one.

#### Keeping counties whole

```sh
rda-ensemble run ... \
  --variant cut-edges-region-aware \
  --region-weights COUNTY=1.0
```

`COUNTY` is a node attribute the dual graph always carries: each precinct's
five-character FIPS code. The sampler prefers cuts whose two ends sit in
different counties, which is to say cuts that leave counties whole. Higher
weights press harder.

On North Carolina, 2,000 steps, everything else equal:

| | counties split, mean | splitting rating, mean |
| --- | --- | --- |
| `cut-edges-ust` | 48.9 | 1.1 |
| `cut-edges-region-aware`, `COUNTY=1.0` | 10.2 | 65.6 |

`--region-weights` is repeatable and comma-separated, and takes any node
attribute the graph carries — today that is `COUNTY` and `GEOID`, and
weighting `GEOID` does nothing useful since it is unique per node. Weights
are applied most-important-first, as rustrecom orders them.

Passing weights to a variant that cannot use them is refused rather than
ignored, because rustrecom panics on that combination.

### What of rustrecom is not reachable

Most of rustrecom's chain options map onto the table above. Five do not,
and it is worth saying why rather than leaving you to guess:

| rustrecom | why not |
| --- | --- |
| `--config`, `--graph-json`, `--pop-col`, `--assignment-col`, `--overwrite-output` | this tool builds the graph and names its own columns; there is nothing to point at |
| `--constraint` | its one constraint reads two node attributes as a ratio, and the dual graph carries no demographic columns to read |
| `--sum-cols` | same: nothing to sum |
| `--edge-weight-keys` | needs per-edge attributes, and the dual graph carries none. rdarust has the shared-perimeter lengths that would go there |
| `--writer`, `--cut-edges-count`, `--bendl-graph-order` | scoring replaces rustrecom's writers, so its output formats never run |

The first row is structural. The rest are not refusals in principle — each
needs the dual graph or the writer to carry something it does not yet. If
you need one, the change is in `rdarust-io::recom`, not here.

### Output

| option | |
| --- | --- |
| `--out <DIR>` | where everything goes; created if absent |
| `--sample-every <N>` | score every Nth step, default `1` |
| `--keep <WHAT>` | also write an intermediate; repeatable, or `all` |
| `--prefixes` | prefix each score column with the dataset it came from |
| `--reverse-weight-splitting` | two extra county-splitting columns |
| `--no-progress` | suppress the progress bar, which is otherwise shown at a terminal for a single chain |

## Knowing what you got

Several options are shorthand. `--cycle` stands for three dataset names,
`--plan-type` stands for a district count, `--elections all` stands for
however many elections the file holds. So every run opens by printing what
it resolved, annotating anything the command line did not say outright:

```
Input
  state                     NC
  chamber                   congress
  districts                 14          statutory, NC congress

Data
  census                    T_20_CENS   --cycle 2020
  voting-age pop            V_20_VAP    --cycle 2020
  citizen VAP               V_20_CVAP   --cycle 2020
  elections                 E_16-20_COMP
...
```

`--dry-run` prints that and stops, without running the chain or writing
anything. It still reads the GeoJSON, so it catches a bad dataset name too.
Under a second on North Carolina.

`manifest.json` carries the same resolved values in machine-readable form,
including the ones with no flag at all — majority-minority districts are
always counted, and the score mode is always `all`, so both are recorded
rather than left to be inferred from the columns.

## How long is this going to take

Scoring costs about 1.4 ms per plan and the chain itself a few microseconds
per step, so the plan count dominates a short run and the step count a long
one. Measured, on an M-series Mac:

| | |
| --- | --- |
| NC, 1,000 plans at 10:1 (10,000 steps) | 2 seconds |
| MI, 10,000 plans at 200:1 (2,000,000 steps) | 9 min 30 s |

Scoring runs on its own thread while the chain keeps going, so the two overlap
and a long run costs less than multiplying the two rates suggests.

That second one is why the progress bar is on by default at a terminal. A
chain with no output is indistinguishable from a hung one, and ten minutes is
long enough that somebody will reasonably kill it.

There is no bar under `--chains` above 1, because several chains writing one
line would interleave into nonsense; each reports as it finishes instead.

Ask for plans, not steps. `--plans 10000 --sample-every 200` is the same run
as `--steps 2000000 --sample-every 200`, but `--steps 10000 --sample-every
200` — the literal reading of "10,000 plans at 200 to 1" — quietly gives you
fifty.

## Has it run long enough?

Every run writes `diagnostics.json` and prints a summary. Two different
questions live there, and the first is the one that misleads:

```
convergence
  R-hat        1.004 at worst, on average_margin   (4 chains of 500)
  effective N  142 at worst, on opportunity_districts   (of 500 sampled)
```

**Effective N** is how many independent draws the sample is worth, from the
integrated autocorrelation time. Consecutive ReCom plans differ in two
districts, so they are correlated, and 500 sampled plans can be worth 142.

**R-hat** asks whether the chain saw the whole distribution, and one chain
cannot answer it — a chain stuck in one corner has perfectly uncorrelated
samples *from that corner*. `--chains 4` runs four chains from four different
starting plans, in parallel, and compares them. Below 1.01 they agree; above,
they do not, and the run needs to be longer.

Measured on Michigan, everything else equal:

| | worst R-hat | |
| --- | --- | --- |
| 4 chains x 2,000 steps | 1.061 | not converged |
| 4 chains x 25,000 steps | 1.004 | converged |

Nothing else in the output distinguishes those two runs. Both finish, both
produce a full CSV, and only one of them is worth drawing conclusions from.

Scores that do not depend on the districting — statewide vote share, for
instance — are skipped rather than diagnosed, since they are the same in
every plan.

## Sampling

`--sample-every 10` scores steps 0, 10, 20 and so on, and the rows are named
by step number. Use it when the chain is long: 100,000 steps at one row each
is a large CSV, and consecutive plans differ by only two districts anyway.

One detail worth knowing. A ReCom chain **self-loops**: a step whose proposal
is rejected leaves the plan unchanged. Those steps are sampled again, so the
same plan can appear twice. That is deliberate — dropping the repeats would
bias the ensemble, because the stationary distribution the chain converges to
depends on them being counted. `--sample-every` selects the same steps
rustrecom's own `--sample-interval` would.

## Keeping intermediates

By default nothing between the stages reaches disk. `--keep` writes any of
them, in exactly the format the stage-by-stage `rdarust` CLI writes:

| `--keep` | file | |
| --- | --- | --- |
| `data-map` | `data_map.json` | which datasets and fields were read |
| `graph` | `graph.json` | which precincts border which |
| `data` | `precinct_data.jsonl` | population, votes and shapes per precinct |
| `recom-graph` | `recom_graph.json` | the chain's dual graph, seed plan included |
| `seed-plan` | `seed_plan.csv` | the starting plan, as `GEOID,District` |
| `plans` | `plans.jsonl` | the ensemble itself, as geoid to district |

`--keep all` writes the lot. Each one costs a serialisation and nothing else:
the value is already in memory, which is why keeping it is cheap and why the
file matches the CLI's byte for byte.

`plans.jsonl` is the useful one. It is what `rdarust score-all` reads, so you
can re-score an ensemble with different options without running the chain
again.

## Settings as a file

Every run writes `settings.json` beside its results, so usually there is
nothing to do. When all you have is someone's `manifest.json`:

```sh
rda-ensemble settings --from their-results/ > settings.json
```

`--from` takes a manifest, a settings file, or a directory holding either.
The JSON goes to standard output so it pipes; the portability warnings go to
stderr so they do not land in the file. `--out FILE` writes instead.

`replay` already reads a manifest directly, so this is not needed to repeat a
run. It is for having the settings *as a file* — to keep, to edit, to send, or
to diff against another run's.

One difference worth knowing. A manifest records what a run **resolved**; a
settings file records what it was **asked**. Converting one to the other
therefore pins things the original left open:

```
  "adjacency": "auto",        ->  "adjacency": "dra",
  "census": null,             ->  "census": "T_20_CENS",
  "vap": null,                ->  "vap": "V_20_VAP",
  "cvap": null,               ->  "cvap": "V_20_CVAP",
```

Both reproduce the same ensemble today. The converted one also reproduces it
if a future DRA package would have resolved `--cycle 2020` differently, which
is the stricter and usually better position to be in.

## Making an ensemble longer

```sh
rda-ensemble extend --from results --out results-longer --plans 5000
```

Reads the earlier run's settings and its last plan, copies its results into
`--out`, and carries the *same chain* on from where it stopped. ReCom is
Markov, so the next plan depends on the current one and nothing else: this is
the chain continued, not a second chain started somewhere plausible. The
original directory is left exactly as it was.

The earlier run must have been made with `--keep plans`; the plan to resume
from is otherwise not written down, and `extend` says so rather than guessing.

### It gives you exactly the longer run

Run 500 steps, or run 300 and extend by 200: the same `scores.csv`,
`plans.jsonl`, `by_district.jsonl`, `diagnostics.json` and `settings.json`,
byte for byte. Chained extensions too — 300 then 500 then 800 equals one run
of 800.

That works because a segmented run puts its boundaries at exact multiples of
the segment length and derives each segment's seed from its index. A 300-step
run and a 500-step run therefore *share* segments 0, 1 and 2: segment 2 starts
at step 200 from the same plan with the same seed in both, and the longer run
merely runs it further. The two only part company where the shorter one
stopped mid-segment.

So `extend` rejoins at a boundary rather than wherever the run happened to
stop, which means backing up — the steps between the last boundary and the
end are regenerated. Nothing is lost: they regenerate identically, being a
prefix of the same stream. `--steps 200` still means two hundred steps more
than the ensemble has, not two hundred past the resume point.

One condition: **the earlier run must have been segmented.** An unsegmented
chain is one continuous stream with no boundary to rejoin.

The two intervals need no particular relationship. `--segment-steps` and
`--sample-every` count different things — where the chain can be interrupted,
and which steps are scored — and the sampling grid is absolute, so boundaries
never disturb it. `extend` simply looks for the last saved plan sitting on a
multiple of the segment length:

| segment | sample every | |
|---|---|---|
| 500 | 2000 | every saved plan is a boundary |
| 2000 | 500 | every boundary is a saved plan |
| 300 | 2000 | both only at multiples of 6000, so it backs up further |

All three resume exactly. Passing a *different* `--segment-steps` to `extend`
does not, because it moves every boundary.

Single-chain runs only, for now. With `--chains` it is no longer one question
which chain an R-hat was computed over, so `extend` refuses rather than
guessing.

## What has been downloaded

```sh
rda-ensemble cache
```

```
/Users/you/Library/Caches/rda-ensemble

  state  version       size   adjacency
  IL     v07          34.7M   DRA graph
  MI     v06          16.0M   DRA graph

  2 package(s), 50.6 MB
  index    221 kB, re-fetched after a day
```

Packages are kept per state *and* version, so pinning `--dra-version v06`
after using v07 leaves both. That adds up quietly — a sweep of every state is
about 780 MB — which is why this command exists.

`--forget XX` removes one state, `--forget XX --dra-version vNN` one package,
`--forget-all` the lot, and `--forget-index` drops the cached list of what DRA
publishes. `--cache DIR` points any of this somewhere else, as it does for
every other subcommand.

## Reproducing a run

Every run writes **`settings.json`** beside its results: every decision it
made, and none of your paths. Hand that file to someone else and

```sh
rda-ensemble replay --settings their-settings.json --out results
```

reproduces the ensemble. `replay` also reads a `manifest.json`, which is what
people usually have, since it is written whether or not anyone thought to keep
settings.

What the file deliberately omits is the GeoJSON path, the cache directory and
the output directory. Those differ between machines; the state and the DRA
version are what let anyone fetch the same data. If no DRA version was pinned,
`replay` says so before it starts.

`manifest.json` is always written. It records the input file and a fingerprint
of its contents, the state, district count and datasets, every chain
parameter, the seed, how many plans were scored, and the versions involved.

In a one-command tool there is no shell history to fall back on, so this is
the only record of what produced an ensemble. Keep it with the results.

Three recorded fields change the plans a given `--rng-seed` produces, so a
reproduction has to match all three: `segment_steps`, `threads` and the DRA
`dra_version`. `stopped_early` says whether the run finished.

## Using it as a library

The binary is one caller of this crate, not the only possible one — a GUI is
the reason the seam exists. A caller supplies parameters as `RunArgs`, whose
fields are public, and receives the narration through an `Events` sink instead
of stderr:

```rust
use std::sync::{Arc, Mutex};
use rda_ensemble::events::{Events, Sink};
use rda_ensemble::run::{self, Cancel};

struct Window(Mutex<Vec<String>>);

impl Events for Window {
    fn status(&self, line: &str) { self.0.lock().unwrap().push(line.into()); }
    fn warn(&self, line: &str)   { self.0.lock().unwrap().push(line.into()); }
    fn progress(&self, step: u64, total: u64) { /* drive a bar */ }
}

let ev: Sink = Arc::new(Window(Mutex::new(Vec::new())));
let cancel = Cancel::default();          // wire this to a Stop button
run::run(&args, &[], &cancel, &ev)?;
```

`Cancel` stops the chain at the next segment boundary, so a caller that wants
a responsive stop sets `segment_steps`. Discovery returns data rather than
text: `dra::listings` for the states and versions, `datasets::read` for what a
GeoJSON carries. Each has a `render` beside it that produces the lines the CLI
prints, so the terminal and a menu are fed from the same place.

## Building

Three libraries, two of them outside this repository:

- [rdarust](..) — extraction and scoring
- [partigraph](https://github.com/proebsting/partigraph-rust) — the starting plan
- [rustrecom](https://github.com/mggg/rustrecom) — the chain

The paths to the latter two are in `Cargo.toml`. Adjust them, then:

```sh
cargo build --release
```

This crate is deliberately **not** a member of the rdarust workspace, because
it cannot build without those two checkouts. That also means the directory
lifts out into its own repository unchanged: only the three path dependencies
would need to become git or version ones.

The result links nothing but system libraries, so the binary can be copied to
another machine of the same platform and run.

## When it goes wrong

**"the precinct graph is in N pieces"** — islands and water crossings leave
precincts with no land neighbour, and a chain cannot reach them.
`rdarust contiguity-mods` proposes the edges that would join them, and
`rdarust apply-mods` adds them.

**The seed plan fails to balance** — loosen `--seed-tolerance`.

**"No proposals were accepted during the entire chain run"** — reversible
ReCom rejects most proposals by design. Use more steps, or
`--variant cut-edges-ust`.

**A dataset name is rejected** — the error lists what the file actually
carries. See *Finding the dataset names* above.
