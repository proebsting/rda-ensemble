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

Two commands. The first tells you what the second needs.

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
| `--variant <NAME>` | which ReCom variant — see below |
| `--tolerance <FRACTION>` | population tolerance during the chain, usually looser than `--seed-tolerance` |
| `--target-pop <N>` | optional — ideal population per district. Defaults to the total divided by the district count |
| `--rng-seed <N>` | the seed; same seed and parameters reproduce the ensemble |
| `--region-weights <COL=W>` | required by, and only used by, the region-aware variants |
| `--balance-ub <N>` | required by, and only used by, `--variant reversible` |
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
| `--progress` | chain progress on stderr |

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
| `plans` | `plans.jsonl` | every scored plan, as geoid to district |

`--keep all` writes the lot. Each one costs a serialisation and nothing else:
the value is already in memory, which is why keeping it is cheap and why the
file matches the CLI's byte for byte.

`plans.jsonl` is the useful one. It is what `rdarust score-all` reads, so you
can re-score an ensemble with different options without running the chain
again.

## Reproducing a run

`manifest.json` is always written. It records the input file and a fingerprint
of its contents, the state, district count and datasets, every chain
parameter, the seed, how many plans were scored, and the versions involved.

In a one-command tool there is no shell history to fall back on, so this is
the only record of what produced an ensemble. Keep it with the results.

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
