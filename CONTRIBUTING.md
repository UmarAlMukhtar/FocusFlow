# Contributing to FocusFlow

Thank you for your interest in contributing to FocusFlow! We want to make contributing to this project as easy and beginner-friendly as possible.

FocusFlow is an open-source screen recording tool that automatically transforms raw recordings into polished tutorial videos. By contributing, you help make professional video creation accessible to everyone.

---

## Table of Contents

1. [How to Fork and Clone](#how-to-fork-and-clone)
2. [Setting Up Your Local Environment](#setting-up-your-local-environment)
3. [Running Locally](#running-locally)
4. [Building for Release](#building-for-release)
5. [Development Workflow](#development-workflow)
6. [Good First Contribution Ideas](#good-first-contribution-ideas)
7. [Reporting Bugs](#reporting-bugs)
8. [Requesting Features](#requesting-features)
9. [Code Style Expectations](#code-style-expectations)
10. [Testing Checklist Before Submitting PRs](#testing-checklist-before-submitting-prs)

---

## How to Fork and Clone

To start working on FocusFlow, you will need to create your own copy of the repository on GitHub and clone it to your local machine.

1. Navigate to the [FocusFlow Repository](https://github.com/UmarAlMukhtar/FocusFlow).
2. Click the **Fork** button in the top-right corner of the page.
3. Once the fork is created, clone it to your machine:

```bash
git clone https://github.com/<your-username>/FocusFlow.git
cd FocusFlow
```

4. Add the original repository as an upstream remote to stay updated:

```bash
git remote add upstream https://github.com/UmarAlMukhtar/FocusFlow.git
```

---

## Setting Up Your Local Environment

FocusFlow is a desktop application built with Tauri, Rust, React, and TypeScript. You need to set up dependencies for both Rust and Node.js.

### Prerequisites

* **Node.js**: Install the latest LTS version (v18 or higher recommended).
* **pnpm**: We use `pnpm` for package management. Install it via `npm i -g pnpm`.
* **Rust**: Install Rust using [rustup](https://rustup.rs/). FocusFlow requires Rust 1.75+.
* **Windows Build Tools**: Because FocusFlow is currently Windows-first, you will need the C++ Build Tools installed via Visual Studio Installer.
* **FFmpeg Sidecar**:
  For development, Tauri requires the FFmpeg executable placed inside `FocusFlow/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`. You can download a standard FFmpeg binary and rename it accordingly, or download the pre-built sidecar from the releases.

### Installing Dependencies

Run the following command in the root folder of the project:

```bash
pnpm install
```

This installs frontend packages and prepares workspace dependencies.

---

## Running Locally

To start the development environment, execute:

```bash
pnpm tauri dev
```

This starts a hot-reloading React server for the frontend, compiles the Rust backend in debug mode, and opens the FocusFlow application window.

---

## Building for Release

To build the production installer (`.msi` or `.exe`), run:

```bash
pnpm tauri build
```

The output bundle will be located in: `FocusFlow/src-tauri/target/release/bundle/msi/`.

---

## Development Workflow

### 1. Sync Your Fork

Before starting any work, ensure your local main branch is up-to-date with the upstream repository:

```bash
git checkout main
git pull upstream main
```

### 2. Create a Branch

Create a descriptive feature branch. Avoid making changes directly on the `main` branch.

```bash
# For features:
git checkout -b feature/your-feature-name

# For bug fixes:
git checkout -b fix/bug-description
```

### 3. Commit Changes

Keep commits granular and write clear, concise commit messages:

```bash
git commit -m "feat(recorder): add window-scoped click filtering"
```

### 4. Push and Open a Pull Request

Push your branch to your GitHub fork:

```bash
git push origin feature/your-feature-name
```

Go to the [FocusFlow Repository](https://github.com/UmarAlMukhtar/FocusFlow) on GitHub. You will see a prompt to open a Pull Request (PR) from your branch. Fill out the template description clearly explaining your changes.

---

## Good First Contribution Ideas

If you are looking for a place to start, consider:

* **UI Enhancements**: Adjusting margins, updating icons, or refining CSS transitions.
* **Documentation updates**: Fixing typos, explaining installation details, or translating documentation.
* **Adding Utility Tests**: Writing Rust unit tests for `recorder.rs` state machine transitions or timeline compilers.
* **Adding UI Unit Tests**: Implementing Jest/Testing Library tests for React components.

Look out for issues labeled `good first issue` on our GitHub issue tracker.

---

## Reporting Bugs

When reporting a bug, please open a GitHub issue with the following details:

* **Clear Title**: Summarize the issue (e.g., "Export fails when recording screen with 4K resolution").
* **Steps to Reproduce**: Detailed list of what you did.
* **Expected vs. Actual Behavior**: Explain what should have happened vs. what actually happened.
* **Logs & Stack Traces**: Check the Tauri terminal console or the session logs in `AppData/Roaming/com.umar.focusflow/Recordings/<session-id>/capture.log`.
* **System Info**: Windows OS build, Rust version, and Node.js version.

---

## Requesting Features

We welcome feature requests! Please open an issue and describe:

1. **The User Story**: Who needs this feature and what problem does it solve?
2. **Proposed Solution**: A mockup, technical design outline, or workflow description.
3. **Alternatives Considered**: Any other ways to solve the problem without adding code complexity.

---

## Code Style Expectations

### Frontend (React & TypeScript)

* Use functional React components with hooks.
* Use TypeScript types and interfaces; avoid `any`.
* Keep UI styling inside modular CSS files or Tailwind helper classes.

### Backend (Rust)

* Follow standard Rust formatting rules. Run `cargo fmt` inside the `src-tauri` directory.
* Run `cargo clippy` to check for common linting issues and optimizations.
* Document new functions, structs, and public interfaces.

---

## Testing Checklist Before Submitting PRs

Before you submit a PR, please complete the following checklist to ensure code quality:

* [ ] **Compilation**: Run `pnpm tauri dev` or `cargo check` and verify the project compiles without warnings or errors.
* [ ] **Formatting**: Run `cargo fmt` inside `src-tauri` to ensure standard style formatting.
* [ ] **Lints**: Run `cargo clippy` and make sure it returns no errors.
* [ ] **Local Verification**:
  * [ ] Record a 10-second screen recording session.
  * [ ] Verify that clicks and drag movements are tracked.
  * [ ] Verify the export pipeline successfully compiles the video with click indicators and auto-zoom.
  * [ ] Confirm the exported video plays properly in default media players.
* [ ] **No Unused Code**: Clean up debug print statements, commented-out code blocks, and unused dependencies.

---

Thank you for contributing to FocusFlow! 🚀
