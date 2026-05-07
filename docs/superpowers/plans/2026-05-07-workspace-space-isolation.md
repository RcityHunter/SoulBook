# Workspace Space Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Spaces are scoped to the active workspace so personal and team workspaces do not show the same global space list.

**Architecture:** Add optional workspace ownership fields to space records (`workspace_kind`, `team_id`) while treating legacy spaces without those fields as personal. The frontend reads `WorkspaceState` and sends workspace scope parameters to the spaces API; the backend filters list results and writes scope during creation.

**Tech Stack:** Rust, Axum, SurrealDB, Dioxus, gloo storage, serde.

---

### Task 1: Backend Query Scope

**Files:**
- Modify: `src/models/space.rs`
- Modify: `src/services/spaces.rs`

- [ ] **Step 1: Write failing tests**

Add tests showing personal scope includes legacy/personal spaces and team scope filters by team id.

- [ ] **Step 2: Run tests**

Run: `cargo test -q workspace_space_scope -- --nocapture`

Expected: fail because query scope helpers do not exist.

- [ ] **Step 3: Implement minimal backend scope helpers**

Add `workspace` and `team_id` query fields, parse personal/team scope, and append SurrealDB conditions.

- [ ] **Step 4: Run tests**

Run: `cargo test -q workspace_space_scope -- --nocapture`

Expected: pass.

### Task 2: Backend Create Scope

**Files:**
- Modify: `src/models/space.rs`
- Modify: `src/services/spaces.rs`

- [ ] **Step 1: Write failing tests**

Add tests showing create SQL contains `workspace_kind` and `team_id` fields.

- [ ] **Step 2: Run tests**

Run: `cargo test -q create_space_workspace_fields -- --nocapture`

Expected: fail before helper exists.

- [ ] **Step 3: Implement create fields**

Extend `CreateSpaceRequest` with optional `workspace` and `team_id`; write `workspace_kind: 'personal'` with `team_id: NONE` by default, and `workspace_kind: 'team'` with `team_id` when provided.

- [ ] **Step 4: Run tests**

Run: `cargo test -q create_space_workspace_fields -- --nocapture`

Expected: pass.

### Task 3: Frontend Spaces API Scope

**Files:**
- Modify: `/mnt/d/code/SoulBookFront/src/api/spaces.rs`
- Modify: `/mnt/d/code/SoulBookFront/src/pages/spaces.rs`

- [ ] **Step 1: Write failing tests**

Add a unit test for the spaces list URL builder: personal sends `workspace=personal`; team sends `workspace=team&team_id=<id>`.

- [ ] **Step 2: Run tests**

Run: `cargo test -q builds_workspace_scoped_space_list_path -- --nocapture`

Expected: fail because scoped URL builder does not exist.

- [ ] **Step 3: Implement scoped API**

Add `WorkspaceScope` and `list_spaces_scoped`; update Spaces page to read `WorkspaceState` and include scope in list and create requests.

- [ ] **Step 4: Run tests**

Run: `cargo test -q builds_workspace_scoped_space_list_path -- --nocapture`

Expected: pass.

### Task 4: Verification And Deploy

**Files:**
- Backend: `src/models/space.rs`, `src/services/spaces.rs`
- Frontend: `/mnt/d/code/SoulBookFront/src/api/spaces.rs`, `/mnt/d/code/SoulBookFront/src/pages/spaces.rs`

- [ ] **Step 1: Run backend verification**

Run: `cargo test -q workspace_space_scope -- --nocapture && cargo test -q create_space_workspace_fields -- --nocapture && cargo check -q`

- [ ] **Step 2: Run frontend verification**

Run: `cargo test -q builds_workspace_scoped_space_list_path -- --nocapture && cargo check -q`

- [ ] **Step 3: Commit and push backend**

Run: `git add src/models/space.rs src/services/spaces.rs docs/superpowers/plans/2026-05-07-workspace-space-isolation.md && git commit -m "fix: scope spaces by workspace" && git push hunter feat/soulauth-oidc-standard`

- [ ] **Step 4: Commit and push frontend**

Run: `git add src/api/spaces.rs src/pages/spaces.rs && git commit -m "fix: request spaces for active workspace" && git push hunter main`

- [ ] **Step 5: Deploy backend**

Run server deploy sequence and confirm `systemctl is-active soulbook.service` returns `active`.
