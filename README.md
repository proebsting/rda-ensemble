# rda-ensemble

Build a redistricting ensemble from Dave's Redistricting data and score every
plan — from a terminal, or from a window.

```
cli/    the command line tool          (see cli/README.md)
gui/    the same thing, in a window    (see gui/README.md)
dist/   building something to hand to someone else
```

This repository is **private**. You are reading it because you were given
access.

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
git clone git@github.com:proebsting/rda-ensemble.git
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

## Rust

1.88 or newer. The `rust-toolchain.toml` here asks rustup for current stable,
so an up-to-date install needs nothing.

If you see errors about `icu_collections` or `icu_properties` "requires rustc
1.88", that is this and nothing else — the message names the dependency rather
than the cause. `rustup update` fixes it.

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

**`dist/`** builds a universal macOS app and disk image. `dist/DISTRIBUTING.md`
is frank about what Gatekeeper will do to an unsigned one.

## The other repositories

| | |
|---|---|
| [rdarust](https://github.com/proebsting/rdarust) | the analytics: a port of [dra2020/rdapy](https://github.com/dra2020/rdapy), scoring byte-identically |
| [partigraph-rust](https://github.com/proebsting/partigraph-rust) | balanced partitioning, for the plan a chain starts from |
| [rustrecom](https://github.com/proebsting/rustrecom) | a fork of MGGG's ReCom implementation |

## Licence

MIT. See `LICENSE`.
