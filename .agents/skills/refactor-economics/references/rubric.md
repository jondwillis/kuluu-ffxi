# The rubric: what actually pays, and what only looks like it does

## Contents
- Where this comes from, and how much to trust it
- Ranked techniques
- Zero-value and negative-value moves
- The catalogue of disguised behaviour changes
- The ordering law, and its honest status
- What the study says about agents doing this work

## Where this comes from, and how much to trust it

Everything labelled MEASURED traces to one experiment: Edwards-Alexander, *The
economic benefit of refactoring for AI agents* (martinfowler.com, Jul 2026). One
Rust file (~17k lines), one repeated task, one run per step, token counts
approximated, **noise floor about ±3%**. Several published rows are byte-identical
to their predecessors in every column including wall-clock — those are copied,
not measured, so read them as *no evidence* rather than as evidence of zero.

It is also a greenfield, single-developer codebase with no wire-protocol
conformance surface and no concurrent editors. Most real codebases are none of
those.

**Take the direction and the shape from this. Take no magnitude.** Quoting its
83% headline as your own result is the single easiest way to discredit a
refactoring program.

The metric is **input (read) tokens only**. Output tokens *rose* 24% across the
experiment and cost several times more per token. No refactoring here is a
generation-cost play; don't sell one as such.

## Ranked techniques

| Rank | Technique | Evidence | Measured effect |
|---|---|---|---|
| 1 | Split a monolith into **per-domain** files whose names answer the question the task arrives under | MEASURED | The two largest drops, together ~82% of the total saving |
| 2 | Move a whole concern out of the hot file into a named by-kind module | MEASURED | Two drops of ~11-13% each |
| 3 | A `mod.rs`/index that is a **re-export manifest, not content** | IMPLIED | present in the winning layout, never isolated |
| 4 | Visibility narrowing to make a split stick (convention → compiler-checked) | EXTRAPOLATED | — |
| 5 | Typed errors instead of a bare optional for "malformed" | EXTRAPOLATED | — |
| 6 | Newtypes over primitives carrying domain meaning | EXTRAPOLATED | — |
| 7 | Extract Function / de-dup / "internal language" | MEASURED — **negative in isolation**; IMPLIED as setup for #1 | worst single step **+11%**; the whole phase **+7%** |
| 8 | Extract Class that creates no file boundary | MEASURED — **zero** | ~-2.7%, inside noise |
| 9 | Splitting on a **mechanism** axis, or splitting with no name to select on | MEASURED — zero/negative | one row 0, others slightly positive |
| 10 | Co-locating tests, iterator-style rewrites, cosmetic tidying | MEASURED — zero | justify on correctness or human ergonomics only |

**Size is not the unit.** Correlation between layer size and tokens was ~0.07,
and the layer *grew* while tokens fell 83%. Anyone arguing "fewer lines, fewer
tokens" is contradicted by the dataset. The relationship is threshold-like: you
are buying the moment a reader can skip a whole file unread, not shaving bytes.

**Big-but-shallow moves measure nothing.** The clearest cause-vs-correlation
datapoint: an Extract Class that displaced ~1,200 lines but left them in the
same file changed the read set not at all, and the meter did not move. Lines
displaced is not the same as bytes no longer read.

## Zero-value and negative-value moves

Recognise these before proposing them:

- **The S5 shape** — replace inline code with a call to a new helper across many
  sites. Indirection with no boundary. The worst-measured step in the study.
  It also has a hidden cost people miss: it destroys grep as an index. If
  `rg 'SomeError::Truncated'` is today a precise list of every bounds decision in
  a crate, folding those into `require(...)` deletes that index permanently.
- **Mechanism-axis modules** — `encoders.rs`, `parsers.rs`, `handlers.rs`,
  `utils.rs`. They intersect every question, so nothing becomes skippable.
- **Test co-location** — measured zero. Sometimes still right (a `mod.rs` that is
  97% test content fails "a manifest is not content"; a test module 1,000 lines
  from the function it covers is a false-negative trap). Take it on *those*
  arguments, and claim no token benefit.
- **Cosmetic renaming of function-local bindings** — invisible to grep and to the
  read set.
- **Macros to compress repetition** — the S5 shape *plus* broken grep. A reader
  must expand the macro mentally to answer a question the source no longer
  states literally. If a codebase has none, that is a defensible position worth
  keeping.
- **Reshaping a type "to prevent a future bug"** with no bug present, at a cost
  of dozens of call-site edits.

## The catalogue of disguised behaviour changes

Every one of these was proposed by a competent reviewer during real work, looked
like a clean refactor, and was a behaviour change. Check for them by name.

- **Clamp vs wrap.** `saturating_sub(1)` / `if next >= n { 0 }` is a *clamp*;
  a modulo helper *wraps*. They agree for an in-range cursor and disagree the
  moment a list shrinks while open — different selected row, in both directions.
- **The guarded call site.** A site wrapped in `if count > 0` is not
  interchangeable with a helper that returns 0 for an empty input: one preserves
  a stale value, the other resets it.
- **N readers, M policies.** Several functions that "all read a name field" can
  implement genuinely different policies — different minimum lengths, some
  filtering by character class, some not. Unifying them silently changes what
  parses. Read every one before calling any two duplicates; the reason for a
  divergence is often in a comment right there.
- **Warn-and-swallow.** Folding "do the thing, warn on failure" into a helper is
  safe *only* if no caller branches on the result. Check every site. In one case
  7 of 33 call sites branched — driving cast-locking and link-down detection —
  so the helper would have silently disabled failure detection.
- **The catch-all arm.** Replacing named arms with `_ =>` removes the
  compiler's drift alarm; adding one where the code previously handled variants
  explicitly means a future variant is silently swallowed.
- **Struct-update syntax on a decoder.** `..Default::default()` deletes the
  exhaustive-field check, so a new field silently defaults instead of failing to
  compile. On a wire boundary that alarm is worth far more than the lines saved.
- **Merging two dispatches that run in a defined order**, where one `continue`s
  on some inputs and suppresses the other. Unifying changes what runs.
- **Optional → typed error** where callers branch on the empty case. Often
  correct in the abstract, still a behaviour change, and it needs its own commit
  and its own review.

## The ordering law, and its honest status

De-duplication and naming → an internal language → a discoverable partition →
visibility narrowing to seal it.

The reasoning is sound: you cannot move what you cannot name, and stripping
shared mechanics is what makes the residue cluster into domains. The author's
own framing is that randomly cutting a file into smaller files is unlikely to
help much.

But there is **no control arm**, the author concedes the order was not planned,
and the enabling phase measured as a net cost. So the binding rule is
asymmetric: a split without prior de-duplication is fine; de-duplication without
a already-written-down target file list is not.

## What the study says about agents doing this work

Worth internalising, because it sets the process discipline:

- Agents were **poor at diagnosing** which refactorings applied — a human had to
  choose. Treat any audit as leads, not facts, and verify load-bearing claims
  yourself.
- Agents were **poor at applying** them mechanically; scripted transforms got
  confused, and the single most valuable refactoring was missed on the first
  pass entirely.

Hence: verify claims before building on them, prove moves mechanically rather
than by eye, and treat "stop and report" as the correct response to a plan that
no longer matches reality.
