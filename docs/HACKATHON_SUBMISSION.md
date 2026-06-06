# Hackathon Submission Description

## Project Name

FocusFlow

## Short Description

FocusFlow is a desktop screen recorder that automatically turns raw screen captures into focused demo videos by zooming into clicks and following drag interactions.

## Problem

Hackathon demos and product walkthroughs often require manual editing to highlight the part of the screen that matters. Even a simple demo can take extra time in video editors just to add zooms, pans, and click emphasis.

## Solution

FocusFlow records the screen and tracks user interactions at the same time. After recording, it generates a timeline from clicks and drags, then uses FFmpeg to export an edited MP4 with automatic zoom, smooth camera movement, and click indicators.

## What Works

- Primary monitor screen recording
- Click and drag tracking
- Session folder storage
- Timeline generation
- Automatic zoom and pan export
- Click indicators
- Export progress
- FFmpeg sidecar in the MSI build
- Dark desktop UI with countdown and recording status

## Tech Stack

- Tauri 2
- Rust
- React
- TypeScript
- Vite
- FFmpeg
- Windows APIs

## Why It Matters

FocusFlow compresses the path from "I recorded my workflow" to "I have a polished demo video." It removes repetitive editing work and lets builders create clearer demos faster.

## Future Work

- Audio capture
- Manual timeline editing
- Annotation tools
- Export presets
- Share workflow

