---
name: kuluu-review
description: Review Kuluu PRs or commit stacks for concrete defects, project fit, retail grounding and applicable runtime evidence. Use for requested code reviews and merge-readiness assessments, including focused correction stacks.
---

# Kuluu review

Review the complete merge diff and linked issue intent against trusted repository instructions. Pin the head and base revisions; use their merge-base for the diff. For a commit-per-issue stack, inspect each commit's intent and then the combined behavior. Recheck revisions before publishing a verdict.

## Project fit

Kuluu defaults to Vanilla/Retail behavior. Assess each change against three categories:

- **Retail parity:** establish the intended behavior with the [retail-grounding skill](../retail-grounding/SKILL.md). Latest retail is the primary target; identify the client build when evidence is version-specific. LSB establishes server semantics, not client presentation. XIM is not retail evidence.
- **Modest improvement:** better FPS or restrained UX improvements can belong in the default client when their benefit is clear and they preserve retail gameplay and recognizable presentation/interaction without excessive convenience. Justify any restrained UX variation and its default activation. Ask what changes for the player, what tradeoffs exist and why the default is justified. A performance optimization still needs evidence that behavior is preserved.
- **Enhanced:** behavior beyond those bounds belongs on the Kuluu side behind explicit user opt-in. Debug/release selection is not an opt-in gate. Identify default activation, extension-surface use and lifecycle cleanup as applicable.

An issue link explains where work came from; it does not justify inclusion. Require the concrete problem, who benefits, why existing behavior or tooling is insufficient, and the cost or variation introduced. "Improves UX", "helps verification" and an issue label alone do not establish that benefit. Assess fit with these categories. Ambiguous convenience or gameplay policy is a decision for the maintainer, not a license to invent retail behavior or silently change defaults. Separate such decisions from demonstrated defects.

Look for unrelated changes that add review cost, regress established behavior or mix these categories. Recommend coherent independent PRs, with stacking only where dependencies require it. Judge scope by behavior and risk, not an arbitrary line limit.

Product-only developer tooling needs a concrete development use case, its activation boundary and evidence that player behavior is preserved. Do not call it retail parity merely because it drives game controls. If its justification claims a control or workflow is retail, support that premise with retail evidence too; a tooling label does not waive an embedded client-behavior claim.

## Contributor-facing issue references

Prefer verified GitHub issue URLs in PR bodies and review reports; retain Beads as the durable tracking source. Resolve the publisher's exact `<!-- beads-id: ID -->` marker across open and closed GitHub issues, or verify an imported bead's `external_ref: gh-N` against the issue. Do not infer an issue number from a bead ID or create a second issue merely to satisfy the template.

Before publishing maintainer-authored PRs, use [beads-github-sync](../beads-github-sync/SKILL.md) to preview and publish only their linked beads, then verify the returned GitHub issue URLs. The scoped path can publish referenced work before its export reaches main; explicitly selected closed tasks can be backfilled. Outside contributors need no Beads installation or issue-management permissions: accept ordinary GitHub issues or a PR for maintainer triage. Verify and reuse an existing contributor issue rather than projecting a duplicate. If publishing permission is unavailable, identify the maintainer action needed; do not demand extra privileges from the contributor.

Use the sync skill when a mapping is missing or stale. Local/unmerged exports have not reached the automatic main-branch publisher; manual publication defaults to a dry run, and closed beads without existing mappings are deliberately not backfilled. GitHub-only issues may require the opt-in inbound path rather than already having a bead. If no verified mapping exists, name the bead and publication gap briefly, or explain why the PR needs no issue. Correct sync problems through the canonical publisher/import workflow within existing authority; never assume two-way synchronization or broaden a review into bulk publication.

## Correctness and evidence

Follow the defect-first discipline of the generic review-agent: inspect surrounding code, call sites and meaningful tests; report discrete introduced problems with a demonstrated trigger and consequence. Continue through the entire diff. Do not turn style preferences, pre-existing failures or missing proof into bugs.

Require an evidence source and its supported conclusion for every material behavioral claim. Client behavior needs decisive retail observation, build-scoped binary inspection, DAT interpretation or an applicable official retail specification. Our implementation, matching test expectations, screenshots of Kuluu, green CI and community recreations do not independently establish retail behavior. Reuse verified records when they cover the claim. LSB is sufficient for server-owned wire semantics; label client presentation and workflow support separately. Cite a source path and symbol or a direct URL, build/pin where relevant, and state what remains inferred. Unsupported material parity or inclusion claims prevent a merge-ready recommendation.

Route LSB-boundary changes to [lsb-mirror-check](../lsb-mirror-check/SKILL.md), state-contract checks and applicable protocol reviewers. Route new spawns, resources or state-boundary work to [bevy-lifecycle-symmetry](../bevy-lifecycle-symmetry/SKILL.md). Use the repository check commands; distinguish a passing hook from full CI, skipped integration tests from executed ones, and a baseline failure from a regression.

Assess readiness against changed behavior and stated claims. Disclose relevant pre-existing gaps without requiring unrelated fixes. Tooling that only forwards existing inputs needs evidence of event routing and activation; it needs media when it changes visible behavior or claims an observed outcome.

PRs that change pixels or visible interaction require reviewer-accessible media in the body: screenshots for static appearance, video for motion, timing, camera, transitions or interaction. Prefer matched before/after conditions. Captions identify the tested commit, install/client profile, scene, reproduction steps and relevant clock/weather/settings. Inspect the media itself; black, stale, unrelated frames, local-only paths, logs and green CI do not prove visual correctness.

Use [verify](../verify/SKILL.md) for runtime collection. Match proof to the claim: a synthetic effect demo can establish rendering and orientation, but does not establish server dispatch or retail timing. An original combined-branch capture does not verify an extracted stack. Refresh affected evidence after relevant changes, or explain why it still applies. When collection or publication is unavailable, name the exact gap and keep the affected claim unverified; do not recommend merge while material correctness or project-fit questions remain unresolved.

## Review and correction authority

A review-only invocation is read-only: return findings, evidence gaps and decisions; do not edit, commit, push or post. When used with the generic review-agent, retain that agent's read-only and no-delegation constraints. A parent workflow may separately perform explicitly authorized verification, corrections and publication. This skill grants no outward-facing authority.

In an authorized correction workflow, put uncontroversial fixes and explanatory limitations in a focused correction stack. Keep one issue per coherent correction commit, referenced in the PR body using the contributor-facing mapping above. Put controversial proposals in a concise decision section for the maintainer. Prefer those notes over separate GitHub comments unless the user requests comments. Do not use a verbose corrections chronology; describe the final behavior, project justification, dependencies and actual validation.

## PR descriptions

Use the repository PR template with short Markdown headings: **Change**, **Why this belongs in Kuluu**, **Evidence**, and **Validation**; add **Visual evidence**, **Dependencies** or **Decision needed** only when useful. State final behavior and concrete rationale, then link decisive sources and describe their limits. Keep issue references and dependency notes compact. Prefer short bullets for parallel checks or claims. Remove refresh chronology, excluded experiments and boilerplate that does not help assess the final diff.

PR bodies are durable descriptions, not progress logs. Never include transient status such as "waiting for CI", "CI is running", "pending validation" or promises of future checks. Wait for the relevant check to finish before writing its result; keep pending status in the task conversation. A newly opened PR may report completed local validation while its first CI run has not finished, but do not add a CI placeholder. Record the completed result afterward, including failures and precise remaining validation gaps. State the tested revision and actual results. Source evidence and runtime/test evidence prove different claims, so do not substitute one for the other.

## Result

Lead with actionable defects ordered by severity, each anchored to a tight changed line range and explaining the trigger and consequence. Say `No findings.` when none qualify. Then give a brief project-fit assessment, actual validation and material evidence gaps or maintainer decisions. Distinguish proven correctness, supported inference and unverified claims. Merge readiness requires both project fit and sufficient proof; it is not implied by the absence of findings.
