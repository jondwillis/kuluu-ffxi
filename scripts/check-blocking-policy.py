#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="kuluu-blocking-policy-") as directory:
        project = Path(directory)
        (project / "src").mkdir()
        manifest = project / "Cargo.toml"
        manifest.write_text('[package]\nname = "blocking-policy-check"\nversion = "0.0.0"\nedition = "2024"\n')
        environment = os.environ.copy()
        environment["CLIPPY_CONF_DIR"] = str(root)
        environment["CARGO_TARGET_DIR"] = str(project / "target")

        def check(source):
            (project / "src/lib.rs").write_text(source)
            return subprocess.run(
                ["cargo", "clippy", "--quiet", "--offline", "--manifest-path", str(manifest), "--", "-D", "warnings", "-A", "clippy::disallowed_methods"],
                env=environment, capture_output=True, text=True,
            )

        denied = check('''
#![deny(clippy::disallowed_methods)]
use std::thread::{sleep as wait, park, JoinHandle};
pub fn blocking(worker: JoinHandle<()>) {
    wait(std::time::Duration::ZERO);
    park();
    worker.join().unwrap();
}
''')
        if denied.returncode == 0 or not all(name in denied.stderr for name in ["disallowed_methods", "sleep", "park", "join"]):
            raise SystemExit("Blocking policy did not reject all forbidden calls.\n" + denied.stderr)
        permitted = check('''
#![deny(clippy::disallowed_methods)]
pub fn path_join(path: &std::path::Path) -> std::path::PathBuf { path.join("asset") }
#[allow(clippy::disallowed_methods, reason = "This fixture represents a dedicated blocking worker.")]
pub fn blocking_worker() { std::thread::sleep(std::time::Duration::ZERO); }
''')
        if permitted.returncode != 0:
            raise SystemExit("Blocking policy rejected a permitted call.\n" + permitted.stderr)
        outside_viewer = check('pub fn blocking_tool() { std::thread::sleep(std::time::Duration::ZERO); }')
        if outside_viewer.returncode != 0:
            raise SystemExit("Blocking policy escaped the viewer scope.\n" + outside_viewer.stderr)
    print("Blocking policy: aliased calls rejected; path joins and explicit exceptions accepted.")


if __name__ == "__main__":
    main()
