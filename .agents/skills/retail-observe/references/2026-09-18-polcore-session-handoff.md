# How polcore hands the session to FFXiMain (2026-09-18)

Static inspection only. No Square Enix binary was executed, patched or
modified; no traffic was sent to any production host. This record answers the
question the lobby login raises: where do retail's 16-byte packet hash and
64-byte authCode come from, and can a third-party client obtain them without
touching the viewer's process?

Builds inspected, from the registered `retail` install (`version.dat` reads
`1.18.00n`, no trailing newline). All are PE32 at image base `0x10000000`.

| Module | SHA-256 prefix | Size |
|---|---|---|
| `PlayOnlineViewer/pol.exe` | `5c2d45bd277e` | 1600512 |
| `PlayOnlineViewer/polhook.dll` | `e4ba164a0e3a` | 45056 |
| `PlayOnlineViewer/viewer/com/polcore.dll` | `f5af5837ffef` | 541696 |
| `PlayOnlineViewer/viewer/com/app.dll` | `bcddc16de7ff` | 4274688 |
| `FINAL FANTASY XI/FFXiMain.dll` | `f2245d1c9d06` | 2901584 |
| `FINAL FANTASY XI/FFXi.dll` | `9053d4106160` | 93251 |

## The channel is in-process COM

`polcore.dll` is an in-process COM server. Its only exports are the four ATL
entry points (`DllGetClassObject`, `DllRegisterServer`, `DllCanUnloadNow`,
`DllUnregisterServer`). The class is `CPOLCoreCom` implementing `IPOLCoreCom`
(RTTI `.?AVCPOLCoreCom@@`, `.?AUIPOLCoreCom@@`, ProgID `POLCore.POLCoreCom.1`).
Registration is `InprocServer32` with `ThreadingModel=Apartment` for all three
region CLSIDs: `{07974581-0DF6-4EF0-BD05-604B3ADA9BE9}` worldwide,
`{E5966FB3-...}` US, `{3501F5DD-...}` EU. There is no `LocalServer32` and no
marshaling AppID, so the authenticated object exists only inside the process
that ran the handshake. A `CoCreateInstance` from another process returns a
fresh, unauthenticated object.

`polcore` carries the classic function-table export string
`GetCommonFunctionTableWW` at VA `0x10407340` and `0x10407960` (packed data;
"WW" is the worldwide build). This is the table of polcore function pointers
the game calls back through.

`FFXiMain.dll` is the COM client and is itself a COM server ("GameMain Class",
CLSID `{1027DC46-750D-4B1F-8834-1D25B8BEBAB8}`). It imports `ole32`
(`CoCreateInstance`, `CLSIDFromString`, `CoInitialize`, `CoSetProxyBlanket`)
but does not embed polcore's CLSID. Inference, not observation: `pol.exe`
constructs both objects and passes the live interface or function table into
the game at init, and the game fetches the session through it.

`polhook.dll`'s 16-byte writable `SHARED` section (`.shared_`, VA
`0x1000C000`, virtual size `0x10`) is `SetWindowsHookExA` window-hook state
(`CPolHook`, `MakeHook`, `UnHook`, USER32 hook imports). It carries no auth
data, which settles the earlier open question about that section.

## What the game fetches, and where it lands in the 0x26

`research/XIClient` stubs the same call sequence with the fetches marked.
`LoginStateMachine::HandleLogin` holds a `char[0x40]` it feeds to
`ntTcpDLLSetAthCode`, annotated "get authcode", and a 16-byte buffer for
`ntTcpDLLSetPasswd`, annotated "get random value from pol". Earlier states
(`HandleStart`, `HandleCreateClient`) are annotated "something from pol": the
polcore state machine must succeed first. Those setters populate the lobby
session context, and `ntTcpDLLRequestLobbyLogin` builds C2S `0x26`.

The header XIClient declares is `{u32 length; u32 tag; u32 opcode; u8
hash[16]}`. **The 16 bytes at offset `0x0C` are a per-packet MD5
authenticator** (`ntLoginHashPacket`), not an account identity. LSB reuses that
field to carry its own session hash, which is why Kuluu's LSB path fills it
with the auth server's `session_hash`; retail computes it per packet. The
64-byte field at `0x34` is the authCode polcore builds. Version string at
`0x74` and client expansions at `0x84` match what Kuluu already sends.

## Can Kuluu obtain the session without touching SE processes?

No out-of-process handoff exists to read. There is no named file mapping, no
file under `usr/` or `polcfg/`, no registry value, no command line and no
`WM_COPYDATA` that carries the authCode. Three postures follow:

1. **Host the genuine, unmodified `polcore.dll` in a Kuluu-launched process
   and drive it over COM.** Injection-free and patch-free: Kuluu takes over
   `pol.exe`'s role using Square Enix's own binary as a library, then reads the
   authCode from the instance it authenticated. Cost: Kuluu must reimplement
   the login flow that drives polcore, which is the profile and chat handshake
   mapped in the 2026-09-16 record. Windows or Wine with COM only.
2. **`ReadProcessMemory` against an already-running `pol.exe`.** No code
   injected and no byte patched, but a foreign-process memory read is a
   different posture from (1), and it is pinned to one build's offsets.
3. **`CoCreateInstance` in Kuluu's own process.** Useless. The inproc-only
   server returns an unauthenticated object with no session.

xiloader is not a model for any of these. It hosts `FFXiMain` itself and either
mints its own authCode for private servers or hooks polcore's function table.
It never obtains a real Square Enix session.

## Unverified

- The `pol.exe`-constructs-both-objects claim is inferred from the registration
  shape and from FFXiMain not embedding polcore's CLSID, not from a call site.
  `polcore.dll` is POL1-LZSS-packed on disk, so its code was not disassembled.
- Which function-table slot returns the authCode and which returns the 16-byte
  random value. Unpacking polcore and disassembling the table builder would
  settle it.
- The account id the data server reads from `0xA1` at offset 1. XIClient's
  `HandleTag0xA1` is unimplemented and the value is not in these binaries'
  strings.
- Retail's byte-exact `0x26` field map rests on XIClient field order and
  XiPackets prose, not on disassembling FFXiMain's builder. XIClient targets a
  modified server and writes only 16 bytes at `0x34`, so it is not retail truth
  for that field.

## Correction, later the same day: the COM surface has no login in it

The unpacked `polcore.dll` `f5af5837ffef` (with the `73b1864bf522` build's
decompilation for the deep routines) settles two of the items above, and the
answer is the opposite of what the first pass assumed.

**`GetCommonFunctionTableWW` is not an export and not an auth table.** The
string lives only in the `.rsrc` MIDL type library (VA `0x104093xx` unpacked;
the earlier `0x10407340` / `0x10407960` were its packed on-disk positions).
It names slot 11 of the `IPOLCoreCom` vtable, and that method is a setter: it
maps its region argument to 0/1/2 and stores the caller's pointer into a
global input-callback array (core `0x100452b0` in `f5af5837ffef`). The reader
of that array is `GetWindowsType` (slot 10). Nothing in its reach carries
auth material.

**`IPOLCoreCom` is a dual (IDispatch) interface of 29 helpers**, vtable at VA
`0x10063874` in `f5af5837ffef` (`0x10065874` in `73b1864bf522`), the object's
offset-8 interface with 36 code slots. The type library names them:
`GethInstance`, `GetlpCmdLine`, `SetParamInit`, `GetWindowsType`,
`GetCommonFunctionTableWW`, `PolViewerExec` (spawns the viewer thread),
`GetWindowsVersion`, `PressAnyKey`, `PolconSetEnableWakeupFuncFlag`,
`CreateInput`, `UpdateInputState`, `GetPadRepeat`, `GetPadOn`, `FinalCleanup`,
`PaintFriendList`, `CreateFriendList`, `DestroyFriendList`,
`SetMaskWindowHandle`, the four `Get*RegKeyName*` accessors, `SetAreaCode`,
`GetAreaCode`, and the three mask-window methods. There is no method that
takes a member name or password, none that reports login progress, and none
that returns the 64-byte authCode or the 16-byte value. `app.dll` embeds
`IID_IPOLCoreCom` but no polcore CLSID; `pol.exe` creates the object and hands
it over. Exports remain the four ATL entry points only.

**The member login is polcore-internal.** The profile-login state machine
(`0x1001e5d0` in `73b1864bf522`) builds a 0x40-byte body (`0x1001e760`) from
obfuscated credential globals and a minute-rounded timestamp and sends it as
category 4 / opcode 7 to `pp%03d.pol.com`:51220. That body has the shape
XiPackets ascribes to the authCode, and it lives in process globals at fixed
VAs (profile-context array base `0x10404ad0`, 0x338 bytes per slot, buffer
pointer at slot+0x328; credential globals `0x100aaaac`, `0x100aaaad`,
`0x100aaac1`; chat key at `0x100aa858`, all in `73b1864bf522`, roughly
`-0x2000` in `f5af5837ffef`). Whether it equals the value FFXiMain writes at
C2S 0x26 offset 0x34 is not established: `pol.exe` is packed and was not
unpacked, and polcore carries no ffxi, lobby or ntTcp string.

### What this does to the three postures

1. Hosting the genuine `polcore.dll` over COM cannot reach a finished session:
   the interface has no login surface. Driving polcore below the COM layer
   means reimplementing the profile and chat handshake, which is the
   "reimplement PlayOnline login" posture, not a shortcut past it.
2. Reading the fixed-VA globals out of an already authenticated viewer with
   `ReadProcessMemory` remains injection-free and patch-free and is now the
   only known producer. It is gated on one dynamic observation this record
   does not contain: that the bytes at those globals match the authCode the
   game then sends. That check reads the player's own processes on the
   player's own machine and needs no traffic from Kuluu.
3. `CoCreateInstance` in Kuluu's own process is still useless.

The full slot table, with both builds' VAs and the observed/inferred split
per claim, is in the working notes under `artifacts/polre/`; this section
carries the conclusions.
