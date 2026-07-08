# HIS XML Sync Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the initial Tauri v2 + React TypeScript desktop app skeleton for HIS XML synchronization.

**Architecture:** The frontend starts with an activation screen and a sidebar-based home shell. The Rust backend exposes stable Tauri command contracts and separates license, settings, XML parsing, HIS API, sync orchestration, and tray behavior into focused modules.

**Tech Stack:** Tauri v2, Rust, React 19, TypeScript, Vite, npm.

---

### Task 1: Scaffold Desktop App

**Files:**
- Create: `his-xml-sync/package.json`
- Create: `his-xml-sync/src-tauri/tauri.conf.json`
- Create: `his-xml-sync/src/App.tsx`

- [x] **Step 1: Run Tauri scaffold**

```bash
npm create tauri-app@latest his-xml-sync -- --template react-ts --manager npm --tauri-version 2 --identifier vn.vietnga.hisxmlsync --yes
```

- [x] **Step 2: Confirm scaffold files exist**

```bash
rg --files his-xml-sync | sed -n '1,80p'
```

Expected: React/Vite files under `src` and Rust/Tauri files under `src-tauri`.

### Task 2: Add App Structure

**Files:**
- Modify: `his-xml-sync/src/App.tsx`
- Create: `his-xml-sync/src/screens/ActivationScreen.tsx`
- Create: `his-xml-sync/src/screens/HomeShell.tsx`
- Create: `his-xml-sync/src/types.ts`
- Modify: `his-xml-sync/src/App.css`

- [x] **Step 1: Replace default demo UI with activation/home flow**

The app uses React state to render `ActivationScreen` until a valid session exists, then renders `HomeShell`.

- [x] **Step 2: Add responsive desktop layout**

The shell uses a left sidebar, topbar, stat cards, and a work surface for each menu section.

### Task 3: Add Rust Command Boundaries

**Files:**
- Modify: `his-xml-sync/src-tauri/src/lib.rs`
- Create: `his-xml-sync/src-tauri/src/commands.rs`
- Create: `his-xml-sync/src-tauri/src/license.rs`
- Create: `his-xml-sync/src-tauri/src/settings.rs`
- Create: `his-xml-sync/src-tauri/src/xml_parser.rs`
- Create: `his-xml-sync/src-tauri/src/his_api.rs`
- Create: `his-xml-sync/src-tauri/src/sync.rs`
- Create: `his-xml-sync/src-tauri/src/tray.rs`

- [x] **Step 1: Register Tauri command handlers**

Commands expose license status, activation, settings, XML preview, and manual sync.

- [x] **Step 2: Add tray close-to-hide behavior**

The Tauri window close event calls `prevent_close()` and hides the window. The tray menu can show the app or quit it.

### Task 4: Create UI Agent Prompts

**Files:**
- Create: `his-xml-sync/docs/prompts/activation-screen-ui.md`
- Create: `his-xml-sync/docs/prompts/home-screen-ui.md`

- [x] **Step 1: Write activation screen prompt**

The prompt describes the license entry screen, validation states, security expectations, and visual tone.

- [x] **Step 2: Write home screen prompt**

The prompt describes the sidebar shell, expected menus, operational dashboard, and desktop app ergonomics.
