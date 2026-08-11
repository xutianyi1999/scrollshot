# Scrollshot

> Capture vertical long screenshots on Windows — scroll, capture, stitch.

[![Build](https://github.com/xutianyi1999/scrollshot/actions/workflows/release.yml/badge.svg)](https://github.com/xutianyi1999/scrollshot/actions/workflows/release.yml)

[中文](./README.zh-CN.md)

Scrollshot is a CLI tool that lets you capture a scrolling (long) screenshot on Windows. You select a region on screen, and Scrollshot automatically scrolls downward while capturing frames, then stitches them into a single tall PNG image.

## How It Works

1. **Select region** — a translucent overlay appears; drag to select the area you want to capture, then click inside the selection to confirm.
2. **Auto-scroll & capture** — Scrollshot sends simulated mouse wheel events at the chosen point, captures each frame via `xcap` (or GDI fallback), and detects overlaps between consecutive frames using computer vision (gradient-based template matching with a text-band fast path and full-content fallback).
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
| `--settle-ms <MS>` | Settle delay after each scroll (ms) | `250` |
| `--wheel-notches <N>` | Notches per scroll step (1+) | `3` |

### Controls

| Key | Action |
|-----|--------|
| `Esc` | Cancel region selection or stop capture early |

## Platform Support

**Windows only.** Scrollshot relies on Win32 API for DPI awareness, GDI-based screen capture (fallback), mouse event simulation, and the selection overlay. Requires Windows 10 or later, Rust 1.85+ (edition 2024).

## How Overlap Detection Works

Each captured frame is compared to the previous one to find the exact pixel row where content overlaps. The pipeline:

1. **Grayscale & content bands** — both frames are converted to grayscale once (shared across all stages). For text-heavy pages, the main text column is detected via Otsu thresholding and ink-density analysis to exclude sidebars, UI noise, and the scrollbar margin (rightmost ~1.2%, capped at 24 px). If that band cannot verify a seam, the full content width is retried, preserving evidence from images, tables, code blocks, and mixed layouts.
2. **Feature maps** — Sobel gradient filtering is applied so matching focuses on edges (text boundaries, image details, and UI borders) rather than flat color fields. Both frames use the same fixed gradient scale, so their comparable pixels retain comparable feature values even when the visible content mix changes. If the frame lacks texture, the raw grayscale image is used as a fallback.
3. **Parallel template matching** — 5 template heights (derived from multiplicative factors `[1,2,3,5,8]` × min overlap) are extracted from the bottom of the previous frame and slid across the top of the current frame using normalized cross-correlation; all heights run in parallel via rayon.
4. **Coarse-to-fine ranking** — a downscaled full-range match narrows the high-resolution search. Previous scroll distances never narrow or bias the search; if the fast match fails validation, the full range is retried.
5. **Validation** — the best candidate must pass: a minimum correlation threshold (0.75), a local confidence margin (≥0.005 over the next-best alternative at the same y), a global margin (≥0.002 over any alternative more than 4 px away), a whole-overlap Sobel feature difference check, and a sampled pixel-difference check (mean delta ≤ 15).
6. **Safe retry & recovery** — if no candidate passes validation, the same position is given one extra settle-and-capture attempt before the frame is discarded. Later retries temporarily use a one-notch scroll and doubled settle delay, allowing a dynamic page to stabilize without guessing a seam. The tool allows up to 10 such recoveries before saving the verified portion.
7. **Stagnation detection** — if two consecutive frames are nearly identical (mean pixel delta ≤ 2.0 under a 2×2 sample step), the page bottom is assumed reached and capture stops.

## License

MIT
