#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/target"
swiftc -O -swift-version 5 "$root/tools/workload/main.swift" -o "$root/target/workload"
echo "built $root/target/workload"
