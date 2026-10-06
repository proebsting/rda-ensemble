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

The state menu comes from DRA's index; the dataset menus come from the chosen
state's GeoJSON, which is why they are empty until **Read this state's
datasets**. Required fields are starred.

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
