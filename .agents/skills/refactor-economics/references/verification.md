# Proving behaviour was preserved

## Contents
- Choosing an oracle
- Validate the oracle before trusting it
- Recipes (before/after capture)
- The adversarial checklist
- Runtime verification

"It compiles" and "tests pass" are necessary and nowhere near sufficient. A
relocation can compile, pass, and still have dropped a bounds check that only
fires on malformed input.

## Choosing an oracle

A good oracle changes if and only if behaviour changes. Pick from:

| Change | Oracle |
|---|---|
| Moving tests or code between files | sorted list of test **names** |
| Extracting/moving a function | consumer files byte-identical |
| Wire encoder/decoder | bytes emitted for a fixed input |
| Any relocation | moved lines byte-identical modulo indentation and imports |

Use test **names**, not counts. A count survives a rename, which is precisely
the drift you are hunting.

## Validate the oracle before trusting it

An oracle that fails silently is worse than none, because it manufactures
confidence. The real failure: an acceptance command that errored, was piped
through `2>/dev/null | sed | sort`, emitted an empty list, and compared equal to
an equally empty baseline. Six commits reported passing it.

Before relying on any oracle:

1. Run it once and **look at the output**. Is it non-empty? Does it have roughly
   the number of entries you expect?
2. Deliberately break something and confirm the oracle notices. If drifting a
   value doesn't change the output, the oracle is decorative.

The same trap applies to exit codes: piping a gate through another command gives
you *that* command's status. `gate | tail -25` exits 0 even when the gate fails.
Capture the status explicitly:

```bash
<gate-command> > gate.log 2>&1; echo "EXIT=$?"
```

## Recipes

**Test-name invariant** (adapt the command to the project's runner):

```bash
# BEFORE any edit
<test-runner> --list | sed 's/.*:://' | sort > /tmp/baseline.txt
wc -l /tmp/baseline.txt        # sanity-check it is non-empty

# AFTER
<test-runner> --list | sed 's/.*:://' | sort > /tmp/after.txt
diff /tmp/baseline.txt /tmp/after.txt && echo "INVARIANT HOLDS"
```

**Consumer files untouched** — the set of files that use the thing you moved
should not appear in your diff at all:

```bash
git diff --name-only <base>..HEAD | sort > /tmp/dirty.txt
comm -12 /tmp/consumers.txt /tmp/dirty.txt      # must be empty
```

**Line-coverage proof for a range move.** The failure mode is a line silently
dropped or duplicated between the source range and the destination. Prove every
source line is assigned exactly once and anything unassigned is blank — with a
script, never by eye. Note that naive brace-counting to find block boundaries
breaks when a closing brace sits on its own line; track whether a block has
opened before testing whether depth returned to zero.

**Citations survived.** On a boundary where comments cite an upstream source,
the multiset of citations should be unchanged by a pure relocation:

```bash
git show <base>:<file> | grep -c '<citation-marker>'
grep -rc '<citation-marker>' <new-files>        # totals must match
```

**Visibility not widened.** Every added `pub`/`export` in the diff needs a
compiler error that demanded it:

```bash
git diff <base>..HEAD | grep -E '^\+.*\bpub(\(| )' 
```
For each hit, try narrowing it and recompile. If it compiles narrower, it was
widened speculatively.

## The adversarial checklist

Run as someone trying to find the defect. The executor's report is a hypothesis,
not evidence.

- [ ] Gate re-run personally, exit code captured explicitly, not inferred
- [ ] Oracle diffed against a baseline that was validated as non-empty
- [ ] Relocated code confirmed byte-identical modulo indentation/imports
- [ ] For each folded duplicate: **original** re-read from history and compared
      against the helper, hunting the dropped guard or changed default
- [ ] No citation or field-layout comment deleted or reworded
- [ ] No visibility widened past what the compiler demanded
- [ ] No new catch-all arm swallowing future variants
- [ ] No conditional-compilation attribute dropped (these fail *only* on the
      platform you cannot build locally — check counts before and after)
- [ ] No user-visible string altered
- [ ] No files touched outside the stated remit
- [ ] Runtime-observable behaviour actually observed at runtime

## Runtime verification

If the change touches anything a user can see or a server can reject, a green
unit gate is not verification. Drive it.

Distinguish clearly between what you observed and what you inferred, and name
what you could **not** reach rather than quietly omitting it — an honest
"this screen was never exercised" is far more useful than an implied all-clear.

Two failure modes that will waste your time if you don't know them:

- **Environmental faults impersonate regressions.** A silent server, a stale VM
  mount, a saturated database — these produce symptoms that look exactly like
  the bug your change would have caused. Before concluding regression, check
  whether the same code passed before the environment changed, and diff what
  your change *actually* touched in that path.
- **A clean shutdown is not always a clean result.** A process can exit through
  its normal teardown path for external reasons, leaving a log that looks like a
  voluntary quit. Read the lines *before* the teardown for the cause.
