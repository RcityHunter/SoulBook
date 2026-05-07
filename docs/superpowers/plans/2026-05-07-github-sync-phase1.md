# GitHub Sync Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing "立即同步" action perform a real SoulBook -> GitHub sync instead of only writing a pending log.

**Architecture:** Keep phase 1 backend-only and one-way. The `git_sync` route loads the bound repo and space documents, converts documents into Markdown files under `path_prefix`, pushes those files through GitHub Contents API using `GITHUB_TOKEN`, then records a success or failed sync log with file count and commit metadata.

**Tech Stack:** Rust, Axum, SurrealDB, reqwest, GitHub REST Contents API, existing `git_repo` and `git_sync_log` tables.

---

### Task 1: Add Pure GitHub Sync Helpers

**Files:**
- Create: `src/services/git_sync.rs`
- Modify: `src/services/mod.rs`

- [ ] Add tests for GitHub URL parsing, path normalization, and Markdown export names.
- [ ] Implement helper functions without network calls.
- [ ] Run `cargo test git_sync -- --nocapture`.

### Task 2: Wire Real Sync Into Route

**Files:**
- Modify: `src/routes/git_sync.rs`

- [ ] Replace fake `trigger_sync` log with real repo load, document load, GitHub sync call, and final log.
- [ ] Use `GITHUB_TOKEN` from environment for phase 1 credentials.
- [ ] Persist `status`, `message`, `files_changed`, and `commit_sha` in `git_sync_log`.
- [ ] Run `cargo test git_sync -- --nocapture`.

### Task 3: Verify And Deploy

**Files:**
- Backend only.

- [ ] Run `cargo check -q`.
- [ ] Commit only GitHub sync files.
- [ ] Push `hunter feat/soulauth-oidc-standard`.
- [ ] Deploy backend and verify `POST /api/docs/git-sync/repositories/:id/sync` writes a real success or actionable failure log.
