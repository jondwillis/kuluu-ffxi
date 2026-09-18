---
name: retail-grounding
description: Ground FFXI vanilla client behavior in available retail observation, binaries and DATs before implementing or reviewing parity changes. Use this whenever the correct behavior depends on the original client -- rendering, animation, camera, UI, input, collision, or client file-format semantics -- including when the request sounds like ordinary implementation work and never mentions retail or parity. Works from whatever retail client is reachable on this machine (native, Wine or a VM) and from the installed binaries and DATs when none is running. Skip it for product-only tooling and for purely server-side protocol work.
---

# Retail grounding

Ground implementation choices in the best available evidence about the original
client. When behavior remains uncertain, identify the inference and continue
within the authorized scope. A plausible community implementation or a desirable
visual result is not by itself a verified vanilla specification.

## Choose evidence for the question

Read [research/AGENTS.md](../../../research/AGENTS.md) for the source ranking.
Existing bead citations and dated retail observations are useful starting
points: check that they cover this behavior and still apply to the relevant
client build. Reuse sufficient evidence rather than repeating an investigation.

- **Visible behavior or timing:** use [retail-observe](../retail-observe/SKILL.md)
  to drive the original client, on whatever host runs it here. Capture a
  comparison that separates competing explanations, with relevant model, pose,
  camera and client settings recorded. Account for private-server differences,
  injected addons, graphics mods and the compatibility layer itself before
  calling a measurement vanilla: a frame rate or a shadow observed under Wine or
  in a VM is evidence about that stack too, so prefer measurements that a
  translation layer cannot plausibly change, or confirm the same result on a
  second host.
- **Exact computation, predicates or format bits:** inspect the relevant retail
  binary/DAT and trace the caller, branch and data provenance. XIClient is the
  preferred community map into that code; its names and comments are not proof.
  Read [binary inspection](references/binary-inspection.md) when this route is
  needed. A screenshot can establish the output without uniquely identifying
  the algorithm; a DAT value alone does not establish how the client uses it.
- **Server-owned wire values or transitions:** use
  [lsb-mirror-check](../lsb-mirror-check/SKILL.md). LSB defines what our server
  sends; it does not establish how retail renders or interprets every field.

Check availability before planning a live drive or assuming a reference clone
is populated. A client you cannot launch right now -- a stopped VM, a Wine
prefix that will not open a window, no install for the era in question -- does
not make local binary/DAT inspection unavailable; those files are on disk
regardless of whether anything can run them.
If a source cannot be used, continue with the best available evidence and say
which claim remains an inference. Do not make unrelated work wait for a client
login, exhaustive reverse engineering, or optional tooling. Follow existing
authorization for live actions; this skill adds no approval gate.

## Turn the evidence into code

Trace the rule end to end: where the input comes from, its units and coordinate
space, scale/transform order, special-case predicates, and behavior while data
is missing or changing. Look beyond the similarly named helper. A static
locator and an animated bone position can share a name yet obey different rules.
Likewise, a server hitbox, rendered bounds and a camera anchor need not coincide.

Implement the supported behavior independently, citing the decisive source at
the boundary. Read authored values from DATs at runtime; use
[vendor-scrape](../vendor-scrape/SKILL.md) for build-time LSB tables. Do not turn
measured model examples into hand-maintained constants. Keep explicitly chosen
enhancements distinct from the vanilla baseline without expanding their scope.

Validate the distinction that motivated the change, not just the new helper's
arithmetic: vary a model, pose, scale, missing-resource state or branch that
would make the old hypothesis fail. For observable client changes, use
[verify](../verify/SKILL.md) to compare Kuluu against the established reference
when available. Compilation, source inspection and live comparison establish
different things; report which actually ran.

Record the conclusion, decisive citations, evidence paths and remaining
uncertainty in the working bead. Durable records of retail behavior belong in
`retail-observe/references/` under the repository policy; captures and binary
dumps stay local. Update stale bead assumptions when stronger evidence changes
the target. Keep unverified acceptance criteria open rather than claiming
parity from a passing build.
