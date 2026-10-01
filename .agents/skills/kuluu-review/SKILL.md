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

An issue link explains where work came from; it does not justify inclusion. Assess the PR's concrete user benefit and fit with these categories. Ambiguous convenience or gameplay policy is a decision for the maintainer, not a license to invent retail behavior or silently change defaults. Separate such decisions from demonstrated defects.

Look for unrelated changes that add review cost, regress established behavior or mix these categories. Recommend coherent independent PRs, with stacking only where dependencies require it. Judge scope by behavior and risk, not an arbitrary line limit.

## Contributor-facing issue references

Prefer verified GitHub issue URLs in PR bodies and review reports; retain Beads as the durable tracking source. Resolve the publisher's exact `<!-- beads-id: ID -->` marker across open and closed GitHub issues, or verify an imported bead's `external_ref: gh-N` against the issue. Do not infer an issue number from a bead ID or create a second issue merely to satisfy the template.

Use [beads-github-sync](../beads-github-sync/SKILL.md) when a mapping is missing or stale. Local/unmerged exports have not reached the automatic main-branch publisher; manual publication defaults to a dry run, and closed beads without existing mappings are deliberately not backfilled. GitHub-only issues may require the opt-in inbound path rather than already having a bead. If no verified mapping exists, name the bead and publication gap briefly, or explain why the PR needs no issue. Correct sync problems through the canonical publisher/import workflow within existing authority; never assume two-way synchronization or broaden a review into bulk publication.

## Correctness and evidence

Follow the defect-first discipline of the generic review-agent: inspect surrounding code, call sites and meaningful tests; report discrete introduced problems with a demonstrated trigger and consequence. Continue through the entire diff. Do not turn style preferences, pre-existing failures or missing proof into bugs.

Route LSB-boundary changes to [lsb-mirror-check](../lsb-mirror-check/SKILL.md), state-contract checks and applicable protocol reviewers. Route new spawns, resources or state-boundary work to [bevy-lifecycle-symmetry](../bevy-lifecycle-symmetry/SKILL.md). Use the repository check commands; distinguish a passing hook from full CI, skipped integration tests from executed ones, and a baseline failure from a regression.

PRs that can change pixels or visible interaction require reviewer-accessible media in the body: screenshots for static appearance, video for motion, timing, camera, transitions or interaction. Prefer matched before/after conditions. Captions identify the tested commit, install/client profile, scene, reproduction steps and relevant clock/weather/settings. Inspect the media itself; black, stale, unrelated frames, local-only paths, logs and green CI do not prove visual correctness.

Use [verify](../verify/SKILL.md) for runtime collection. Match proof to the claim: a synthetic effect demo can establish rendering and orientation, but does not establish server dispatch or retail timing. An original combined-branch capture does not verify an extracted stack. Refresh affected evidence after relevant changes, or explain why it still applies. When collection or publication is unavailable, name the exact gap and keep the affected claim unverified; do not recommend merge while material correctness or project-fit questions remain unresolved.

## Review and correction authority

A review-only invocation is read-only: return findings, evidence gaps and decisions; do not edit, commit, push or post. When used with the generic review-agent, retain that agent's read-only and no-delegation constraints. A parent workflow may separately perform explicitly authorized verification, corrections and publication. This skill grants no outward-facing authority.

In an authorized correction workflow, put uncontroversial fixes and explanatory limitations in a focused correction stack. Keep one issue per coherent correction commit, referenced in the PR body using the contributor-facing mapping above. Put controversial proposals in a concise decision section for the maintainer. Prefer those notes over separate GitHub comments unless the user requests comments. Do not use a verbose corrections chronology; describe the final behavior, project justification, dependencies and actual validation.

## Result

Lead with actionable defects ordered by severity, each anchored to a tight changed line range and explaining the trigger and consequence. Say `No findings.` when none qualify. Then give a brief project-fit assessment, actual validation and material evidence gaps or maintainer decisions. Distinguish proven correctness, supported inference and unverified claims. Merge readiness requires both project fit and sufficient proof; it is not implied by the absence of findings.
