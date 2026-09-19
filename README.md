# Code Map

Fly over a codebase as a zoomable GPU treemap. A small demo app built with
[Makepad](https://github.com/makepad/makepad), a Rust UI framework that renders everything on
the GPU. Inspired by Rik Arends' "Makepad Scope" demo.

- Every file is a box sized by its line count, grouped by folder. Zoom in far enough and the
  actual code appears inside the box.
- Respects `.gitignore` (the file list comes from `git ls-files`). Ignored entries show up as
  striped collapsed boxes and are only read when you click them.
- Search: type to highlight matches, Enter flies to the next one, Escape clears.
- Git heatmap: color by "Recently changed" or "Most changed" instead of file type.
- 3D city mode: folders become districts, files become towers with their code on the roof.

## Detail level

Use the **Normal / High / Ultra / Custom** dropdown in the toolbar to change rendering detail
without restarting or rescanning. Normal retains the original thresholds and budgets.
High and Ultra lower the screen-space culling thresholds in both 2D and 3D, so
more distant folders, towers and code strips can become visible. They also raise
label and code-panel budgets. Custom exposes geometry detail, text detail and the
rendering budget in the inspector. Changes apply immediately. Rendering still has
finite safety limits, and geometry smaller than a screen pixel may remain visually
indistinguishable. Higher settings can increase CPU work, memory use and label overlap
on large repositories.

## Language

The UI supports English and Simplified Chinese using **Fluent** (`fluent-bundle`).
Translations are external UTF-8 files in `locales/en-US/main.ftl` and
`locales/zh-CN/main.ftl`. Keep the files alongside the distributed executable:

```text
code-map.exe
locales/
  en-US/main.ftl
  zh-CN/main.ftl
```

The app loads the translation catalogs on first use and caches them. Editing an `.ftl`
file takes effect after restarting the app; no recompilation is necessary. For development,
`cargo run` finds `locales/` in the current working directory or the project source tree.
For release, place `locales/` **next to the executable**, or set
`CODE_MAP_LOCALES_DIR` to the complete path of the `locales` directory. If a translation
is missing or invalid, the app logs a diagnostic to stderr and falls back to English;
if the English translation cannot be loaded, missing message IDs are shown as a last resort.

The initial language follows the system language when supported; the toolbar selector
changes it at runtime. `CODE_MAP_LANG=zh-CN` or `--lang=zh-CN` overrides detection
(use `en` for English). Keep translation IDs consistent in both `.ftl` files. The
`src/i18n.rs` adapter retains the existing `Language` API and handles dynamic formatting.

## Setup

You need Rust (https://rustup.rs) and a Makepad checkout **next to** this repo, because
`Cargo.toml` points at `../makepad`:

```sh
mkdir makepad-demo && cd makepad-demo
git clone https://github.com/makepad/makepad.git
git -C makepad checkout a4ea2536a4ab223fb31e0282be923191230f44fe   # dev branch, 2026-09-15
git clone https://github.com/Peeter95/code-map.git
```

Makepad's API moves fast, so stick to the pinned commit above. Newer commits may or may not
build.

## Run

```sh
cd code-map
cargo run --release -- /path/to/some/project          # 2D treemap
cargo run --release -- /path/to/some/project --3d     # start in 3D city mode
```

The first build compiles Makepad and takes a few minutes. Later builds are fast. Without a path
it maps the current folder. It works best on a git repository: the ignore rules then match
`git status` exactly and the heatmap has history to show. Outside git it falls back to a simple
ignore list.

Headless check, no window, just scan statistics:

```sh
cargo run --release --bin scan -- /path/to/some/project [ignored/path ...]
```

## Controls

| Input | Action |
| --- | --- |
| Scroll | Zoom |
| Drag | Pan (2D) or orbit (3D) |
| Shift drag or right drag | Pan (3D) |
| Click | Inspect a file or folder |
| Double click | Fly to it |
| Enter in search | Next match |

## Code tour

| File | What it does |
| --- | --- |
| `src/main.rs` | App shell: window, toolbar and inspector written in Makepad's Splash DSL |
| `src/i18n.rs` | Fluent loading, cached lookup, English fallback, language detection and formatting |
| `locales/*/main.ftl` | Editable English and Simplified Chinese translation resources |
| `src/scan.rs` | Reads the project from disk, asks git which files are ignored, summarizes every line |
| `src/model.rs` | Treemap layout and colors |
| `src/history.rs` | Reads `git log` for the heatmap modes |
| `src/map_view.rs` | The custom `CodeMap` widget: drawing, zoom, picking, search |
| `src/map_view/city.rs` | 3D city rendering (offscreen pass, cubes, projected labels) |
| `src/orbit.rs` | Orbit camera for 3D mode |

Tested on macOS. Makepad also targets Windows, Linux and web, but this app has not been tried
there.
