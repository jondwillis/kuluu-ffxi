# PlayOnline in-house login: the whole account handshake (2026-09-19)

Static inspection of the PlayOnline Viewer and FFXi binaries of a legally
installed retail client. No Square Enix binary was executed, patched, or
contacted; no production host was resolved. This record is the durable map;
the byte-exact detail lives in the `ffxi-pol` crate, where each routine cites
the polcore build it was read from, and in the git-ignored working notes under
`artifacts/polre/inhouse/`.

Builds: polcore.dll `73b1864b` (viewer 1.18.15e, the decompiled build),
cross-checked against `f5af5837` (1.18.00n, retail tree); app.dll `7ba99828`;
pol.exe `5c2d45bd`; FFXiMain.dll `f2245d1c`; FFXi.dll `9053d410`. All were
POL1-LZSS-packed on disk and unpacked for reading. The Viewer's on-screen
text is not in any of them: it lives in `viewer/data/common/StringTable.bin`,
a `PEX\0` LZ77 container whose 5515 entries are indexed by the message ids
app.dll passes around. Decoding it is what turned several structural guesses
below into quotations.

## Why this exists

Retail FFXI has no auth server of its own. The PlayOnline Viewer signs the
account in over two services and hands the game a 16-byte value and a 64-byte
authCode that the lobby validates. Kuluu once read a session a producer wrote
to a file beside the Viewer; that handoff is gone. Kuluu performs the account
handshake itself, the same way the Viewer does: the PlayOnline auth flavor of
`kuluu_session::auth_client` takes the member name and password and runs
`kuluu_session::pol_inhouse`. It adds reach, not power: the player
authenticates their own account against Square Enix, and someone without a
paid account gains nothing.

## What the player has to supply

An account carries two identities and the handshake uses both, which is what
the Viewer's own login form asks for (the labels here are its string table's):

| Field | Width | What it authenticates |
|---|---|---|
| **PlayOnline ID** | 8 characters, four capitals then four digits, with the first character a check letter over the rest | the connection: it is the chat `NICK` handle, the profile-service identity, and where the profile host index is carried |
| **PlayOnline Password** | at most 15 characters | the same: it is the `NICK` digest's second input and the profile authenticator's secret, both verbatim |
| **Square Enix ID** | at most 16 characters | the member: it is the login transaction's name |
| **Square Enix Password** | -- | the same: `SHA1(hex(SHA1(password)) || "playonline")` is what the login proves |
| One-Time Password | exactly 6 characters | only for an account with a security token |

The PlayOnline ID is the 8-character code the Viewer shows on a service
account line, with the region beside it; both halves render out of one packed
64-bit identity word. A **Content ID** is something else entirely: a six-bit
ordinal the Viewer labels `Content ID-N`, which is what the world select
carries. The Viewer's **Member Name** is a display nickname that no
transaction sends.

## The two services

- **Chat service** (IRC dialect, TCP 51240/51241/51242). Establishes the
  session Blowfish key by an ephemeral-RSA exchange. Ported in
  `ffxi-pol::{rsa, chat, crypto}`.
- **Profile service** (fixed-frame, TCP 51220 on `pp%03d.pol.com`). Carries the
  member login and the transactions that yield the lobby session. Its frames
  are enciphered with the Blowfish key the chat service agreed. Ported in
  `ffxi-pol::{profile, authcode}`.

## The ordered handshake

1. **Chat key agreement** (`ffxi-pol::transport::agree_session_key`): connect,
   read the greeting (which carries a challenge and a clock-sync word), send
   `USER` with the client's 256-bit RSA modulus in the realname, read numeric
   300 and RSA-decrypt the session Blowfish key from it, then `NICK` and read to
   numeric 422 (which the client treats as login complete). The stream cipher is
   on from numeric 300; each IRC line reciphers from an IV seeded by the
   client's own modulus.
   After registration a **second numeric 300** arrives carrying a 0x18-byte
   routing struct in base32 rather than a key in base64. Its 7-bit host field
   is the profile service's host number, so the profile host is assigned by
   the chat service and is not derivable from the member id.
2. **Profile member login** (category 4, opcode 7): a 0x40-byte body of the
   mode, the Square Enix id, and a SHA-1 over the hex of the 20-byte secret
   and the minute-rounded unix time. Proves possession; registers nothing.
3. **World / service select** (category 4, opcode 6): its 0x80-byte reply is
   not an acknowledgement. polcore writes the leading sixteen bytes into the
   globals the next request reads back as its defaults, so the reply is how a
   selection is confirmed: the content ordinal, whether its service is
   available, which of its eight service slots was chosen, the context flag,
   the region and the world index.
4. **Enter community** (category 4, opcode 5): carries the confirmed
   selection; its 0x20-byte reply is the only server-issued entropy in the
   whole session.

Category 2 / opcode 3 is **not** part of a login. An earlier record called it
the content-id list; app.dll builds friend records from its reply, so it is
the friend and ignore lists.

## The lobby session is assembled client-side

The community reply becomes the lobby values by a transform that needs no
further server participation (`ffxi-pol::authcode`):

- the **16-byte value** is the reply's first sixteen bytes;
- the **64-byte authCode** is a scratch of `MD5(reply[0:16] || clock ||
  chat_key)`, the 20-byte POL address struct, and two derived head bytes,
  enciphered with polcore's `*5`-chain cipher (an 8-byte key baked into the
  DLL), then run through a forward XOR chain and four head fixups. polcore fills
  52 of the 64 bytes; the retail client leaves the rest uninitialised.

So the session cannot be minted offline (the op5 reply is server-issued), but
once that reply is in hand the assembly is entirely client-side over state the
client already holds. This is the decisive fact: a faithful client that
completes the real handshake can produce the lobby session without any Viewer
process and without reading another process's memory.

## The hosts

Every host name is a literal in the binaries; nothing is configured by the
player in the Viewer, and Kuluu's PlayOnline profile needs only the lobby.

| Service | Host | Where it is read from |
|---|---|---|
| chat (IRC dialect) | `ci000.pol.com`, ports 51240/41/42 | app.dll `7ba99828` writes this literal into the chat-host field of every member record it creates (`0x101a2695`, `0x101ada50`, `0x101ae280`), and polcore resolves whatever that field holds. The `pc%03d%s.pol.com` template an earlier record ascribed to the chat service is the **patch** server, whose fallback is the config key `PATCH_SERVER_DOMAIN`. |
| profile | `pp%03d.pol.com`, port 51220 | polcore `0x10075430`; the index is bits 9..15 of the member identity's high dword, which the chat service assigns in the post-registration routing struct. |
| FFXI lobby | `ffxi00.pol.com`, ports 54230/54001 | FFXiMain.dll of the retail-2026-09 row (sha256 f2245d1c9d06e02c): the string at VA 0x10362044 is resolved from VA 0x100ed84d when the connection-mode global at VA 0x104ca430 is 0. Modes 1 and 2 are development paths (`ci000.pol.com` sits beside a `172.16.x` address and `c:\image\ffxi\serv`). |

So a PlayOnline profile's Host is the FFXI lobby server, and `ffxi00.pol.com`
is the game's own default for it (`ffxi_pol::hosts::LOBBY_HOST`); the auth
port is unused because the account services replace the auth server. The
world-select reply carries no lobby address: the two dwords it leaves at
offset 8 have no reader anywhere in the module, so the game's own default
stands.

## The Viewer-to-game seam (for the COM posture)

FFXiMain reads the finished session through a polcore function table, not a file
or a shared section. `IPOLCoreCom` slot 7 `GetCommonFunctionTableWW` is a
getter returning that table; FFXi.dll hands it to FFXiMain, which calls
table+0xEA0 for the authCode bytes and table+0xFAC for the 16-byte value. This
is why hosting the genuine unmodified polcore over COM is a viable
injection-free posture as well.

## What a refusal means

The profile service answers a bad request with a status byte, which polcore
turns into a client code and app.dll turns into a message out of the Viewer's
own error resource. Two of those statuses share one sentence on screen and
must not be collapsed:

| Status | Meaning |
|---|---|
| `0x6E` | the address is blocked from the account service |
| `0x6F` | the Square Enix id, password or one-time password was refused |
| `0x70` | the Square Enix account itself cannot sign in: closed, withdrawn, or in arrears |
| `0xE2` | the account service could not certify the account |

The Viewer shows the same "incorrect id or password" sentence for `0x6F` and
`0x70`, and only the untranslated internal label of the error resource tells
them apart. A client that reports `0x70` as a bad password sends the player to
re-type something that was never wrong. `ffxi_pol::profile::status_meaning`
describes each in our own words rather than reproducing the Viewer's text.

## The lobby dialect is a separate problem

Kuluu's lobby client is LandSandBoat-shaped: two sockets opened up front, a
`0x26` login on one and a `0xA1` character-list request on the other. Retail
does none of that.

- **One socket, not two.** The lobby client embeds a single TCP connection and
  declares one in its group descriptor. Every WS2_32 reference in the image
  falls into four clusters, and only one of them is the lobby. The map and
  search sockets are created later, from addresses a lobby reply supplies.
- **It connects late**, after a deliberate disconnect, and before polcore is
  asked for the session.
- **The port is a compile-time default**, not the ports a LandSandBoat profile
  carries. The address comes from polcore resolving the lobby host, which
  FFXiMain renders back to a dotted quad and hands to its TCP layer.
- **The ordered exchange** is connect, then client `0x26`, server `0x05`
  carrying a key, client `0x1F`, server `0x20`, client `0x24`, server `0x23`.
- **The character list is server `0x20`** on that same socket, in reply to
  `0x1F`, as a count followed by fixed-size records. There is no `0xA1`
  anywhere: a byte-level scan of FFXiMain, both polcore builds, pol.exe and
  FFXi.dll finds no store of that tag, and FFXiMain's only lobby builder
  dispatches over a table that does not admit it. The `0xA1` is LandSandBoat
  lineage and its account id is that server's own account row, learned over
  the auth port the PlayOnline flavor does not use.
- **The two session values go to different places.** The 64-byte authCode sits
  in the `0x26` at offset 0x34, which Kuluu already does. The 16-byte value is
  not in the `0x26` at all: it is held and sent from `0x1F` onward, enciphered
  under the key the server's `0x05` carries, ratcheting after each packet.
- **Every packet carries an MD5 over itself** with a span of its own header
  zeroed, and retail verifies it on receive.

So a finished account handshake is necessary but not sufficient. Reaching the
retail lobby needs a second dialect beside the LandSandBoat one, not a tweak
to it.

## What is not yet pinned

- Live wire compatibility. Every test vector in `ffxi-pol` is self-derived from
  the static reading and pins the reading, not interoperability. Verifying
  against Square Enix contacts their servers with a real account and is the
  player's own action, out of scope for automated runs.
