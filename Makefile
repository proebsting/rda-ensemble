# Developer tasks. Installing from scratch is in the README and stays there
# as commands you can read -- this is for the things you do repeatedly once
# you have a checkout.
#
# Written for the make that ships with Apple's command line tools, which is
# GNU Make 3.81, so nothing here uses .ONESHELL or anything else from 3.82
# onward. Each recipe line runs in its own shell, which is why every one of
# them begins with its own `cd`.

CLI  := cli
GUI  := gui/src-tauri
APPS := $(HOME)/Applications
BUNDLE := $(GUI)/target/release/bundle/macos/rda-ensemble.app

.DEFAULT_GOAL := help

.PHONY: help
help:
	@echo 'Developer tasks. Run `make <target>`.'
	@echo
	@echo '  deps         clone the three sibling repositories this builds against'
	@echo
	@echo '  build        build both the command line tool and the window'
	@echo '  test         run the test suite'
	@echo '  lint         clippy, both of them, warnings and all'
	@echo '  check        test and lint together -- what to run before committing'
	@echo
	@echo '  run          build the window and open it, installing nothing'
	@echo '  app          wrap the window up as rda-ensemble.app'
	@echo '  install-app  the same, then copy it into ~/Applications'
	@echo '  install-cli  put `rda-ensemble` on your path'
	@echo
	@echo '  dist         both tools, Intel and Apple Silicon, app and disk image'
	@echo '  clean        delete every build directory'
	@echo
	@echo 'Installing from nothing is in README.md, as commands rather than targets.'

# ---------------------------------------------------------------- building

.PHONY: deps
deps:
	./dist/get-deps.sh

.PHONY: build
build:
	cd $(CLI) && cargo build --release
	cd $(GUI) && cargo build --release

.PHONY: test
test:
	cd $(CLI) && cargo test --release

# --all-targets so tests and examples are linted too, not just the library.
.PHONY: lint
lint:
	cd $(CLI) && cargo clippy --release --all-targets -- -D warnings
	cd $(GUI) && cargo clippy --release -- -D warnings

.PHONY: check
check: test lint

# ---------------------------------------------------------------- the window

.PHONY: run
run:
	cd $(GUI) && cargo run --release

# Checked here rather than left to fail as "no such subcommand: tauri",
# which does not say what to install.
.PHONY: app
app:
	@cargo tauri --version >/dev/null 2>&1 || \
	  { echo 'This needs the Tauri CLI:  cargo install tauri-cli' >&2; exit 1; }
	cd $(GUI) && cargo tauri build --bundles app

.PHONY: install-app
install-app: app
	mkdir -p $(APPS)
	rm -rf $(APPS)/rda-ensemble.app
	cp -R $(BUNDLE) $(APPS)/
	@echo
	@echo "Installed in $(APPS). Open it from Launchpad or with command-space."

# ---------------------------------------------------------------- the tool

.PHONY: install-cli
install-cli:
	cd $(CLI) && cargo install --path .

# ---------------------------------------------------------------- packaging

.PHONY: dist
dist:
	./dist/build-macos.sh

# ---------------------------------------------------------------- tidying

.PHONY: clean
clean:
	rm -rf $(CLI)/target $(GUI)/target $(GUI)/gen dist/out
