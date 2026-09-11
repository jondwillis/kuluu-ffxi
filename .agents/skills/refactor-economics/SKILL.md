---
name: refactor-economics
description: Ranks a proposed refactor by what actually pays before any code moves, and executes mechanical migrations behind a validated oracle rather than asserting behaviour was preserved. Use when a file is called too big or hard to work in, for file/module splits, codemods, dependency upgrades, cross-codebase renames, or executing a refactoring plan someone hands you — especially across a protocol, wire-format, or other boundary where a silent behaviour change surfaces hours later. Refusing unprofitable work is a first-class deliverable.
user-invocable: true
---

# refactor-economics

Most of the value here is in what you **refuse**. A refactoring plan that
proposes everything an audit found is a bad plan: in the one controlled study
we have, the single most popular refactoring move — extract a helper and call
it from N sites — was the *worst-measured step*, and the phase built entirely
out of such moves cost more than it saved. Meanwhile the two moves that paid
were unglamorous file splits along the right axis.

So the job is not "find things to improve." It is: separate the few moves that
pay from the many that look like improvements and aren't, and — the part that
actually bites — catch the ones that are behaviour changes wearing a
refactor's costume.

The reference files are priced context — load each at the step that needs it,
never all of them up front. `references/rubric.md` before ranking or refusing
candidate work, because the intuitive ranking is wrong in a specific,
repeatable way. The other two only once work survives Step 0: a refusal or a
recommendation-not-to-proceed needs nothing beyond the rubric.

## The prime directive

A refactor that changes observable behaviour is a bug with good PR. On a
protocol or wire boundary it is a bug that surfaces hours later, to a user, far
from the commit that caused it.

Four rules carry most of the safety:

- **Similar is not identical.** Two functions with the same shape that differ in
  a bounds check, a minimum length, a filter, a default, or an error branch are
  not duplicates. Unifying them is a behaviour change. Put it on the rejected
  list, or raise it as its own proposal — never fold it into a refactor commit.
- **Mechanical and semantic changes ride in separate commits**, each
  independently green. A relocation commit that also "fixes" something cannot be
  reviewed, because the reviewer can no longer diff-check the relocation.
- **Let the compiler decide visibility**, not your checklist. Compile first, read
  the error, widen exactly that much. A grep hit is a hypothesis; the compiler
  is the authority. (A `grep` for a module name will happily match a struct
  field of the same name and send you widening visibility nothing needed.)
- **Don't touch user-visible strings** — log lines, chat text, error messages.
  If two of them disagree and one looks wrong, that is a finding to file, not a
  hunk to include.

## Step 0 — Is this worth doing at all?

Answer before editing. If you cannot, the honest output is a recommendation not
to proceed, and that is a perfectly good deliverable.

**The pre-flight gate.** All five must hold, or this is a redesign, not a
refactor:

1. You can write the exact public surface that must be identical afterwards, on
   one page, *before* you start.
2. Grep proves no collaborator reaches past that surface.
3. The repetition you plan to remove has a name and a count ("62 identical
   constructions", "all 20 parse functions"). If it has no name, you do not yet
   understand the domain well enough to split it.
4. The partition you plan to impose is one the *domain* supplies, not one you
   invented.
5. You have an oracle cheap enough to run after every single step.

**The funding argument.** State plainly why this file and why now. "It is big"
is not a reason — size is the symptom that makes you look, never the
justification. Real reasons: it changes constantly (check the actual commit
count over a real window), it has many open issues against it, it is where new
work keeps landing. If the honest answer is design quality rather than
measurable savings, say *that* — it is a legitimate reason and a much more
defensible claim than an invented number.

## Step 1 — Pick the axis

Splitting is only worth it if the result lets a reader **decide not to open a
file, from its name alone**. That property, not line count, is what pays.

Apply to every proposed module; a `no` means it does not earn its split:

1. **Negative inference** — does the name alone rule this file out for a typical
   question in this domain?
2. **No false negatives** — is nothing relevant hiding behind a name that says
   otherwise? One shared helper parked in a domain file breaks this silently and
   permanently.
3. **Axis match** — is it split on the axis *questions arrive on* (domain:
   `party`, `people`, `inventory`) rather than by mechanism (`encoders`,
   `parsers`, `handlers`)? A mechanism split intersects every question and buys
   nothing.
4. **Uniform skeleton** — do siblings share a shape, so reading one teaches the
   rest?
5. **Near-disjoint** — would a typical question require opening two or more of
   these? Then the boundary leaks.
6. **Real read-set delta** — name the bytes a reader loads today and won't after.
   If you can't name them, you're buying noise.

Also check the **edge count**: if the partition needs many sibling-to-sibling
references, it is a bad partition regardless of the file sizes it produces.
Sibling references are also where visibility traps live — in Rust,
`pub(super)` inside a nested module is *not* visible to a sibling module, which
is why flat module directories usually beat nested ones.

## Step 2 — Capture the invariant baseline BEFORE you touch anything

You cannot prove preservation without a "before", and after the first edit it is
gone. This is the step most likely to be skipped and most likely to be regretted.

Pick an oracle that would actually change if behaviour changed, then record it:

- the sorted list of test **names** (not just the count — a count is preserved by
  a rename, which is exactly the drift you're hunting)
- hashes of every file that consumes the thing you're moving
- for a wire/protocol boundary: bytes emitted for a fixed input

Then **verify the oracle is real** before trusting it. An oracle that silently
produces empty output compares equal to an equally empty baseline and proves
nothing — this has happened, and six commits reported passing it. Run it once
and look at the output with your own eyes.

`references/verification.md` has concrete recipes, including the
line-coverage check that proves a range move dropped nothing.

## Step 3 — Plan, and write the rejected list

The rejected list is not a footnote; it is the main deliverable. For each
rejected proposal record what it was and *why* — behaviour change, zero
measured value, wrong wave, or blocked. Future readers will otherwise re-derive
the same bad idea and re-litigate it.

Ordering: de-duplication and naming work comes *before* file splitting, so the
splits fall along seams the vocabulary revealed. But note the asymmetry — this
ordering is plausible and **not** experimentally controlled, and the
de-duplication phase measured as a net cost on its own. So:

- Splitting without a prior de-duplication pass: permitted.
- De-duplication without a **named target file list already written down**:
  forbidden. On its own it has negative measured value; it only pays as setup
  for a split you have already committed to.

**Name the cheapest standalone increment, and offer stopping there.** Before
pricing the full programme, find the one slice that buys most of what the user
actually complained about at the least risk, and present it as an explicit
off-ramp: do this first, then reassess whether the rest is still wanted.
Often it is something unglamorous — relocating self-contained `#[cfg(test)]`
modules to sibling files is a common case (child modules keep private access
through `use super::*`, so it is pure relocation). A move can price at zero on
the token rubric and still be the right first increment on ergonomics or risk
grounds; the rubric measures read-cost, not everything. A ten-slice plan with
no stopping point converts a navigability complaint into a mandatory
architecture project.

Size slices so each is independently committable and independently green.
Prefer more, smaller slices. Write each slice's mechanics precisely enough that
an executor cannot improvise — exact ranges, exact names, exact visibility —
because a vague step invites a creative reinterpretation of the task.

## Step 4 — Execute

Per slice: baseline → change → gate → diff the oracle against baseline → commit.

- Use the tooling that preserves history for file moves (`git mv`), so blame
  follows the code.
- For range moves, prove coverage **mechanically**: every source line assigned
  exactly once, any dropped line provably blank. Not by eye.
- Move comments and citations verbatim with their code. Deleting a source
  citation to tidy up is a gate failure, not a cleanup — on a protocol boundary
  that comment may be the only record of why an offset is what it is.
- Fix lint failures properly rather than suppressing them.
- If you cannot make a slice green, revert it cleanly and report. A clean
  abandon beats a broken tree, because later slices build on this one.

If reality contradicts the plan, **stop and report** rather than improvising a
different refactor. Agents are measurably poor at diagnosing which refactor to
apply; a plan that no longer matches the code is a signal to re-plan, not to
freelance.

## Step 5 — Verify adversarially

Verify as someone trying to *find the defect*, not to confirm the report. The
executor's own account is a hypothesis.

- Re-run the gate yourself and report the real result. Beware wrappers that
  swallow exit codes — piping a gate through another command gives you *that*
  command's status, not the gate's.
- For a relocation: confirm moved code is byte-identical modulo indentation and
  imports. Check it; don't assume.
- For a de-duplication: re-read each **original** occurrence in history and
  compare against the extracted helper, hunting the dropped guard, the changed
  default, the swallowed error branch.
- Check that no citation was deleted, no visibility widened past what the
  compiler demanded, no catch-all arm introduced that will silently swallow a
  future variant.
- Where behaviour is only observable at runtime, run it. A green unit gate says
  nothing about whether the screen still responds.

## Step 6 — Report honestly

State what you can and cannot claim. Do not quote a published percentage as if
it were yours — a saving measured on someone else's codebase is not a
measurement of yours. If you did not measure a before and after, say so.

Report the cost side too, including the analysis effort, and be explicit when
the deliverable was mostly *refusals* — that is a real result, not a shortfall.

## When to escalate to a multi-agent fan-out

Solo and sequential is the default and is usually right. Escalate when the
target is large (thousands of lines), very hot, or on a correctness-critical
boundary. The shape that worked:

rubric → parallel audits under **different lenses** (duplication, cohesion/
coupling, language idiom) → a planner that culls them → serialized execution →
independent adversarial review.

Two things matter. **Different lenses, not more of the same lens** — redundancy
finds the same things twice; diversity finds different failure modes. And
**serialize execution** when slices share a build tree or a crate, since a
broken intermediate in one poisons another's gate.

Budget honestly: a three-target wave of this shape ran ~41 agents and several
million tokens over hours, and produced 18 commits of which only two were
actual de-duplication. Worth it for a hot boundary file; absurd for a 300-line
module.

## Reference files

Load each when its step arrives, not before:

- `references/rubric.md` — ranked techniques with evidence class, the known
  zero-value and negative-value moves, and the catalogue of disguised
  behaviour changes. Load before ranking or refusing candidate work (Step 0-1).
- `references/verification.md` — baseline/oracle recipes, the line-coverage
  proof, and the adversarial-verification checklist. Load at Step 2, when you
  are specifying a real baseline; a plan that ends in a refusal never needs it.
- `references/kuluu.md` — this repo's specifics: gate commands and their
  traps, the LSB citation rule, beads writeback, comment and magic-number
  rules. Load before writing oracle commands or touching this repo's tree;
  skip it when analysing code in some other repository.
