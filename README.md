# Random Karma

A browser-based Yew application for creating randomized car/lap groups near a target total time. The application is published as a GitHub Pages project site at **`/random-karma/`**; this is the single public base path configured in `Trunk.toml`.

## Requirements

- Rust `1.97.0` (managed by `rust-toolchain.toml`)
- The `wasm32-unknown-unknown` target, `rustfmt`, and Clippy (installed automatically by the toolchain file)
- [Trunk `0.21.14`](https://trunkrs.dev/)

Install Trunk reproducibly:

```sh
cargo install trunk --version 0.21.14 --locked
```

## Run locally

```sh
trunk serve
```

Open the local URL printed by Trunk. For a production-equivalent build, run:

```sh
trunk build --release
```

The generated site is written to `dist/` (ignored by Git). `Trunk.toml` owns the release setting, asset hashing, `wasm-opt` `version_125` pin, and the Pages public path. Do not pass a different `--public-url` in CI or release commands.

## Data, privacy, and network behavior

Car data, results, and calculation caches stay in browser memory for the active page session; the application does not send them to an application backend or persist them in browser storage.

The **Paste Car Data from Clipboard** button requests browser permission to read text from the clipboard only after it is clicked. **Copy Results as CSV** writes generated results to the clipboard only after it is clicked.

At page load the browser requests two third-party presentation assets: Google Fonts and Chart.js `4.4.9` from jsDelivr. Chart.js is version-pinned and protected by a SHA-384 Subresource Integrity check in `index.html`. The application itself makes no API, analytics, or telemetry requests.

## CSV input schema

Paste CSV records with no required header. Each accepted row requires at least two columns:

```csv
car-id,lap-time
GT3-01,1:42
GT3-02,1m 43s
GT3-03,103s
```

- Column 1 is a non-empty car identifier; duplicate identifiers are skipped after the first occurrence.
- Column 2 is a lap time accepted as `M:SS`, `Mm SSs`, or `SSs`.
- Invalid or incomplete rows are skipped. Additional columns are ignored.

## Validate, build, and deploy

Run the same main checks locally:

```sh
cargo fmt --all -- --check
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
cargo check --target wasm32-unknown-unknown --all-targets
trunk build --release
```

`.github/workflows/main.yml` validates pushes and pull requests to `master`; it has no deployment permissions. `.github/workflows/deploy.yml` is a separate workflow that builds and deploys only pushes to `master` (or a manual dispatch). Pull requests never deploy. GitHub Pages must be configured in the repository to use **GitHub Actions** as its source.
