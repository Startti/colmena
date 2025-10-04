# Branch Protection Configuration Guide (GitHub Rulesets)

This guide provides step-by-step instructions for configuring **GitHub Rulesets** (the modern replacement for branch protection rules) to ensure code quality and prevent unauthorized changes.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [Why Rulesets vs Classic Branch Protection](#why-rulesets-vs-classic-branch-protection)
- [Branch Protection Configuration](#branch-protection-configuration)
  - [main (Production)](#1️⃣-main-production)
  - [staging (Pre-Production)](#2️⃣-staging-pre-production)
  - [develop (Development)](#3️⃣-develop-development)
  - [feature/* (Optional)](#4️⃣-feature-branches-optional)
- [GitHub Actions Permissions](#github-actions-permissions)
- [Status Checks Matrix](#status-checks-matrix)
- [Troubleshooting](#troubleshooting)

---

## Overview

Our repository uses a **GitFlow with staging** approach:

```
feature/* → develop → staging → main
    ↓          ↓         ↓        ↓
  CI only  CI + Val  CI+TestPyPI  PyPI+Release
```

Each branch has different protection rules to ensure quality gates at each stage.

---

## Prerequisites

Before configuring branch protection:

1. ✅ You must be a repository **admin** or have appropriate permissions
2. ✅ All workflows (CI, staging, validation) must be in place
3. ✅ At least one commit must exist in each branch

---

## Why Rulesets vs Classic Branch Protection

GitHub Rulesets are the **modern replacement** for classic branch protection rules:

| Feature | Classic Branch Protection | GitHub Rulesets ✨ |
|---------|---------------------------|-------------------|
| **Multiple branches** | ❌ One rule per branch pattern | ✅ One ruleset = multiple patterns |
| **Bypass control** | ❌ Limited bypass options | ✅ Granular bypass by user/team/app |
| **Push restrictions** | ⚠️ Hidden in options | ✅ Clear "Restrict pushes" option |
| **Insights** | ❌ No analytics | ✅ Compliance graphs & insights |
| **Priority** | ❌ N/A | ✅ Ordered by priority |
| **API support** | ⚠️ Limited | ✅ Full REST & GraphQL API |

---

## Branch Protection Configuration

### How to Access Rulesets

1. Go to your repository on GitHub
2. Click **Settings** → **Rules** → **Rulesets** (left sidebar)
3. Click **New ruleset** → **New branch ruleset**

---

## 1️⃣ **`main` (Production)**

The most critical branch - contains production-ready code.

### Configuration Steps

1. Click **New ruleset** → **New branch ruleset**

#### General Settings

- **Ruleset Name**: `Main Protection (Production)`
- **Enforcement status**: ✅ **Active**
- **Bypass list** (for emergencies only):
  - Add: `Repository administrators` ✅

#### Target Branches

- Click **Add target**
- Select **Include by pattern**
- Enter pattern: `main`

#### Rules to Enable

##### ✅ Restrict deletions
- **Enable** ✅
- **Why**: Prevents accidental deletion of the production branch

##### ✅ Require a pull request before merging
- **Enable** ✅
- **Required approvals**: `1`
- ✅ **Dismiss stale pull request approvals when new commits are pushed**
- ✅ **Require approval of the most recent reviewable push**
- [ ] Require review from Code Owners (optional)

> **Why**: Ensures all production code is reviewed and approved.

##### ✅ Require status checks to pass
- **Enable** ✅
- ✅ **Require branches to be up to date before merging**
- Click **Add checks** and select:
  - `Test and Publish to TestPyPI` ✅ (from ci-staging.yml)
  - `Validate Commit Messages` ✅ (from validate-commits.yml)

> **Why**: Code must pass staging validation before production deployment.

##### ✅ Require conversation resolution before merging
- **Enable** ✅

> **Why**: All PR comments must be resolved.

##### ✅ Require linear history
- **Enable** ✅

> **Why**: Maintains clean git history. Forces squash or rebase merges.

##### ✅ Block force pushes
- **Enable** ✅

> **Why**: Prevents rewriting production history.

##### ✅ Restrict pushes
- **Enable** ✅
- **Restrict who can push to matching branches**
- **Allowed actors**:
  - `github-actions[bot]` ✅ (for version bump commits)
  - `Repository administrators` ✅ (emergencies only)

> **Why**: Only automated CI and admins can push directly.

**Click "Create" to save the ruleset.**

---

## 2️⃣ **`staging` (Pre-Production)**

Testing environment before production release.

### Configuration Steps

1. Click **New ruleset** → **New branch ruleset**

#### General Settings

- **Ruleset Name**: `Staging Protection`
- **Enforcement status**: ✅ **Active**
- **Bypass list**: `Repository administrators` (optional)

#### Target Branches

- **Include by pattern**: `staging`

#### Rules to Enable

##### ✅ Restrict deletions
- **Enable** ✅

##### ✅ Require a pull request before merging
- **Enable** ✅
- **Required approvals**: `1`
- [ ] Dismiss stale approvals (more flexible for staging)
- ✅ **Require approval of the most recent reviewable push**

##### ✅ Require status checks to pass
- **Enable** ✅
- ✅ **Require branches to be up to date before merging**
- **Required checks**:
  - `Test` ✅ (from ci-develop.yml)
  - `Validate Commit Messages` ✅

> **Why**: Ensures develop code is tested before staging deployment.

##### ✅ Require conversation resolution before merging
- **Enable** ✅

##### ✅ Require linear history
- **Enable** ✅

> **Why**: Maintains clean history for easier rollbacks.

##### ✅ Block force pushes
- **Enable** ✅

##### ✅ Restrict pushes
- **Enable** ✅
- **Allowed actors**:
  - `github-actions[bot]` ✅
  - `Repository administrators` ✅

**Click "Create" to save.**

---

## 3️⃣ **`develop` (Development)**

Main development branch where features are integrated.

### Configuration Steps

1. Click **New ruleset** → **New branch ruleset**

#### General Settings

- **Ruleset Name**: `Develop Protection`
- **Enforcement status**: ✅ **Active**
- **Bypass list**: `Repository administrators` (optional)

#### Target Branches

- **Include by pattern**: `develop`

#### Rules to Enable

##### ✅ Restrict deletions
- **Enable** ✅

##### ✅ Require a pull request before merging
- **Enable** ✅
- **Required approvals**: `1`
- [ ] **Do NOT enable** "Dismiss stale approvals" (more flexible for development)
- ✅ **Require approval of the most recent reviewable push**

> **Why**: More flexible review process for active development.

##### ✅ Require status checks to pass
- **Enable** ✅
- ✅ **Require branches to be up to date before merging**
- **Required checks**:
  - `Test` ✅ (from ci-develop.yml)
  - `Validate Commit Messages` ✅

> **Why**: All code must pass tests and follow commit conventions.

##### ✅ Require conversation resolution before merging
- **Enable** ✅

##### ❌ Require linear history
- [ ] **Disabled**

> **Why**: Allows merge commits from feature branches, preserving feature history.

##### ✅ Block force pushes
- **Enable** ✅

**Click "Create" to save.**

---

## 4️⃣ **Feature Branches** (Optional)

Individual feature development branches.

### Configuration Steps

1. Click **New ruleset** → **New branch ruleset**

#### General Settings

- **Ruleset Name**: `Feature Branches`
- **Enforcement status**: ✅ **Active**

#### Target Branches

- **Include by pattern**: `feature/*`

#### Rules to Enable

##### ✅ Require a pull request before merging
- **Enable** ✅
- **Required approvals**: `1`

##### ⚠️ Other Settings
- Minimal restrictions for development flexibility

> **Why**: Maximum flexibility while ensuring code review.

**Click "Create" to save.**

---

## 5️⃣ **Hotfix Branches** (Optional)

**Branch pattern**: `hotfix/*`

### Configuration Steps

Same as feature branches, but add:

##### ✅ Require status checks to pass
- **Required checks**:
  - `Test` ✅
  - `Validate Commit Messages` ✅

---

## GitHub Actions Permissions

**Critical**: Configure GitHub Actions permissions to allow automated workflows.

### Steps

1. Go to **Settings** → **Actions** → **General**
2. Under **Workflow permissions**:
   - ✅ Select **Read and write permissions**
   - ✅ Enable **Allow GitHub Actions to create and approve pull requests**
3. Click **Save**

### Why This Is Needed

This allows `github-actions[bot]` to:
- ✅ Commit version bumps to `main`
- ✅ Create git tags
- ✅ Push to protected branches (if listed in "Allowed actors")
- ✅ Create GitHub Releases
- ✅ Update CHANGELOG.md

---

## Status Checks Matrix

Required status checks for each branch:

| Target Branch | Source Branch | Required Status Checks |
|---------------|---------------|------------------------|
| `main` | `staging` | `Test and Publish to TestPyPI`, `Validate Commit Messages` |
| `staging` | `develop` | `Test`, `Validate Commit Messages` |
| `develop` | `feature/*` | `Test`, `Validate Commit Messages` |

---

## Permission Levels

| Role | Permissions |
|------|-------------|
| **Developers** | Can create PRs to `develop` |
| **Tech Leads** | Can merge `develop` → `staging` |
| **Release Manager** | Can merge `staging` → `main` |
| **GitHub Actions Bot** | Can commit version bumps, create tags/releases |
| **Repository Admins** | Can bypass in emergencies |

---

## Verification Checklist

After configuring all rulesets, verify:

- [ ] Go to **Settings** → **Rules** → **Rulesets** - you should see all rulesets listed as "Active"
- [ ] Create a test PR to `develop` with a non-conventional commit - should be blocked by `Validate Commit Messages`
- [ ] Try to push directly to `develop` - should be blocked
- [ ] Create a test PR to `staging` - should require status checks from develop CI
- [ ] Create a test PR to `main` - should require status checks from staging CI
- [ ] Verify GitHub Actions can push to `main` (check workflow permissions)

---

## Troubleshooting

### Problem: Status check not appearing in the list

**Solution**:
1. The workflow must run at least once before GitHub recognizes it
2. Create a test PR to trigger the workflow
3. Once it runs, the status check will appear when you click "Add checks"
4. If still missing, check the exact job name in the PR's "Checks" tab

### Problem: GitHub Actions can't push to protected branch

**Solution**:
1. Verify **Workflow permissions** are set to "Read and write"
2. Check that `github-actions[bot]` is in the "Allowed actors" list under "Restrict pushes"
3. Ensure the bypass list doesn't accidentally block the bot

### Problem: PR can't be merged despite passing checks

**Solution**:
1. Check if "Require branches to be up to date" is enabled
2. Rebase or merge the target branch into your PR branch
3. Wait for status checks to run again

### Problem: Can't find specific status check name

**Solution**:
1. Go to a recent PR's "Checks" tab
2. Copy the **exact name** of the workflow/job (case-sensitive)
3. Use that exact name when adding checks to the ruleset

### Problem: Ruleset not enforcing on certain users

**Solution**:
1. Check the "Bypass list" - admins might be accidentally bypassing
2. Verify "Enforcement status" is set to "Active" (not "Evaluate" or "Disabled")
3. Check if there's a conflicting ruleset with higher priority

---

## Viewing Ruleset Insights

GitHub Rulesets provide analytics:

1. Go to **Settings** → **Rules** → **Rulesets**
2. Click on any ruleset name
3. View the **Insights** tab to see:
   - How many times the ruleset was evaluated
   - How many times it was bypassed
   - Compliance metrics

---

## Best Practices

1. ✅ **Start with `main`**: Configure main branch first, then work backwards
2. ✅ **Test thoroughly**: Create test PRs to verify rules work as expected
3. ✅ **Document exceptions**: If you allow bypassing, document why in the PR
4. ✅ **Review regularly**: Audit rulesets quarterly using the Insights tab
5. ✅ **Train team**: Ensure all developers understand the workflow
6. ✅ **Use patterns wisely**: Group similar branches (e.g., `feature/*`, `hotfix/*`) in one ruleset

---

## Migrating from Classic Branch Protection

If you have existing classic branch protection rules:

1. **Don't delete old rules immediately** - test rulesets first
2. Create equivalent rulesets following this guide
3. Test with PRs to ensure everything works
4. Once verified, you can delete the old branch protection rules
5. Rulesets take precedence over classic rules when both exist

---

## Related Documentation

- [CI/CD Guide](./10_cicd_guide.md)
- [Git Hooks Guide](./09_git_hooks.md)
- [Contributing Guide](./08_contributing.md)

---

## External Resources

- [GitHub Rulesets Documentation](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets)
- [Conventional Commits](https://www.conventionalcommits.org/)
- [Semantic Versioning](https://semver.org/)

---

**Last Updated**: 2025-10-04
**Version**: 2.0.0 (Updated for GitHub Rulesets)
