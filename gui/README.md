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

Every decision a run makes — all 28 of them — on one page, grouped as the
`--help` is: data, datasets, starting plan, chain, output. Nothing is hidden
behind an "advanced" disclosure yet. That is deliberate for now: it is easier
to learn which settings nobody touches by watching than by guessing.

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
- No progress bar under `--chains` above 1, for the same reason the CLI has
  none: several chains report at once.
