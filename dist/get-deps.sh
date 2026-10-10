#!/bin/sh
# Clone the three repositories this one builds against, beside it.
#
#   ./rda-ensemble/dist/get-deps.sh
#
# Run it from wherever you want the four directories to live, or from
# anywhere -- it works out where rda-ensemble is and puts the others next to
# it. Already-cloned ones are left alone, so running it twice is harmless.
set -eu

# The parent of the repository this script lives in.
HERE=$(cd "$(dirname "$0")/.." && pwd)
PARENT=$(dirname "$HERE")

echo "Putting dependencies in $PARENT"
echo

for repo in rdarust partigraph-rust rustrecom; do
    if [ -d "$PARENT/$repo" ]; then
        echo "  $repo already there, leaving it"
        continue
    fi
    echo "  cloning $repo"
    git -C "$PARENT" clone --quiet "https://github.com/proebsting/$repo.git"
done

echo
missing=
for repo in rdarust partigraph-rust rustrecom; do
    # Each has to be a real checkout, not an empty directory someone made.
    if [ -d "$PARENT/$repo/.git" ]; then
        printf '  %-18s ok\n' "$repo"
    else
        printf '  %-18s MISSING\n' "$repo"
        missing=yes
    fi
done

if [ -n "$missing" ]; then
    echo
    echo "Something did not clone. The four directories have to be siblings:"
    echo
    echo "    $PARENT/"
    echo "      rda-ensemble/   rdarust/   partigraph-rust/   rustrecom/"
    exit 1
fi

echo
echo 'Ready. The next step is the README section "Install the window as an'
echo 'application", here:'
echo
echo "    $HERE/README.md"
echo
echo 'It ends with the application in ~/Applications. Nothing needs building by'
echo 'hand first -- that section compiles the window on the way. The command'
echo 'line tool is separate and optional, in the section after it.'
echo
echo 'Once something is installed, this lists the tasks that come up repeatedly:'
echo
echo "    cd $HERE && make"
