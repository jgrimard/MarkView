# Third-party notices

MarkView includes or links to the following open-source software. Each component remains under its own license.

## Bundled in the executable (`ui/`)

| Component | Version | License | Notes |
|---|---|---|---|
| [CodeMirror 6](https://codemirror.net/) | 6.x | MIT | Bundled into `ui/editor.bundle.js` |
| [Mermaid](https://mermaid.js.org/) | 11.4.1 | MIT | `ui/vendor/mermaid/LICENSE` |
| [KaTeX](https://katex.org/) | 0.16.11 | MIT | `ui/vendor/katex/LICENSE` |
| [highlight.js](https://highlightjs.org/) | 11.11.1 | BSD-3-Clause | `ui/vendor/highlightjs/LICENSE` |
| [github-markdown-css](https://github.com/sindresorhus/github-markdown-css) | 5.8.1 | MIT | `ui/vendor/github-markdown-css/license` |

## Rust crates (selected)

| Crate | License |
|---|---|
| [tauri](https://tauri.app/) | Apache-2.0 OR MIT |
| [comrak](https://github.com/kivikakk/comrak) | BSD-2-Clause |
| [notify](https://github.com/notify-rs/notify) | CC0-1.0 |
| [rfd](https://github.com/PolyMeilex/rfd) | MIT |
| [window-vibrancy](https://github.com/tauri-apps/window-vibrancy) | Apache-2.0 OR MIT |
| [webview2-com](https://github.com/wravery/webview2-rs) | MIT |
| [winreg](https://github.com/gentoo90/winreg-rs) | MIT |

The full dependency tree is in `src-tauri/Cargo.lock`. Run `cargo tree` to inspect it.

## Build tools

[esbuild](https://esbuild.github.io/) (MIT) bundles the editor. It is a development dependency and is not shipped.
