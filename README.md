# FocusFlow

## Overview

FocusFlow is an open-source Windows desktop application that automatically transforms raw screen recordings into polished tutorial videos.

Built with Rust, Tauri, React, TypeScript, and FFmpeg, FocusFlow records screen activity, tracks user interactions such as clicks and drag movements, automatically generates an editing timeline, and exports videos with intelligent zooming and camera movement.

The goal is to help creators, students, developers, and founders produce professional-looking walkthroughs without spending hours manually editing recordings.

---

## Problem Statement

Creating tutorial videos and product demos is time-consuming.

After recording a screen capture, creators often need to:

* Manually zoom into important interactions
* Follow cursor movements
* Highlight clicks and actions
* Edit timelines in video editing software

Existing tools that automate this process are often:

* Expensive
* Subscription-based
* Closed source
* Not easily accessible to students and indie developers

While learning new technologies and building projects, I faced this problem myself and wanted a free, open-source alternative.

---

## Solution

FocusFlow automatically converts raw screen recordings into focused tutorial videos.

The application:

1. Records the screen.
2. Tracks user interactions such as clicks and drag movements.
3. Generates an interaction timeline.
4. Automatically creates zoom and camera movement effects.
5. Exports a polished MP4 video.

This removes much of the repetitive editing work normally required when creating tutorials and demos.

---

## Features

* Screen recording
* Automatic zoom generation
* Click tracking with coordinates and timestamps
* Drag interaction tracking
* Timeline generation from interactions
* Smooth camera pan and zoom effects
* Click indicators in exported videos
* Session-based recording management
* Export progress tracking
* Recording countdown
* Global hotkeys
* Bundled FFmpeg sidecar
* Windows File Explorer integration
* Modern desktop UI

---

## Tech Stack

### Frontend

* React
* TypeScript
* Vite

### Backend

* Rust
* Tauri 2

### Database

* No database required
* JSON-based session storage

### APIs

* FFmpeg Sidecar
* Windows APIs for interaction tracking and hotkeys

### Hosting

* GitHub Repository
* Windows MSI distribution

---

## Codex / OpenAI Usage

Codex played a significant role throughout development.

Used for:

### Ideation

* Refining the product concept
* Defining the MVP scope
* Feature prioritization

### Architecture Planning

* Designing the Tauri + Rust + React architecture
* Defining session storage structure
* Planning the export pipeline

### Code Generation

* Recorder implementation
* Interaction tracking
* Timeline generation
* FFmpeg export pipeline
* UI improvements

### Debugging

* FFmpeg integration issues
* Windows path handling
* Sidecar configuration
* Export generation problems
* Tauri permissions and packaging

### Testing & Iteration

* Rapid feature prototyping
* Reviewing architecture decisions
* Production build preparation

### Documentation

* README creation
* Release notes
* Pitch deck support
* Demo preparation

---

## Demo

### Pitch Deck

[Link](https://gamma.app/docs/Turn-Raw-Recordings-Into-Polished-Tutorials-Automatically-qjdjzxy1zv4jb4f)

### Demo Video

[Link](https://youtu.be/NfqgOWDMl8A)

---

## Screenshots

### Main UI

![Main UI](docs/screenshots/main-ui.png)

### Recording State

![Recording State](docs/screenshots/recording-state.png)

### Export State

![Export State](docs/screenshots/export-state.png)

### Edited Output

![Edited Output](docs/screenshots/edited-video.png)

### Demo

![Demo](docs/gifs/demo.gif)

---

## How to Run Locally

```bash
git clone <repo-url>
cd FocusFlow
pnpm install
pnpm tauri dev
```

Build release:

```bash
pnpm tauri build
```

---

## Live / Hosted Project

Not applicable.

FocusFlow is currently a Windows desktop application distributed as an MSI installer.

---

## Future Roadmap

* Audio and microphone recording
* Window and monitor selection
* Region recording
* Timeline editor
* Cursor customization
* Annotation tools
* Export presets
* Cross-platform support

---

## Repository

GitHub Repository: [repository-link](https://github.com/UmarAlMukhtar/FocusFlow)
