# Getting a client running, against whichever server

The retail client is just a directory of files plus a loader that starts it and
tells it where the server is. Nothing about observing it depends on a particular
server or a particular private-server launcher, so treat those as two
independent choices:

- **which client install** -- a retail install, or a private server's install
  (retail files plus an overlay: Ashita, a bootloader, addons);
- **which server** -- a private server such as HorizonXI, or a local
  LandSandBoat stack you run yourself.

The interesting pairing is a **retail client against a local LSB stack**: the
oracle and the remake then face the same server, so a behavioral difference is
the client's, not the server's. That is the comparison worth setting up when a
parity question is subtle.

## Finding an install

Ask, do not assume a path. This repository keeps a registry of named installs:

```bash
kuluu install list          # each install with its KNOWN_CLIENTS row
kuluu install path NAME     # the directory to use
```

`FFXI_OBSERVE_INSTALL` accepts either a directory or `kuluu:NAME`, which asks
that registry. A macOS/Linux wrapper (FFXI on Mac, Whisky, a Proton prefix)
keeps its own per-world folder -- point at that directory the same way. The
checkout itself ships no game files, and captures of retail DATs are
SE-copyrighted reference material that stays local.

A usable install contains the retail tree (`SquareEnix/FINAL FANTASY XI` with
`FFXiMain.dll` beside `VTABLE.DAT`, and `SquareEnix/PlayOnlineViewer`), plus
whatever overlay the server needs.

## The loaders, and what each one is for

| Loader | Role |
|---|---|
| `pol.exe` / `polboot.exe` | the PlayOnline viewer boot path, as retail ships it |
| `xiloader.exe` | the community loader: takes a **server address** and starts the client against it. This is the decoupling point |
| `Ashita-cli.exe` + a boot config | injects Ashita, then starts the client. Private servers that ship addons use this |
| a server-branded loader | wraps the above with the server's own ticket/session flow |

`xiloader` is the one that makes the client server-agnostic. Its options are
self-documenting (`xiloader --help`) and include `--server`, `--serverport`,
`--dataport`, `--authport`, `--lang`, plus a hairpin fix for reaching a server
through your own NAT. `observe.sh launch --server HOST` appends `--server`,
which assumes an xiloader-style loader; for `pol.exe` or a branded loader, put
whatever it needs in `loader_args` instead.

## Against a local LandSandBoat stack

```bash
FFXI_OBSERVE_INSTALL=kuluu:NAME \
FFXI_OBSERVE_LOADER=_bootloader/xiloader.exe \
scripts/observe.sh launch --server 127.0.0.1
```

Bring the stack up with the [verify](../../verify/SKILL.md) skill -- it owns the
recipe, including the fact that the map server must advertise an address the
client can actually reach. Credentials for a local stack are throwaway test
accounts you own, so an agent may drive that login end to end; `xiloader` also
prompts in its console if you would rather not pass them.

Two things to get right before blaming the client:

- **Ports.** Non-default auth/lobby/data ports on the server need the matching
  `--authport`/`--serverport`/`--dataport` here. The repository's own constants
  are the source for those values; do not re-type a port literal you can import.
- **Client era.** A server expects a client of a particular vintage, and the
  installed build is a fact you can check: `kuluu install list` names each
  install's `KNOWN_CLIENTS` row (see `KNOWN_CLIENTS` in
  `ffxi-dat/src/client_profile.rs`). A mismatch shows up as handshake or packet
  surprises, not as a clear error, and it also means an address or offset noted
  for one build does not transfer to another.

## Against a private server

Follow that server's own launcher: it usually authenticates, patches, then hands
a short-lived session to its loader. Two standing rules:

- **Credentials are the human's.** Someone else's server, someone else's account
  -- hand off rather than typing stored secrets, and never store them in a
  profile. Once a launcher session persists, a re-login usually needs no
  credentials at all, which is why switching characters is fully drivable.
- **Do not fight a ghost session.** A crashed prior session can hold the slot for
  minutes; wait it out rather than logging out of a session you cannot restore
  without the password.

## Client configuration traps

The client reads resolution and graphics settings from the Windows registry --
the guest's registry in a VM, the prefix's registry under Wine -- which is why
installs ship registry-seeding tools and why the settings survive reinstalling
the game files.

- **Background resolution too high opens no window at all.** 4096 squared wedged
  the loader; 2048 squared works. Set it in the launcher or FFXI Config before
  suspecting the observe tooling.
- **Run the client windowed** while driving it. Fullscreen exclusive mode makes
  window-targeted capture unreliable on every host.
- The standalone config tools (FFXI Config, gamepad config) are separate Win32
  windows with their own titles; drive them with `FFXI_OBSERVE_WINDOW_TITLE`, and
  be aware that on some hosts they ignore pid-targeted input (see the host
  reference).

## After launch, prove it is really up

```bash
scripts/observe.sh status     # does a window resolve, and where
scripts/observe.sh ocr        # what is actually on screen right now
```

A window that resolves is not the same as a client that is responding: a
mid-load client, or a Wine app orphaned from its server, still composites a
window and still captures. `ocr` twice with an input in between, and treat "no
change at all" as a stalled client rather than a missed keypress.
