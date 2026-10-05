# Shi — A Rust-based 2D CAD application

Shi is a desktop 2D CAD application written in Rust: the UI is built on [egui](https://github.com/emilk/egui)/[eframe](https://github.com/emilk/eframe),
and the geometry kernel, rendering, and file I/O are all provided by the in-house **Cadrs** SDK in the same repository (the `sdk/` directory), with no dependency on external CAD kernels such as OpenCascade.

```
┌──────────────────────────┐        ┌────────────────────────────────────────┐
│  src/  Shi desktop app   │ deps → │  sdk/  Cadrs CAD kernel (pure Rust)    │
│  app.rs   canvas/tool/UI │        │  geometry  primitives & operations     │
│  i18n.rs  EN/zh UI       │        │  data_structure  doc / entities / lay. │
│  layers.rs layer helpers │        │  dimension linear/R/Ø/angular dims     │
│  main.rs  entry point    │        │  render  tessellation & rendering      │
└──────────────────────────┘        │  io      DXF/SVG/PDF/bitmap import/exp │
                                     └────────────────────────────────────────┘
```

## Feature overview

**Drawing**: line, polyline (closeable), rectangle, circle, arc, point, ellipse, B-spline, solid fill.

**Editing**: select / window select, move, copy, rotate, scale, mirror, delete, undo / redo, keyboard shortcuts.

**Dimensioning**: aligned (linear) dimensions, **radius dimensions (R)**, **diameter dimensions (Ø)**, angular dimensions; dimension text is drawn with a built-in vector stroke font,
so rendering, picking, bounding boxes, and all export formats are supported automatically, with no external fonts required.

**Aids**: infinite canvas (stepless zoom / pan), grid and grid snapping, object snap (endpoint / midpoint / center / intersection),
multiple layers (color / visibility / lock), length and area measurement, automatic English/Chinese UI switching.

**Files**: open DXF / DWG / SVG / JSON; export CAD drawings, vector graphics, bitmaps (PNG/BMP/JPEG/WebP), EPS, PDF, and WMF.

## Quick start

```bash
# Run the app (debug build by default; dependencies are precompiled with opt-level=2, so the UI stays smooth)
cargo run

# Release build
cargo run --release
```

Requirements: Rust 1.85+ (edition 2024) on a Windows / Linux / macOS desktop.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` | New / Open / Save |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo |
| `Delete` | Delete selected entities |
| `Esc` | Cancel the current drawing |
| `F` | Fit view (zoom to everything) |
| Arrow keys / wheel | Pan / zoom around the cursor |
| Middle-button drag (or left drag with a non-select tool) | Pan the canvas |
| Double-click / `Enter` / right-click | Finish a polyline or spline |
| Right-click | Context menu |

## Using radius / diameter dimensions

1. Select "Radius Dimension" or "Diameter Dimension" in the toolbar.
2. **Click a circle or arc** on the canvas (8px pick tolerance; press `Esc` to cancel).
   Once selected, the status bar shows "circle/arc selected…", and a direction guide line appears at the cursor.
3. **Move the cursor to set the dimension direction**, then click once to place the dimension text.

> Object snap is temporarily disabled while placing: the dimension direction is determined by the cursor position, and if it gets hijacked by center snapping,
> you will see a "direction cannot be determined" situation. If the cursor is almost on the center, the app prompts "move the cursor outside the circle to determine the dimension direction" and waits to be re-placed.

## Project structure

```
.
├── Cargo.toml          app crate (sh i)
├── src/                desktop application
│   ├── app.rs          canvas, tool state machine, rendering, file dialogs
│   ├── i18n.rs         English/Chinese strings
│   ├── layers.rs       layer read/write helpers
│   └── main.rs         entry point
├── sdk/                Cadrs CAD SDK (standalone crate, usable on its own)
│   ├── src/            geometry / data model / rendering / IO / dimensioning …
│   ├── tests/          end-to-end integration tests
│   ├── README.md       SDK usage guide
│   └── API.md          SDK API reference
└── tools/              documentation completeness scripts (Python)
```

## Tests

```bash
# App tests (dimension placement, picking, coordinate transforms, snap strategies)
cargo test

# All SDK tests
cargo test --manifest-path sdk/Cargo.toml

# Only the dimension-specific tests (radius / diameter / linear / angle)
cargo test --manifest-path sdk/Cargo.toml dimension
cargo test --manifest-path sdk/Cargo.toml --test dimension_integration
```

## Documentation

```bash
# Generate and open the SDK API docs
cargo doc --manifest-path sdk/Cargo.toml --no-deps --open

# Report documentation completeness (count of undocumented public items)
python tools/doc_lint.py
python tools/doc_report.py
```

## License

The Shi application and its SDK (Cadrs) are both released under the Apache-2.0 / MIT licenses; see `sdk/LICENSE`.
