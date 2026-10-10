# rda-ensemble

Build a redistricting ensemble from Dave's Redistricting data and score every
plan — from a terminal, or from a window.

```
cli/    the command line tool          (see cli/README.md)
gui/    the same thing, in a window    (see gui/README.md)
dist/   packaging, for later
```

Both are built from source. Nothing here is distributed as a ready-made
application at the moment.

## What you need installed

Starting from a Mac that has never been used for development, in this order.
All of it happens in **Terminal** — ⌘-Space, type `terminal`, Return.

### 1. Apple's command line tools

```sh
xcode-select --install
```

A dialog appears; accept it. This takes several minutes and installs both
`git` and the C compiler. The compiler is not optional: ensembles are
compressed with xz, and the `xz2` crate builds liblzma from C source. Without
it the build fails with a `cc`-not-found error mentioning neither xz nor
compression.

On Linux this is `sudo apt install build-essential git` or your distribution's
equivalent.

### 2. Rust

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Accept the default installation. Then either open a new Terminal window or:

```sh
source "$HOME/.cargo/env"
```

Install it through rustup rather than any other way: `rust-toolchain.toml`
here asks rustup for current stable, which is how an out-of-date toolchain
repairs itself. The code needs 1.88 or newer. If you ever see
`icu_collections` or `icu_properties` "requires rustc 1.88", that is an old
toolchain and nothing else — the message names a dependency rather than the
cause, and `rustup update` fixes it.

Check both steps worked:

```sh
cargo --version && cc --version && git --version
```

### 3. Nothing else

Building and running both the command line tool and the window needs only the
two steps above. `dist/` holds packaging machinery that is not currently in
use; if you ever run it, it will tell you what it wants.

### How long the build takes

The first `cargo build --release` compiles several hundred dependencies and
takes five to fifteen minutes depending on the machine. It is not stuck.
Later builds take seconds.

## Building it

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
./rda-ensemble/dist/get-deps.sh
```

`get-deps.sh` clones the other three beside it and checks the result. Or do it
by hand:

```sh
git clone https://github.com/proebsting/rdarust.git
git clone https://github.com/proebsting/partigraph-rust.git
git clone https://github.com/proebsting/rustrecom.git
```

Then build whichever you want:

```sh
cd rda-ensemble/cli && cargo build --release
```

```sh
cd rda-ensemble/gui/src-tauri && cargo build --release
```

The binaries land in each one's `target/release/`. They link nothing but the
operating system's own libraries, so either can be copied to another machine
of the same platform and run as-is.

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

**`dist/`** holds packaging machinery, not currently in use. An unsigned
application runs into Gatekeeper and into institutional device policy, which
is a worse problem than it sounds, so the only supported route today is
building from source.

## The other repositories

| | |
|---|---|
| [rdarust](https://github.com/proebsting/rdarust) | the analytics: a port of [dra2020/rdapy](https://github.com/dra2020/rdapy), scoring byte-identically |
| [partigraph-rust](https://github.com/proebsting/partigraph-rust) | balanced partitioning, for the plan a chain starts from |
| [rustrecom](https://github.com/proebsting/rustrecom) | a fork of MGGG's ReCom implementation |

## Licence

MIT. See `LICENSE`.
