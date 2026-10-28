#!/usr/bin/env bash
# Packs Burli with native libraries: package.sh OUTPUT_DIR RID=LIBRARY...
set -euo pipefail

if (($# < 2)); then
    echo "usage: $0 OUTPUT_DIR RID=LIBRARY [RID=LIBRARY ...]" >&2
    exit 2
fi

project="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/Burli.csproj"
output_dir=$1
shift
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

for asset in "$@"; do
    rid=${asset%%=*}
    library=${asset#*=}
    if [[ "$rid" == "$asset" || -z "$rid" || -z "$library" ]]; then
        echo "invalid native asset: $asset" >&2
        exit 2
    fi
    mkdir -p "$stage/$rid"
    cp "$library" "$stage/$rid/"
done

dotnet pack "$project" --configuration Release --output "$output_dir" -p:NativeAssetRoot="$stage"
