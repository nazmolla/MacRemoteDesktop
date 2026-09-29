#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
swiftc -O -swift-version 5 -import-objc-header Bridging.h main.swift -o probe \
  -framework AppKit -framework ScreenCaptureKit -framework CoreGraphics
echo "built $(pwd)/probe"
