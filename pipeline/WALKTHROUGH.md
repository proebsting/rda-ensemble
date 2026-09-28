# NC Ensemble Walkthrough

Everything below was actually run. The outputs are copied verbatim, including
the mistakes. Re-run on 28 September 2026 against the current build, which is
several fixes further on than the first draft.

## What you start with

One file. The binary, and nothing else installed.

```
$ ls -lh
-rwxr-xr-x  4.2M  rda-ensemble
```

No GeoJSON, no Python, no R. The tool fetches its own data.

The user knows three things about what they want: **North Carolina**,
**congressional districts**, and about **a thousand plans**. They do not know
what a data map is, what ReCom is, what any dataset inside DRA's files is
called, or that North Carolina has 14 congressional districts.

## Step 1: what does this thing do?

```
$ ./rda-ensemble --help
Redistricting ensembles from a Dave's Redistricting GeoJSON

Usage: rda-ensemble <COMMAND>

Commands:
  datasets  List the datasets a GeoJSON carries, with what each one is
  states    List the states DRA publishes data for, and which versions exist
  fetch     Download a state's data from DRA and unpack it
  run       Generate an ensemble and score every plan
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

Four things it can do, and each reads like a question somebody might have. A
wall of twenty options here would not.

## Step 2: what is in my file?

`datasets` takes a state, not a path, so it works before anything is
downloaded — the file it needs is fetched and kept.

```
$ ./rda-ensemble datasets --state NC
  downloading NC v07 (4.6 MB)

Total population  (--census)
  T_22_ACS      Total Pop (ACS) 2022
  T_20_ACS      Total Pop (ACS) 2020
  T_20_CENS     Total Population 2020
  T_19_ACS      Total Population 2019
  T_10_CENS     Total Population 2010

Voting-age population  (--vap)
  V_20_VAP      Voting Age Pop 2020
  V_20_VAP_NH   VAP (NH race) 2020
  V_10_VAP      Voting Age Pop 2010

Citizen voting-age population  (--cvap)
  V_22_CVAP     Citizen VAP 2022
  V_20_CVAP     Citizen VAP 2020
  V_19_CVAP     Citizen VAP 2019

Elections  (--elections)
  E_24_AG       Attorney Gen 2024
  E_24_CONG     Congress 2024
  ...
  E_16-20_COMP  Composite 2016-2020
                averages E_20_PRES, E_20_GOV, E_20_SEN, E_16_PRES, E_20_AG, E_16_SEN
  ...

Pass any of these names to the matching option of `rda-ensemble run`,
or `--elections all` to score every election at once.
```

`T_20_CENS` means nothing on its own; "Total Population 2020" does.

The user needs one name from this list: **`E_16-20_COMP`**, a composite of six
statewide races, rather than betting the analysis on a single election. The
`averages` line under each composite is what makes that choice possible
without knowing the field beforehand.

The three demographic datasets do not have to be copied out. `--cycle 2020`
picks them, using the year DRA tags each one with. This listing is still where
you check *what it will pick*, and it is the only place to look when a year has
more than one candidate — 2020 carries both `T_20_CENS` and `T_20_ACS`, and the
difference between a decennial count and an ACS estimate is not something to
discover by accident.

## Which states, and which version?

```
$ ./rda-ensemble states
  state  latest       size   older
  AK     v07          2.6M   v06
  AR     v06          2.9M   -
  NC     v07          4.6M   v06
  ...
```

52 of them, including DC and Puerto Rico. Versions are not uniform — most
states are at `v07`, eleven are still at `v06` — so leaving `--dra-version` out
and taking the newest is the right default rather than a shortcut.

## Step 3: what does a run need?

```
$ ./rda-ensemble run --help
Generate an ensemble and score every plan.

Reads one GeoJSON, builds the precinct graph, draws a population-balanced starting plan, runs a
ReCom chain, and scores plans as the chain produces them. Nothing is written between stages unless
--keep asks for it.

Usage: rda-ensemble run [OPTIONS] --state <XX> --plan-type <NAME>
       --elections <LIST> --seed-tolerance <FRACTION> --variant <VARIANT>
       --tolerance <FRACTION> --rng-seed <N> --out <DIR>

Input:
      --geojson <FILE>
          The DRA GeoJSON for one state. Left out, the state's data is downloaded from DRA
          and kept in --cache

      --state <XX>
          Two-letter state abbreviation, e.g. NC

      --plan-type <NAME>
          Which chamber the plan is for. Fixes how many districts to draw, unless --districts
          says otherwise

      --districts <N>
          How many districts to draw. Defaults to the number this state's chamber actually has,
          so most runs leave it out

Datasets:
      --cycle <YEAR>
          Census cycle, e.g. 2020. Picks the population, voting-age and citizen voting-age
          datasets for that year out of the GeoJSON, so most runs need nothing else here

      --census <NAME>
          Census dataset name, overriding --cycle's choice. e.g. T_20_CENS
      ...

Chain:
      --steps <N>
          Chain steps to run, counting rejected proposals. One of --steps or --plans;
          --plans is usually what you mean

      --plans <N>
          How many plans you want in the ensemble. The chain is run for as many steps as
          that needs: --plans 10000 --sample-every 200 runs two million steps

      --variant <VARIANT>
          Which ReCom variant to run

          Possible values:
          - reversible:          Reversible ReCom. Samples the intended distribution exactly, at
            the cost of rejecting most proposals. Requires `--balance-ub`
          - cut-edges-ust:       Non-reversible; district pairs chosen by a random cut edge,
            spanning trees uniform. The usual choice when reversibility is not needed
          ...
```

*(Trimmed; the real output runs to about 90 lines.)*

**Eight options are required**, down from thirteen when this walkthrough was
first written. `--geojson` went, because the tool fetches. `--districts` went,
because the chamber fixes it. The three dataset names went, because `--cycle`
picks them. All still exist as overrides.

The two a newcomer cannot reason about are `--variant` and the two tolerances.
The variant list says which one is "the usual choice", which is the only steer
most people need.

## Step 4: the attempts that fail

Four mistakes a newcomer actually makes, in the order they tend to make them.

**Forgetting that `run` is a subcommand.** The most likely first mistake, and
the worst-handled:

```
$ ./rda-ensemble --geojson NC.geojson --state NC
error: unexpected argument '--geojson' found

  tip: a similar argument exists: '--version'

Usage: rda-ensemble --version <COMMAND>
```

The tip is nonsense — `--version` is not what they meant, and the usage line it
prints is not a command anybody would want to run. This is the one bad error in
the set.

**Guessing dataset names.** Less likely than it was, now that `--cycle` covers
the three demographic ones, but `--elections` still has to be named:

```
$ ./rda-ensemble run --elections NOPE ...
rda-ensemble: --elections names NOPE that the GeoJSON does not carry.
Available: E_08_PRES, E_12_PRES, E_14_SEN, E_16-20_COMP, E_16-22_COMP, ...
`rda-ensemble datasets <file>` describes each one, and `--elections all` takes them all.
```

This one recovers itself: the list is right there, and it names the command
that explains the list.

**Asking for a tolerance nothing can satisfy.** Someone who reads "tolerance"
as "accuracy" will set it small:

```
$ ./rda-ensemble run --seed-tolerance 0.0000001 ...
  2666 precincts, 1 election(s)
drawing a starting plan with 14 districts
rda-ensemble: no valid partition found in 1000 attempts
A tighter --seed-tolerance is harder to satisfy; try loosening it.
```

It takes about four seconds to fail, which is long enough to wonder and short
enough not to mind.

**Leaving out a required option.**

```
$ ./rda-ensemble run ...            # no --plan-type
error: the following required arguments were not provided:
  --plan-type <NAME>
```

Exact, and it names only what is actually missing.

**Asking for a run and getting no data.** GitHub allows sixty unauthenticated
requests an hour, and somebody experimenting can spend them:

```
rda-ensemble: GitHub's rate limit for unauthenticated requests is used up, and
resets in about 29 minute(s).
It resets on its own; meanwhile `--geojson FILE` skips GitHub entirely, and an
already-downloaded state in the cache still works.
```

It says the three things worth knowing: that it is temporary, roughly how
temporary, and that there are two ways past it. A state already in the cache
keeps working, because asking which version is newest needs the network but
using one already downloaded does not.

Typing `rda-ensemble` with no arguments prints the help rather than an error,
which is the right thing for somebody poking at an unfamiliar binary.

## Step 5: check, then run

Adding `--dry-run` works everything out, prints it, and stops without writing
anything. It still reads the GeoJSON, so a bad dataset name is caught here too.
Under a second.

```
$ ./rda-ensemble run ... --dry-run
reading NC.geojson

Input
  state                     NC
  chamber                   congress
  districts                 14          statutory, NC congress

Data
  census                    T_20_CENS   --cycle 2020
  voting-age pop            V_20_VAP    --cycle 2020
  citizen VAP               V_20_CVAP   --cycle 2020
  elections                 E_16-20_COMP

Starting plan
  population tolerance      0.01

Chain
  variant                   cut-edges-ust
  steps                     10000               --plans 1000 x --sample-every 10
  population tolerance      0.05
  rng seed                  1
  threads                   1
  batch size                1

Scoring
  sample every              10 step(s)
  majority-minority         counted     always on
  reverse-weight splitting  not reported
  column names              plain

--dry-run: nothing was written
```

This is where the shorthand stops being a leap of faith. The third column
appears only where the command line did not say a value outright, so the user
can see that 14 came from the statutory table and that `--cycle 2020` chose
`T_20_CENS` rather than `T_20_ACS`. Values they typed themselves stand alone —
explaining those would bury the ones that matter.

Two lines have no flag behind them at all. Majority-minority districts are
always counted, and the score mode is always everything; both are printed
because otherwise the only way to learn them is to notice which columns turned
up in the CSV.

Satisfied, the same command without `--dry-run`:

```
$ ./rda-ensemble run \
    --geojson NC.geojson \
    --state NC --plan-type congress --cycle 2020 \
    --elections E_16-20_COMP \
    --seed-tolerance 0.01 \
    --plans 1000 --sample-every 10 \
    --variant cut-edges-ust --tolerance 0.05 --rng-seed 1 \
    --out results

reading NC.geojson

  ... the same settings block ...

  2666 precincts, 1 election(s)
drawing a starting plan with 14 districts
  worst district is 0.085% from ideal
running 10000 steps, scoring every 10
scored 1000 plans, from steps 0 to 9999, in 1.9s

wrote results
  scores.csv          one row per scored plan
  by_district.jsonl   the same plans, district by district
  manifest.json       what this run was, so it can be repeated
```

**2.0 seconds**, from a 14 MB GeoJSON to a thousand scored plans. Two seconds
is fast enough that a newcomer will try it several times with different
settings, which is the point.

Notice what is *not* in that command. No `--districts`: North Carolina's
congressional delegation has 14 seats and the tool knows. No `--census`,
`--vap` or `--cvap`: `--cycle 2020` picks them, and echoes what it picked so
the choice is not silent.

The progress lines are chosen to be checkable. "2666 precincts" is a number
somebody who knows North Carolina can sanity-check. "worst district is 0.085%
from ideal" says the starting plan is genuinely balanced, not merely accepted.
And the closing block names each file and what is in it, rather than leaving
three filenames to be interpreted.

Why the remaining settings:

- `--variant cut-edges-ust` because the help calls it "the usual choice".
  `reversible` is more rigorous and needs far more steps.
- `--tolerance 0.05` — five percent during the chain, looser than the one
  percent the starting plan was drawn to. Congressional districts are held to
  much tighter in law, but a chain needs room to move.
- `--plans 1000 --sample-every 10`, which is 10,000 chain steps. Asking for
  plans rather than steps is the point: consecutive plans differ in only two
  districts, so sampling is normal, and `--steps 1000 --sample-every 10` —
  the literal reading of "1,000 plans at 10 to 1" — would quietly give a
  hundred.
- `--elections E_16-20_COMP` from step 2, a composite of six statewide races
  rather than betting the analysis on one election. This one stays explicit on
  purpose: which races to analyse is a judgement, not a consequence of the
  decade.
- `--rng-seed 1` because it was required. That turns out to matter — see the
  manifest below.

## Step 6: what came out

```
$ ls -lh results/
4.3M  by_district.jsonl
813B  manifest.json
301K  scores.csv
```

`scores.csv` has 1,000 rows and 44 columns. The first few, on a handful of
columns:

| name | population_deviation | proportionality | competitiveness | minority | compactness | efficiency_gap | reock |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 000000 | 0.001600 | 73 | 34 | 64 | 24 | 0.047800 | 0.339400 |
| 000010 | 0.091400 | 64 | 45 | 60 | 40 | 0.065400 | 0.398700 |
| 000020 | 0.086000 | 98 | 45 | 59 | 30 | -0.001700 | 0.349000 |

The name is the chain step, so `000010` is step 10. Row one is the starting
plan partigraph drew, which is why its population deviation is 0.0016 against
0.09 for the others: it was drawn to a one-percent tolerance, and the chain is
allowed five.

That is a spreadsheet anybody can open. The ratings (proportionality,
competitiveness, minority, compactness) are 0–100 scores in DRA's own scale;
the rest are the underlying measures.

`manifest.json` is written every time:

```json
{
  "tool": "rda-ensemble",
  "input": {
    "geojson": "NC.geojson",
    "geojson_fingerprint": "fnv1a64:e731e9090a7162ac",
    "state": "NC",
    "plan_type": "congress",
    "districts": 14,
    "districts_from": "statutory",
    "precincts": 2666,
    "cycle": 2020,
    "census": "T_20_CENS",
    "vap": "V_20_VAP",
    "cvap": "V_20_CVAP",
    "elections": ["E_16-20_COMP"],
    "shapes_label": "S_20_DRA",
    "expand_composites": false
  },
  "chain": {
    "steps_requested": 10000,
    "steps_taken": 9999,
    "variant": "cut-edges-ust",
    "tolerance": 0.05,
    "seed_tolerance": 0.01,
    "rng_seed": 1,
    "balance_ub": null,
    "threads": 1,
    "batch_size": 1
  },
  "output": {
    "sample_every": 10,
    "plans_scored": 1000,
    "prefixed_columns": false,
    "majority_minority": true,
    "reverse_weight_splitting": false,
    "score_mode": "all",
    "seconds": 1.940149333
  },
  "versions": {
    "rda-ensemble": "0.1.0",
    "rdarust": "0.1.0"
  }
}
```

Every value in there can be typed back into a command line, which is the test
it was written to pass. The fingerprint answers "is this the same GeoJSON I ran
on". Six months later, that file is the only thing standing between the results
and nobody knowing how they were made.

This is the settings block again, in a form a program can read, and it carries
the things the block cannot. `districts_from` says whether 14 came from the
statutory table or from `--districts`, since the number alone cannot. `census`,
`vap` and `cvap` record what `--cycle 2020` resolved to — the datasets used,
not the flags given. `majority_minority` and `score_mode` have no flag behind
them at all, and without them nothing says which columns this CSV has.

Writing this section is what caught two of those. The first version recorded
the flags rather than the resolved names, so a `--cycle` run wrote
`"census": null`; and `reverse_weight_splitting` was a real option that was
simply never written down.

`shapes_label` is worth one line of caution. rdapy writes `S_20_DRA` into every
data map, and it looks like a dataset name, but it appears nowhere in the
GeoJSON — not in `datasets`, not on any feature. The geometry comes from the
file's own `geometry` field. It is a label, not a choice that was made, which
is why the key says so.

## Where it still trips people up

Writing this walkthrough turned up more than I expected, and six of them are
now fixed. What follows is the state after those fixes, with the struck-through
headings kept so the reasoning is still readable.

### ~~`--districts` is redundant and `--plan-type` does nothing~~ — fixed

Writing this walkthrough found a real bug. `--plan-type` had nothing to do: its
only functional use in rdarust is looking up the statutory district count, and
the driver passed `--districts` as an override on every run, so the chamber
never reached anything. Both of these ran without complaint:

```
--plan-type senate --districts 14     # not one of the three chamber names
--plan-type congress --districts 13   # NC's delegation has 14
```

This is now fixed. rdarust's own CLI already had the right shape — `score-all`
takes `--plan-type` as required and `--districts-override` as the exception —
and the driver had inverted it.

`--districts` is optional and defaults to the statutory count:

```
$ ./rda-ensemble run --state NC --plan-type congress ...    # no --districts
drawing a starting plan with 14 districts
```

A chamber name that is not one of the three is refused, with the three listed:

```
$ ./rda-ensemble run --plan-type senate ...
error: invalid value 'senate' for '--plan-type <NAME>'
  [possible values: congress, upper, lower]
```

And a `--districts` that disagrees is reported rather than swallowed, since the
deliberate case — a hypothetical map — is legitimate:

```
$ ./rda-ensemble run --plan-type congress --districts 13 ...
warning: NC congress has 14 districts, and --districts says 13. Drawing 13.
```

A state and chamber with no statutory count now says so before reading the
GeoJSON:

```
$ ./rda-ensemble run --state NE --plan-type lower ...
rda-ensemble: no statutory district count for NE lower; pass --districts to say how many
```

The required-option count drops from thirteen to twelve, and the walkthrough's
premise holds: a user who wants North Carolina congressional districts no
longer has to know there are 14.

### ~~Three dataset names nobody can guess~~ — fixed

`--cycle 2020` now picks the population, voting-age and citizen voting-age
datasets, reading the year DRA tags each one with rather than parsing its name.
Ten required options, down from thirteen.

It nearly shipped with a quiet bug. The first version took the alphabetically
smallest match for each year, which chose `T_20_ACS` over `T_20_CENS` — an ACS
estimate instead of the decennial census — and ran perfectly happily, moving
the starting plan's worst deviation from 0.085% to 0.135%. Exactly the class of
mistake this tool is meant not to make.

DRA marks its variants: `nhAlone` on the non-Hispanic-alone breakdown, an ACS
`description` on the estimates, and nothing on the headline dataset. So
`--cycle` takes the unqualified one, and where that still leaves a tie it lists
the candidates and stops rather than choosing.

`--elections` is deliberately not part of it. Which races to analyse is a
judgement, not a consequence of the decade.

### ~~Everything needed a GeoJSON you had to find yourself~~ — fixed

The first draft of this walkthrough began with two files in a directory and
said nothing about where the GeoJSON came from. That was the largest gap in
it: DRA publishes 52 states at versions that differ state by state, and
rdapy's own download script makes `--version` mandatory with no way to
discover it, so anybody following the instructions guessed and got a 404.

`states` lists what exists, `fetch` downloads, and `run` and `datasets` do it
themselves when no file is named. Leave `--dra-version` out and the newest is
used. Packages are kept in a per-user cache, so building six ensembles for
one state downloads once.

### ~~Plans and steps were easy to confuse~~ — fixed

"Ten thousand plans sampled at 200 to 1" is two million chain steps, and the
tool only took steps. Typing the sentence literally — `--steps 10000
--sample-every 200` — gave fifty plans, with no error and no warning, after a
run that looked entirely successful. That is the only failure found in this
walkthrough that produced a wrong answer rather than a message.

`--plans` now does the arithmetic and the settings block shows its working:

```
  steps                     2000000             --plans 10000 x --sample-every 200
```

### ~~No way to tell what the shorthand expanded to~~ — fixed

A consequence of the two fixes above: by the time a run needed only ten
options, four of the values it used were no longer written anywhere the user
could see. Shorthand that hides what it chose is worse than the typing it
saves, particularly for a tool whose output is meant to be evidence.

Every run now opens with the settings block in step 5, annotating anything the
command line did not say outright, and `--dry-run` prints it without running
the chain. The manifest carries the same values for a program to read,
including two — majority-minority scoring and the score mode — that have no
flag at all and were previously undiscoverable except by inspecting the CSV's
columns.

### The `run` subcommand error

`tip: a similar argument exists: '--version'` is actively misleading, and the
usage line it suggests is nonsense. It is clap's fuzzy matcher, not our text,
but it is the first thing a fair number of people will see.

### Two tolerances

`--seed-tolerance` and `--tolerance` sit in different help sections and mean
the same kind of thing at different moments. I expect people to swap them. A
single `--tolerance` with the starting plan drawn tighter automatically would
remove the question, at the cost of control that most users do not want.

### A long run looks like a hung one

The Michigan ensemble above — 10,000 plans at 200 to 1 — is two million steps
and takes about half an hour. Between "running 2000000 steps" and the end,
nothing was printed. The progress bar is now on by default at a terminal, and
`--no-progress` turns it off, but the underlying point stands: nothing warns
you before you start that the run you have asked for is a long one.

### Nothing says how many steps is enough

This is the real scientific gap. `--steps 10000` was picked because it is a
round number, and the tool offers no opinion and no diagnostic. Whether a chain
has mixed is the central question in ensemble work, and right now a user has no
way to tell from anything the tool produces. Not a documentation problem — it
needs a convergence statistic in the output.

### Smaller things

- `--rng-seed` is required with no hint about what to choose. It wants a line
  saying any number will do, and to write it down.
- What to *do* with `scores.csv` is not covered either. The tool produces an
  ensemble; the analysis is left to the reader.
- A macOS user who downloads the binary will hit Gatekeeper before any of this,
  and the fix is a terminal incantation.

### What worked

Worth recording, since it is the part not to break: `datasets` carried the
whole middle of this walkthrough. Every dataset error names what the file
actually holds and points at the command that explains it, so the two most
likely mistakes are self-correcting. And at two seconds a run, experimenting is
free.
