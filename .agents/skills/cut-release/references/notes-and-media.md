# Release notes, media, and announcements

## Write the change summary

Review the full range from the previous published release to the candidate:

```bash
git log --no-merges --format='%h %s' vPREVIOUS..CANDIDATE
```

Group changes by player-visible result. Use commit details to establish the actual change.
Do not turn the notes into a commit list or repeat the same change from merged histories.
Describe what changed since the previous release.
Omit unchanged requirements, supported-platform lists, and instructions already in the README or earlier releases.
Include a compatibility instruction only when this release introduces a specific change that requires it.
Keep known limits specific to the described behavior. Avoid generic warnings repeated in every release.

Use the user's writing style when examples are available.
Keep thanks and contributor credits factual. Do not infer contributor identities from agent-generated commit authors.

## Select existing media before capturing more

Search relevant workspace artifacts and user-identified Downloads files.
Inspect selected images. Inspect representative video frames and listen before making audio claims.
Do not inspect unrelated personal files merely because they are in Downloads.

Record each selected file's source, hash, build or commit, scene, and observed result.
Explain older-build or fixture limits in captions.
A developer animation room demonstrates its effect or motion; it does not establish live combat or original-client timing.
A controlled HUD fixture demonstrates the displayed UI; it does not establish a complete live transaction.

Use fresh captures when existing media cannot show the release change.
Use the verify skill and the actual packaged artifact when practical.
Prefer captures without private paths, credentials, or unrelated chat.
An OS window capture can avoid game screenshot notices in chat.
Keep verification pixels unchanged. Stop after a bounded capture failure and record the diagnostic.

## Prepare the GitHub release

The release page owns the release notes and media. A PR can link there instead of duplicating uploads.
Use an existing draft when available. Inspect tag state before and after creating a draft.
Keep the draft target tied to the intended candidate; update it when the approved merge commit changes.

Use `gh release edit --notes-file` for the reviewed body.
Use `gh release upload` with explicit selected files.
Never upload a whole artifact directory; it can contain private fixture data.

Draft asset URLs can contain an unstable `untagged-...` path.
Use stable final links in the notes:

```text
https://github.com/jondwillis/kuluu-ffxi/releases/download/vX.Y.Z/ASSET_NAME
```

Preview selected media locally while the release is a draft.
Place each image or video link beside the feature it demonstrates. State the scene and evidence limits briefly.
Do not assume Markdown image syntax embeds a playable video. Use a video link or a linked poster image.

Inspect the publication action's behavior before relying on draft preservation.
Generated release notes can append a raw change list to the prepared body.
After publication, compare the actual body with the reviewed file.
If necessary, reapply the reviewed body with `gh release edit --notes-file`.
Verify that media survived publication. Check each final public URL without authentication and compare file hashes.

## Announce only when requested

Publication does not authorize a Discord post or another external announcement.
When requested, confirm the server and channel from current application state.
Write for players. Describe what they can see or do differently.
Replace internal terms with concrete results. For example, describe clearer menus instead of a HUD fixture.
Keep test status, comparison plans, bead references, and development-process notes out of announcements.
If a limitation affects players, describe the visible problem and any useful action in plain language.
Use a short summary of new changes and link to the full release notes.
Include selected screenshots and short videos with feature captions when the release has visible changes.
Prefer native attachments or verified playable embeds over a page link alone.
Show more than one relevant feature when suitable inspected media exists.
Check that images render and videos have playback controls in the posted message.
Keep test-scene context brief and factual; do not present an animation-room clip as live combat.
Do not repeat README setup instructions or unchanged information from previous releases.
Do not add mass mentions unless requested.
Prefer an available connector. Otherwise, use the signed-in application through computer use.
Verify that the message was posted. Correct existing text by editing it instead of sending a duplicate announcement.
If an existing message cannot accept attachments, add one captioned media reply beside it.
