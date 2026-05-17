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

1. **Grayscale & text body** — both frames are converted to grayscale once (shared across all stages). The main text-body column is detected via Otsu thresholding and ink-density analysis to exclude sidebars, UI noise, and the scrollbar margin (rightmost ~1.2%, capped at 24 px).
2. **Feature maps** — Sobel gradient filtering is applied so matching focuses on edges (text boundaries, UI borders) rather than flat color fields. If the frame lacks texture, the raw grayscale image is used as a fallback.
3. **Parallel template matching** — 5 template heights (derived from multiplicative factors `[1,2,3,5,8]` × min overlap) are extracted from the bottom of the previous frame and slid across the top of the current frame using normalized cross-correlation; all heights run in parallel via rayon.
4. **Multi-bias ranking** — candidates are scored by correlation. When an expected overlap from recent history is available, scores are biased toward the historical value (50 % weight on proximity).
5. **Validation** — the best candidate must pass: a minimum correlation threshold (0.75), a local confidence margin (≥0.005 over the next-best alternative at the same y), a global margin (≥0.002 over any alternative more than 4 px away), and a sampled pixel-difference check (mean delta ≤ 15).
6. **Sub-pixel refinement** — the peak y coordinate is refined via parabolic interpolation of its neighbors.
7. **Temporal smoothing** — outlier overlaps (>3 px from the median of the last 3 frames) are replaced with the median before stitching.
8. **Stagnation detection** — if two consecutive frames are nearly identical (mean pixel delta ≤ 2.0 under a 2×2 sample step), the page bottom is assumed reached and capture stops.
9. **History-based estimation** — when overlap detection fails (e.g., during a page transition), the median of the last 10 measured overlaps is used as a fallback.

## License

MIT
