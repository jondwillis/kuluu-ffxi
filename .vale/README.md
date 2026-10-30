# Prose checks

Use Vale 3.24.0 with the rules in this directory.
Install the matching binary from the [Vale releases](https://github.com/vale-cli/vale/releases/tag/v3.24.0).
Check its archive against the release checksum file before use.
Set `VALE` to the binary path if Vale is not on `PATH`.

Check a document:

```sh
python3 scripts/check-prose.py path/to/document.md
```

Check a draft reply or commit message from standard input:

```sh
printf '%s\n' 'The checks passed.' | python3 scripts/check-prose.py
```

For instructions, add `--procedures` to select the 20-word limit.
The default limit is 25 words per sentence.
For mixed text, check instruction sections separately with `--procedures`.
Warnings return a nonzero exit status. Review them and correct the prose before use.

These rules support the ASD-STE100 guidance in `AGENTS.md`.
They check sentence length and a small set of word choices.
They do not implement the full approved dictionary, grammar rules, or semantic review.
Vale token counts can differ from the standard's counting rules.
Preserve exact text and technical meaning when you review a warning.
Markdown code blocks and inline code are excluded by Vale.
Use a blockquote for an exact quotation; quotation handling needs human review.

The check reads complete input, not changed lines.
Run it on new prose or the sections you edit to avoid unrelated historical warnings.
It does not run on every existing document in CI.
The reminder hooks do not intercept replies or run Vale automatically.

Keep the existing `readme`, `comments`, and `records` checks.
They check repository scope, local links, source citations, and binary provenance.
Those rules need repository context and are not replaced by these Vale rules.
No existing prose gate was removed.
