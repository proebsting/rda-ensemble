# rda-ensemble

Build a redistricting ensemble from Dave's Redistricting data and score every
plan — from a terminal, or from a window.

```
cli/        the command line tool          (see cli/README.md)
gui/        the same thing, in a window    (see gui/README.md)
dist/       wraps the window up as a macOS app, if you want one
Makefile    the tasks you repeat once it is all installed
```

Both are built from source. There is no download — and building your own
turns out to be the better position anyway, since an application you compiled
carries no quarantine flag and so meets none of the Gatekeeper or
device-policy obstacles a downloaded one does.

## What you need installed

Starting from a Mac that has never been used for development, in this order.
All of it happens in **Terminal** — ⌘-Space, type `terminal`, Return.

### 1. Apple's command line tools

```sh
xcode-select --install
```

A dialog appears; accept it, and wait for it to finish before going on. This
installs both `git` and the C compiler. The compiler is not optional:
ensembles are compressed with xz, and the `xz2` crate builds liblzma from C
source. Without it the build fails with a `cc`-not-found error mentioning
neither xz nor compression.

On Linux this is `sudo apt install build-essential git` or your distribution's
equivalent.

### 2. Rust, via rustup

Rust is installed by a tool called **rustup**, which also keeps it up to date.
This is the official installer, from [rustup.rs](https://rustup.rs) — it
fetches rustup and rustup then fetches Rust:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Accept the default installation when it asks. It puts everything under
`~/.cargo` and `~/.rustup`, touching nothing else, and adds `~/.cargo/bin` to
your path. For that to take effect, either open a new Terminal window or:

```sh
source "$HOME/.cargo/env"
```

If piping a downloaded script into a shell makes you uneasy — a reasonable
instinct — [rustup.rs](https://rustup.rs) offers the same thing as a file you
can read first, and `brew install rustup` works too if you already have
Homebrew.

Use rustup rather than any other route. The `rust-toolchain.toml` in this
repository asks rustup for current stable, which is how an out-of-date
toolchain repairs itself; a Rust installed some other way cannot do that. The
code needs 1.88 or newer. If you ever see `icu_collections` or
`icu_properties` "requires rustc 1.88", that is an old toolchain and nothing
else — the message names a dependency rather than the cause, and `rustup
update` fixes it.

### 3. Check both arrived

```sh
cargo --version && cc --version && git --version
```

Three version numbers means you are ready. Worth doing: `xcode-select
--install` can look finished while its dialog is still working, and the
failure then turns up a quarter of an hour into a build as a `cc` error.

## Get the code

Four repositories have to sit beside one another in the same directory. The
dependencies between them are relative paths, so the layout is not optional:

```
somewhere/
├── rda-ensemble/     this repository
├── rdarust/          the analytics, a port of dra2020/rdapy
├── partigraph-rust/  draws the starting plan
└── rustrecom/        runs the Markov chain
```

From whatever directory you want those four to live in:

```sh
git clone https://github.com/proebsting/rda-ensemble.git
```

```sh
./rda-ensemble/dist/get-deps.sh
```

That clones the other three beside it and checks the result. By hand instead,
it is:

```sh
git clone https://github.com/proebsting/rdarust.git
```

```sh
git clone https://github.com/proebsting/partigraph-rust.git
```

```sh
git clone https://github.com/proebsting/rustrecom.git
```

## Install the window as an application

This is the whole path for anyone who wants the window and nothing else. The
command line tool below is a separate thing you can ignore.

One more tool, which turns a built program into a macOS application:

```sh
cargo install tauri-cli
```

Then build it:

```sh
cd rda-ensemble/gui/src-tauri
```

```sh
cargo tauri build --bundles app
```

That first build compiles several hundred dependencies and takes five to
fifteen minutes depending on the machine. It is not stuck.

Put the result where macOS looks for applications:

```sh
mkdir -p ~/Applications
```

```sh
cp -R target/release/bundle/macos/rda-ensemble.app ~/Applications/
```

Open it from Launchpad, or with ⌘-Space and typing its name. It keeps working
after you delete the checkout it was built from.

It simply opens, with nothing to approve: the unidentified-developer warning
macOS shows applies to applications that were **downloaded**, and one you
compiled yourself was not.

`gui/README.md` explains what every setting in the window means. Choosing a
state downloads its data the first time, a few megabytes, and keeps it.

## The command line tool

Optional, and separate from the window — the two do the same job.

```sh
cd rda-ensemble/cli
```

```sh
cargo install --path .
```

`cargo install` builds it and puts it in `~/.cargo/bin`, which rustup already
added to your path, so afterwards `rda-ensemble` works from any directory.

```sh
rda-ensemble --help
```

### A first run

```sh
rda-ensemble states
```

That asks Dave's Redistricting what it publishes and prints a state per line.
Nothing has been downloaded yet.

Now build a small ensemble for North Carolina:

```sh
rda-ensemble run \
  --state NC --plan-type congress --cycle 2020 \
  --elections E_16-20_COMP \
  --seed-tolerance 0.01 --tolerance 0.05 \
  --variant cut-edges-ust --rng-seed 1 \
  --plans 100 --sample-every 10 \
  --out my-first-ensemble
```

The first run downloads that state from DRA — 4.6 MB for North Carolina,
17 MB once unpacked — and keeps it, so later runs start immediately. The
hundred plans themselves take a couple of seconds.

It will finish by telling you the ensemble is too small:

```
  effective N  6 at worst, on population_compactness   (of 100 sampled)
               consecutive plans are correlated; a larger --sample-every helps
```

That is the tool working, not failing. A hundred plans taken ten steps apart
really are too alike to be worth a hundred independent ones, and real work
uses thousands of plans sampled hundreds of steps apart. Being told so is the
point of that section.

When it finishes, `my-first-ensemble/` holds:

| | |
|---|---|
| `scores.csv` | one row per plan, one column per measure — this is the result |
| `by_district.jsonl` | the same plans, district by district |
| `settings.json` | every decision this run made, enough to repeat it exactly |
| `manifest.json` | the same, plus what actually happened |
| `diagnostics.json` | whether the chain ran long enough to be trusted |

`cli/README.md` explains every option, and `cli/WALKTHROUGH.md` follows one
real run from nothing to results, including the mistakes that are easy to
make.

## Working on the code

Everything above is a first-time installation, written out as commands so that
a failure says which step failed. Once it is installed, the tasks that come up
repeatedly live in the `Makefile`. Run `make` with no target to list them:

```sh
make
```

The four that matter:

| | |
|---|---|
| `make run` | build the window and open it, installing nothing |
| `make check` | the tests and clippy — what to run before committing |
| `make install-app` | rebuild the application and replace the one in `~/Applications` |
| `make dist` | both tools for Intel and Apple Silicon, as an app and a disk image |

`make run` is the quick loop while changing the window: it builds and opens it
with nothing installed, so it is gone when you close it. `make install-app` is
the slower one, and is how the copy in `~/Applications` gets updated after a
change.

`make dist` is a different thing again — it builds something to put on another
machine, which is a harder problem than installing one yourself, and
`dist/DISTRIBUTING.md` explains why.

## Why four clones

Honestly, because this is an interim arrangement. The three dependencies could
be git dependencies in `Cargo.toml`, which would make this a single clone and
`cargo build`. That is the intended destination.

It is not there yet because `rustrecom` belongs to someone else — it is
[MGGG's](https://github.com/mggg/rustrecom), and `proebsting/rustrecom` is a
fork pinned so an upstream change cannot break a build mid-analysis. Settling
how that dependency should be expressed is a conversation still to have.

## What these are

**`cli/`** does the whole job in one command: reads the GeoJSON DRA publishes
for a state, builds the precinct adjacency graph, draws a population-balanced
starting plan, runs a ReCom chain, and scores plans as they are produced.
`cli/README.md` covers every option; `cli/WALKTHROUGH.md` follows one real run
from nothing to results.

**`gui/`** is the same library behind a window, for people who would rather not
type a forty-flag command line. Every setting carries an explanation of what it
is for and what goes wrong if it is set badly.

**`dist/`** holds the two scripts the `Makefile` calls out to: `get-deps.sh`,
used above, and `build-macos.sh` behind `make dist`.

## The other repositories

| | |
|---|---|
| [rdarust](https://github.com/proebsting/rdarust) | the analytics: a port of [dra2020/rdapy](https://github.com/dra2020/rdapy), scoring byte-identically |
| [partigraph-rust](https://github.com/proebsting/partigraph-rust) | balanced partitioning, for the plan a chain starts from |
| [rustrecom](https://github.com/proebsting/rustrecom) | a fork of MGGG's ReCom implementation |

## Licence

MIT. See `LICENSE`.
