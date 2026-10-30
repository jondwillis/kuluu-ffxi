# Release artifact verification across hosts

Use the runtime available on the current host. Discover its prerequisites before starting a check.
Record unavailable capabilities as coverage gaps. Continue independent checks for other artifacts.

| Artifact | Runtime selection | Passing evidence |
|---|---|---|
| macOS | Compatible macOS host and CPU architecture | Archive hashes, signatures, extraction, and executable launch |
| Linux | Compatible Linux host, or an available container engine with the required architecture | Archive hashes, extracted executable, required libraries, and executable launch |
| Windows | Wine on macOS or Linux; native execution on a compatible Windows host | Archive hashes, executable architecture, runtime identity, command, and exit status |
| Browser | Available browser with the bundle's graphics capabilities | Loaded artifact hashes, inspected rendering, and applicable relay interaction |

Executable launch is a packaging check. Verify affected gameplay through its production path separately.
Compilation and archive inspection do not prove executable launch.
Keep each architecture separate. An x86_64 result does not verify an ARM64 artifact.

## Windows through Wine

Discover Wine with `command -v wine`.
If the host uses a wrapper, use its configured runner path.
Run the selected executable with `--version` before using a prefix.
An executable on PATH can still be incompatible with the host CPU.
If this prerequisite fails, record the diagnostic before attempting an artifact launch.
An absolute runner path is host configuration. Do not copy it into shared instructions.
Use the same runner for Wine, its prefix, and any Wine utilities.

Use an isolated verification prefix or an explicitly selected existing test prefix.
Do not modify a player's prefix to check an archive.
Set `WINEPREFIX` for that process. Keep the prefix path in local configuration or the evidence record.
Run the extracted candidate executable with `--help` through the selected runner.
Use a bounded process timeout. Save standard output, standard error, and the exit status.
Record the Wine version and executable hash.

Inspect the PE architecture before selecting a runtime.
If the runner cannot execute that architecture, keep the check blocked.
Do not assume that x86_64 Wine can run the ARM64 Windows artifact.
Do not replace this check with a requirement for a named Windows VM or external disk.
Label successful results as Wine compatibility evidence.
Keep any native Windows requirement separate and explain why the candidate needs it.

Retail client observation also needs an installed client, a compatible graphics path, and applicable host permissions.
Use the retail-observe skill's runner profile and `doctor` command for that work.
A successful Kuluu `--help` check does not establish retail client compatibility.

## Linux

Prefer native execution when the host architecture matches the artifact.
Otherwise, inspect available container engines and their architecture support.
Use the configured Docker or Podman command. Check its runtime before creating a task container.
Copy extracted artifacts with the engine's copy command when shared host paths are unavailable.
This avoids assumptions about macOS bind mounts or private volume names.

Use an image with the required runtime libraries. Retain its image identity and setup commands.
Run the extracted executable with `--help`. Record its hash, architecture, output, and exit status.
Remove only the container created for that check.
Do not restart a shared engine or VM to obtain architecture support without applicable authority.

The deck-build-sync skill describes a configured Apple Silicon and Steam Deck delivery workflow.
Its Rosetta and Syncthing prerequisites are specific to that workflow.
They are not prerequisites for every Linux release check.

## macOS

Check executable and bundle signatures after extraction.
Use the host's native archive tool when archive metadata affects signature checks.
Record the OS version, CPU architecture, minimum supported OS, and executable hashes.
Use a configured remote host only when local execution cannot cover the target.
Discover its connection and paths from local configuration.

## Browser and live gameplay

Discover available browser automation before selecting the drive procedure.
Record the bundle hash, browser version, graphics backend, and inspected captures.
State whether a matching relay supplied live snapshots.
Keep HUD startup, relay connection, and gameplay results separate.

Local LSB helpers can have host-specific prerequisites.
The current `scripts/lsb-stack.sh` assumes Colima and configured container names.
Inspect those requirements before using it on another machine.
Use an applicable existing stack or server configuration when those requirements are absent.
Do not present that helper as a universal server installer.
