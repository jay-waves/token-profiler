#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
mkdir -p bin dist/token-profiler/bin dist/.agents/plugins
cp target/release/token-profiler bin/token-profiler
cp target/release/token-profiler dist/token-profiler/bin/token-profiler
cp plugin.json LICENSE README.md dist/token-profiler/
mkdir -p dist/token-profiler/assets
cp assets/DroidSans-LICENSE.txt dist/token-profiler/assets/
cp -R hooks skills dist/token-profiler/
cat > dist/.agents/plugins/marketplace.json <<'JSON'
{
  "name": "token-profiler-local",
  "plugins": [{
    "name": "token-profiler",
    "source": {"source": "local", "path": "./token-profiler"},
    "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
    "category": "Productivity"
  }]
}
JSON
printf 'Built local plugin and marketplace: %s/dist\n' "$PWD"
