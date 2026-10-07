# rda-ensemble, in a window

A front end for [`rda-ensemble`](../pipeline), for people who would rather not
type a forty-flag command line. It is a window, not a second implementation:
every button calls the same library the CLI drives, so the two cannot disagree
about what a run is.

```
gui/
  src-tauri/   the Rust side: ten commands, each a thin call into rda-ensemble
  ui/          one HTML file, no build step
```

## Running it

```sh
cd gui/src-tauri
cargo run --release
```

No npm, no bundler, no `tauri` CLI. The page is plain HTML and talks to Rust
through `window.__TAURI__`.

## What it shows

Three tabs, for three jobs that share nothing:

- **Build an ensemble** — every decision a run makes, all 28 of them, grouped
  as the `--help` is: data, datasets, starting plan, chain, output.
- **Extend one** — continuing an earlier run's chain. It takes a directory,
  not the form.
- **Downloads** — what is cached and how much room it takes. Maintenance, not
  part of building anything.

Progress and the log sit below the tabs rather than inside one, because only
one job runs at a time and switching tabs should not lose sight of it.

Nothing is hidden behind an "advanced" disclosure yet. That is deliberate for
now: it is easier to learn which settings nobody touches by watching than by
guessing.

### What is still needed

A strip under the tabs lists every decision the run wants and has not been
given, and the fields themselves are outlined where they sit. It updates as
you type. Nobody should have to press a button to discover they have not
finished filling in a form.

That list is not written here. It comes from `Settings::missing` in the
library — the same function `into_args` calls to refuse an incomplete run — so
the form cannot ask for one thing while the run requires another.

Two exceptions it cannot cover, both handled in the page and commented there:
a seed of zero is a perfectly good seed, so only an empty box means undecided;
and the output directory is not part of the settings at all.

The state menu comes from DRA's index. The dataset menus come from the chosen
state's own file, so choosing a state reads it — silently when the package is
already downloaded, and otherwise saying what it would fetch and waiting for
you to agree:

```
MI v06 has not been downloaded yet (3.8 MB).  [Download and read its datasets]
```

Nobody should have to work out that a menu is empty because a button has not
been pressed.

### Census year

A cycle is a year, and a year is a shortcut for three datasets: total
population, voting-age, and citizen voting-age. So the year **fills the three
menus in**, and says which it chose:

```
Census year  [2020 ▾]   chose T_20_CENS, V_20_VAP and V_20_CVAP
```

It is not an alternative to those menus, and an earlier version that presented
it as one — with an option reading "name the three below instead" sitting in a
list of years — was indefensible. The menus stay editable afterwards and
whatever is in them is what runs.

Where a year cannot choose on its own, it says so instead: 2020 carries both a
decennial count and an ACS estimate of total population, and they are not
interchangeable. `datasets::survey` resolves every year in one pass over the
file, so the window never re-reads tens of megabytes to answer this.

### Elections

What gets scored is always a **plan**. But the partisan metrics — efficiency
gap, declination, seats bias — cannot be computed from a plan alone: they ask
how it would have treated the parties, which needs real votes. An election
dataset supplies those votes, which is why the label is *Election results to
judge plans by* and not, as it once was, "elections to score".

Picking two elections does not score two things. It scores every plan twice,
once against each set of results.

A **composite** is one dataset holding an average of several contests;
`E_16-20_COMP` blends 2016, 2018 and 2020. Ticking *also judge plans by each
election a composite is made of* adds those contests as bases of their own, so
a finding can be checked against each year rather than only the blend.

### Defaults, and what has none

Where a setting has a real default the label says so — `auto (default)`,
`1 by default`, `blank is the default: the chamber's real count`. Five options
genuinely default in the CLI (`adjacency`, `chains`, `threads`, `batch_size`,
`sample_every`); the rest are "blank means X" behaviours, marked the same way.

Writing those in turned up an inconsistency. The window used to pre-select a
**chamber**, which the CLI requires and refuses to guess. Congressional and
legislative plans differ in almost everything, so nothing should pick one for
you: the menu now starts empty and the chamber appears in "still needed" until
it is set.

The **variant** went the same way, and for the same reason. `cut-edges-ust` is
what most published ensembles use, but "most" is not "obviously", and which
variant ran changes what the ensemble means. Both menus start empty, and the
seven names in the menu are checked against `settings::VARIANTS` so the window
cannot offer one the library would reject.

What is left pre-selected is only what the CLI itself defaults: `adjacency`,
`chains`, `threads`, `batch_size`, `sample_every`.

### Labels

A label asks a question rather than naming a flag. "Plans" became **Ensemble
size**, "Cycle" became **Census year**, "Sample every" became **Keep one plan
every** — with the unit moved into the hint beside it, so the sentence
finishes.

"Adjacency" became **Which precincts count as neighbours**, which took two
goes. "Where precinct borders come from" was plainer but wrong: DRA's graph
joins precincts across water, where there is no shared border at all, and that
is precisely why Alaska runs with DRA's graph and comes apart into
disconnected pieces without it. A label that promised borders described the
option that fails.

Where a rename moves far from what the settings file calls the field, the hint
gives the key: *saved as `cycle`*, *saved as `plans`*. Someone reading a
`settings.json` can still find their way back.

The "still needed" tally quotes the same words, read out of the label itself,
so renaming a field renames it in both places.

### Choosing directories

**Choose…** beside the two path fields opens a native folder chooser, starting
from wherever the field already points — or its parent, when an output
directory has not been created yet. It is driven from Rust, because the dialog
plugin's JavaScript side ships as an npm package and this app has no bundler.

The second path is a **cache**, and the label says so: one directory per state
and version, shared by every run, a state fetched once and reused. Deleting it
costs only the time to download again, and the Downloads tab shows what is in
it.

### Advanced

Four settings sit under a collapsed *Advanced (uncommon)*: steps between
stopping points, worker threads, steps per unit of threaded work, and the
centre of the balance window.

That last one deserves its place. Districts divide the whole state between
them, so their average population is *necessarily* total ÷ districts and
nothing can change it — what the setting actually moves is the number the
tolerance is measured from, making the accepted range lopsided about the
average that will occur anyway. It is reachable because every rustrecom
parameter is, not because a run wants it.

### Stopping, and the segment size

The window starts **steps between stopping points** at 500, so Stop works
without anyone going looking for it. The CLI keeps its own default of none:
changing that would alter every ensemble it has ever produced.

How long a step takes depends on how many precincts a merged pair spans, so
**fewer districts run slower, not faster**:

| | steps/s | 500 steps |
|---|---|---|
| Michigan, 4 districts | 1,176 | 0.43s |
| Illinois, 17 districts | 1,712 | 0.29s |
| Michigan, 13 districts | 3,425 | 0.15s |

`reversible` is the reason for not choosing a larger number: it rejects most
proposals and its rate depends on the balance bound rather than the state —
at 13 districts it did not finish three thousand steps in two minutes. No
fixed segment can bound stopping there, but a small one keeps it to seconds
rather than minutes.

Chopping the chain is free. The same 50,000 steps took 42.0s in segments of
5000 and 42.3s whole; a thousand segments of 50 took 14.7s against 14.6s
unsegmented. So the only thing a smaller segment costs is the arithmetic.

It is a fixed number rather than one timed on the machine, deliberately. The
segment length changes which plans come out, so a measured value would make
the same settings produce different ensembles on different computers. A
constant travels with the settings file.

On a machine slower than an M1 Pro these times scale up proportionally.

### What a run keeps

The window ticks every intermediate by default, where the CLI keeps none
unless asked. A command line is run in a loop by someone who knows what they
want; a window is used once by someone who will not think to ask until it is
too late. The plans especially cannot be recovered afterwards — they are what
an extension carries on from.

That is affordable because they are compressed: a 20,000-step run keeping
everything went from 32.4 MB to 2.5 MB. See the CLI README for why xz and not
brotli — it comes down to `lzma` being in the Python standard library.

### Help on every question

Every field carries a **?** that opens a short explanation beneath it: what
the setting is for, and what goes wrong if it is set badly. They are written
for people who know redistricting and not Markov chains.

The text lives in one table in the page rather than in the markup, and the
window checks at startup that every field it collects has an entry — a field
added without an explanation is reported rather than left silently bare.

### Input widths

A box is sized to what belongs in it — a four-digit seed does not get the same
width as a filesystem path. Sizes are in `ch`, so they track the font rather
than fighting it.

### Explanations

Each pane has a *What these mean* disclosure, collapsed by default: what a
cycle is, why there are two population tolerances, what the variants differ
in, what gets written where.

One thing those say plainly: the suggested values are **conventions, not
recommendations from rdapy**. rdapy scores plans and does not generate them,
so it has nothing to say about chain parameters — its only `EPSILON` is a
float-comparison tolerance. Where a number is offered it is as placeholder
text, never pre-filled, because a value the tool invented and the user did not
choose is exactly what this project avoids.

A check worth knowing about: the page declares its fields once, in a list, and
compares that list against what the Rust side sends at startup. A setting
added to the library and not to the form is reported in the window rather than
silently dropped.

## Settings files

The point of the top panel. **Download settings** writes every decision to
JSON; **Load settings** reads one back.

The file holds no paths. Not the GeoJSON, not the cache, not the output
directory — those differ between machines, and a file carrying them would not
survive being emailed. What it carries instead is the state and the DRA
version, which is enough for anyone to fetch the same data.

Three things make this work in practice:

- **Every run writes one.** `settings.json` lands beside `manifest.json`
  whether or not anyone asked, because the person who wants it is usually not
  the person who ran the job.
- **A `manifest.json` loads too.** That is what people actually have when they
  want to repeat a colleague's run. Same button; the two are told apart by
  their shape.
- **The CLI reads the same file.** `rda-ensemble replay --settings
  whatever.json --out DIR` reproduces it, so a file from the window works for
  someone who never opens one.

Verified both directions: a run, then a replay from its `settings.json` and
another from its `manifest.json`, give byte-identical `scores.csv`,
`plans.jsonl` and `by_district.jsonl`.

The panel also lists anything that would stop someone else reproducing the
run — no pinned DRA version, `threads` above 1, datasets left to `cycle`
rather than named. Those are warnings, not errors: the run is still a run.

## Extending an ensemble

The **Extend an ensemble** panel carries an earlier run's chain on for
longer. Point it at that run's directory, say where the longer one goes, and
how many more plans.

Its settings come from the earlier run's own directory, not from the form —
extending continues one chain, and changing its parameters halfway would make
it a different one. Choosing the directory fills the form in with what will
actually be used, so what you see is what you get.

If that run was segmented, the result is **byte-identical** to one run of the
full length. If it was not, the chain is still continued — valid, since ReCom
is Markov — but the window says it will not match. Either way the original
directory is left alone.

## What has been downloaded

A collapsed panel under **Output**, because it is not part of building an
ensemble and most of the time it does not matter. When it does: one directory
per state *and* version, so pinning an older version never evicts a newer
one, and a full sweep is about 780 MB. Forget one package, everything, or
just the index of what DRA publishes.

## Repeating someone else's run

There is no separate button for this. **Load settings** takes their
`settings.json` or their `manifest.json`, **Run** runs it — which is what the
CLI's `replay` does.

## Stopping a run

The **Stop** button needs somewhere to stop. rustrecom runs a chain to
completion and offers no way out from inside, so a run can only be interrupted
between segments — which means **Segment steps** has to be set, and the button
is disabled when it is not. The window says so before you start rather than
after you want to stop.

Setting it changes the ensemble: each segment draws its own derived seed. It
is recorded in the settings file, so a reproduction keeps it.

## What is not here yet

- No native file picker. Output and cache are text fields, and settings move
  through the browser's own download and file input. Adding
  `tauri-plugin-dialog` is the obvious next step.
- No chart of the scores. The window tells you the ensemble is written and
  whether it converged; reading it is still someone else's job.
- No `fetch` button. Downloading happens when it is needed — reading a state's
  datasets, or running — rather than as a step of its own.
- No progress bar under `--chains` above 1, for the same reason the CLI has
  none: several chains report at once.
