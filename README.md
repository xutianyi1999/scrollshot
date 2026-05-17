# Scrollshot

> Capture vertical long screenshots on Windows — scroll, capture, stitch.

[![Build](https://github.com/xutianyi1999/scrollshot/actions/workflows/release.yml/badge.svg)](https://github.com/xutianyi1999/scrollshot/actions/workflows/release.yml)

[中文](./README.zh-CN.md)

Scrollshot is a CLI tool that lets you capture a scrolling (long) screenshot on Windows. You select a region on screen, and Scrollshot automatically scrolls downward while capturing frames, then stitches them into a single tall PNG image.

## How It Works

1. **Select region** — a translucent overlay appears; drag to select the area you want to capture, then click inside the selection to confirm.
2. **Auto-scroll & capture** — Scrollshot sends simulated mouse wheel events at the chosen point, captures each frame via `xcap` (or GDI fallback), and detects overlaps between consecutive frames using computer vision (gradient-based template matching with text body band detection).
3. **Stitch** — overlapping regions are removed and frames are assembled into one continuous image.

## Install

```bash
cargo install --git https://github.com/xutianyi1999/scrollshot
```

Or build from source:

```bash
git clone https://github.com/xutianyi1999/scrollshot
cd scrollshot
cargo install --path .
```

## Usage

```bash
scrollshot --output longshot.png
```

1. A full-screen overlay appears. **Drag with the left mouse button** to select the capture region.
2. Release the mouse button. **Click inside the selected area** to begin capture.
3. Scrollshot scrolls downward and captures frames automatically.
4. Press **Esc** at any time to stop early — captured frames are still saved.
5. The final stitched image is written to `--output` (default: `scrollshot.png`).

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--output <PATH>` | Output PNG path | `scrollshot.png` |
| `--max-scrolls <N>` | Maximum scroll steps | `8000` |
| `--settle-ms <MS>` | Settle delay after each scroll (ms) | `200` |
| `--wheel-notches <N>` | Notches per scroll step (1+) | `4` |

### Controls

| Key | Action |
|-----|--------|
| `Esc` | Cancel region selection or stop capture early |

## Platform Support

**Windows only.** Scrollshot relies on Win32 API for DPI awareness, GDI-based screen capture (fallback), mouse event simulation, and the selection overlay. Requires Windows 10 or later, Rust 1.85+ (edition 2024).

## How Overlap Detection Works

Each captured frame is compared to the previous one to find the exact pixel row where content overlaps. The pipeline:

1. **Graveyard & text body** — converts both frames to grayscale once (shared across stages), detects the main text-body column to exclude sidebar/UI noise and scrollbar margin.
2. **Feature maps** — applies Sobel gradient filtering so matching focuses on edges (text boundaries, UI borders) rather than flat color fields.
3. **Parallel template matching** — slides templates of 7 different heights from the bottom of the previous frame across the top of the current frame using normalized cross-correlation; all 7 heights run in parallel via rayon.
4. **Multi-scale refinement** — re-matches at half resolution for a coarse estimate, then refines at full resolution in a narrow search window.
5. **Validation** — verifies the best candidate with Sum-of-Squared-Errors; checks local/global confidence margins and multi-band voting (5 non-overlapping bands in parallel).
6. **Temporal smoothing** — suppresses single-frame outlier overlaps via median filtering before stitching.

## License

MIT
