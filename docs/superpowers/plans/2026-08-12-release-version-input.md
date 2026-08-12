# Release Version Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a manual Windows release accept a semantic version and embed that exact version in the Tauri installer and GitHub Release.

**Architecture:** Preserve the existing tag-triggered release behavior. For `workflow_dispatch`, validate a required version input before updating the checked-out runner copies of the three version manifests; `tauri-action` then reads the synchronized Tauri configuration and derives its existing `v__VERSION__` tag and release title from it.

**Tech Stack:** GitHub Actions, PowerShell, Tauri v2, npm, Cargo.

## Global Constraints

- The manual input accepts semantic versions without a leading `v`, such as `1.2.3` or `1.2.3-rc.1`.
- The runner must synchronize `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml` before Tauri builds.
- Push-tag releases continue to use the repository version unchanged.

---

### Task 1: Add and consume the workflow version input

**Files:**
- Modify: `.github/workflows/release-windows.yml:17-76`
- Modify: `README.md:51-69`

**Interfaces:**
- Consumes: `inputs.version: string` from a manual `workflow_dispatch` run.
- Produces: matching version fields in the runner checkout and a release named `HIS XML Sync v{version}` via the existing `tauri-action` placeholders.

- [ ] **Step 1: Add the manual workflow input**

```yaml
workflow_dispatch:
  inputs:
    version:
      description: "Phiên bản SemVer để build, ví dụ 1.2.3"
      required: true
      type: string
```

- [ ] **Step 2: Add a failing-version guard**

```powershell
if ($version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
  throw "Version must be SemVer without a leading v, for example 1.2.3."
}
```

- [ ] **Step 3: Synchronize the build manifests for manual releases**

```powershell
$json = Get-Content -Raw $path | ConvertFrom-Json
$json.version = $version
$json | ConvertTo-Json -Depth 100 | Set-Content -NoNewline -Encoding utf8 $path
```

Update `package.json`, `src-tauri/tauri.conf.json`, and only the root package `version` line in `src-tauri/Cargo.toml`. Run this step only for `workflow_dispatch`, after `npm ci` and before `tauri-apps/tauri-action`.

- [ ] **Step 4: Document manual release behavior**

```markdown
Actions → Release Windows → Run workflow → nhập `version` theo SemVer, ví dụ `1.2.3`.
```

State that the installer and draft release are generated with that version.

- [ ] **Step 5: Validate YAML and the frontend build**

Run: `python -c "import yaml; yaml.safe_load(open('.github/workflows/release-windows.yml', encoding='utf-8')); print('workflow YAML valid')"; npm run build`

Expected: workflow parses as YAML and the frontend build succeeds.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release-windows.yml README.md
git commit -m "feat: add version input to Windows release"
```
