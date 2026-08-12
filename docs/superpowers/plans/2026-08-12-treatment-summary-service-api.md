# Treatment Summary Service API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve each device's examination-service ID from the new per-treatment HIS summary endpoint while consistently using the configured facility ID.

**Architecture:** Keep the patient lookup and service-ID cache. Change only the treatment-summary request to `tong-hop/{nbDotDieuTriId}`, retain `data.dsDvKham[0].id`, and use the saved HIS facility ID in all outgoing requests.

**Tech Stack:** Rust, Tauri, reqwest, serde_json, SQLite, React, TypeScript.

## Global Constraints

- Use `/api/his/v1/nb-dot-dieu-tri/tong-hop/{nbDotDieuTriId}?dsCoSoKcbId={configured facility ID}` for KR-800, HDR-9000, and CT-800.
- Read only `data.dsDvKham[0].id`; do not require `dsDvKham[].nbDotDieuTriId` to equal the path ID.
- The default facility ID is `1`; a value entered and saved in HIS settings remains authoritative.

---

### Task 1: Update treatment-summary requests and service parsing

**Files:**
- Modify: `src-tauri/src/kr800_process.rs:23,162-168,1332-1365,1843-1849`
- Modify: `src-tauri/src/hdr9000.rs:28,899-916`
- Modify: `src-tauri/src/ct800.rs:31,1537-1556`

**Interfaces:**
- Consumes: `nb_dot_dieu_tri_id: i64` and `settings.ds_co_so_kcb_id: i64`.
- Produces: `dv_kham_id: i64` read from `data.dsDvKham[0].id`.

- [ ] **Step 1: Write the failing KR-800 parser test**

```rust
let body = r#"{"data":{"dsDvKham":[{"id":292415,"nbDotDieuTriId":40822}]}}"#;
assert_eq!(parse_service_visit_id(body), Ok(292415));
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test parses_first_service --manifest-path src-tauri/Cargo.toml`

Expected: FAIL because the parser still validates the treatment ID.

- [ ] **Step 3: Implement the per-treatment request**

```rust
let summary_url = format!("{}/{}", his_api::join_url(&settings.his_api_url, SUMMARY_PATH), nb_id);
client.get(&summary_url).bearer_auth(&auth).query(&[
    ("dsCoSoKcbId", settings.ds_co_so_kcb_id.to_string()),
])
```

Apply this shape in all three device modules and let KR-800 parse only the first service ID.

- [ ] **Step 4: Run focused tests**

Run: `cargo test parses_first_service --manifest-path src-tauri/Cargo.toml`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/kr800_process.rs src-tauri/src/hdr9000.rs src-tauri/src/ct800.rs
git commit -m "fix: use per-treatment HIS summary endpoint"
```

### Task 2: Make the configured facility ID authoritative with default 1

**Files:**
- Modify: `src-tauri/src/settings.rs:43-45`
- Modify: `src-tauri/src/db.rs:53,458,503,956,996`
- Modify: `src-tauri/src/xml_track.rs:316-319`
- Modify: `src-tauri/src/kr800_process.rs:1177-1178,1276-1292`
- Modify: `src/lib/appCommands.ts:25,152`

**Interfaces:**
- Consumes: `AppSettings.ds_co_so_kcb_id` from the existing HIS settings form.
- Produces: `dsCoSoKcbId` using that saved value, with `1` for fresh configurations.

- [ ] **Step 1: Write failing default and query tests**

```rust
assert_eq!(default_ds_co_so_kcb_id(), 1);
assert_eq!(build_patient_query(&params, from, to, 7)
    .iter().find(|(key, _)| key == "dsCoSoKcbId"), Some(&("dsCoSoKcbId".into(), "7".into())));
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml facility`

Expected: FAIL because defaults are `4` and KR-800 uses the separately stored query value.

- [ ] **Step 3: Implement central facility handling**

```rust
"dsCoSoKcbId" => settings.ds_co_so_kcb_id.to_string(),
```

Pass the saved facility ID into `build_patient_query`. Replace schema and UI defaults of `4` with `1`, without overwriting a facility value already saved by the user.

- [ ] **Step 4: Run full verification**

Run: `cargo test --manifest-path src-tauri/Cargo.toml; npm run build`

Expected: all Rust tests pass and the frontend build succeeds.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/settings.rs src-tauri/src/db.rs src-tauri/src/xml_track.rs src-tauri/src/kr800_process.rs src/lib/appCommands.ts
git commit -m "fix: default HIS facility to one"
```
