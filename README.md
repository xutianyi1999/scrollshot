# Scrollshot

> Capture vertical long screenshots on Windows — scroll, capture, stitch.

[中文](./README.zh-CN.md)

Scrollshot is a CLI tool that lets you capture a scrolling (long) screenshot on Windows. You select a region on screen, and Scrollshot automatically scrolls downward while capturing frames, then stitches them into a single tall PNG image.

## How It Works

1. **Select region** — a translucent overlay appears; drag to select the area you want to capture, then click inside the selection to confirm.
2. **Auto-scroll & capture** — Scrollshot sends simulated mouse wheel events at the chosen point, captures each frame via `xcap` (or GDI fallback), and detects overlaps between consecutive frames using computer vision (gradient-based template matching with text body band detection).
3. **Stitch** — overlapping regions are removed and frames are assembled into one continuous image.

## Installation

```bash
cargo install scrollshot
```

Or build from source:

```bash
git clone https://github.com/your-username/scrollshot
cd scrollshot
cargo build --release
```

The binary will be at `target/release/scrollshot.exe`.

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
| `--settle-ms <MS>` | Settle delay after each scroll (ms) | `100` |
| `--wheel-notches <N>` | Notches per scroll step (1+) | `4` |

### Controls

| Key | Action |
|-----|--------|
| `Esc` | Cancel region selection or stop capture early |

## Platform Support

**Windows only.** Scrollshot relies on Win32 API for DPI awareness, GDI-based screen capture (fallback), mouse event simulation, and the selection overlay. It requires Windows 10 or later with DPI virtualization compatible.

## How Overlap Detection Works

Each captured frame is compared to the previous one to find the exact pixel row where content overlaps. The algorithm:

1. Converts both frames to grayscale, then applies Sobel gradient filtering so matching focuses on edges (text boundaries, UI borders) rather than flat color fields.
2. Detects the main text-body column to avoid sidebar/UI noise.
3. Slides a template (bottom of previous frame) across the top of the current frame using normalized cross-correlation.
4. Validates the best match with local/global confidence margins and multi-band voting.
5. If overlap detection fails several times consecutively, the capture loop stops with a warning.

## License

MIT
