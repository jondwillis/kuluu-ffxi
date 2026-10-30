#!/usr/bin/env python3
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
MAC_BINARY = "kuluu-windows-mac-probe"
ICON_BINARY = "kuluu"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(command, directory, output, environment):
    result = subprocess.run(
        command, cwd=directory, env=environment, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=600,
    )
    output.write_text(result.stdout, encoding="utf-8")
    if result.returncode:
        raise RuntimeError(f"Command failed with exit {result.returncode}: {command[0]}; see {output.name}")
    return result.stdout


def packages(path):
    return tomllib.loads(path.read_text(encoding="utf-8"))["package"]


def version(locked, name):
    matches = [p["version"] for p in locked if p["name"] == name]
    return max(matches, key=lambda value: tuple(int(part) for part in value.split(".")))


def check_resolution(original, resolved, binary):
    expected = {(p["name"], p["version"]): p.get("checksum") for p in original}
    for package in resolved:
        if package["name"] == binary:
            continue
        identity = package["name"], package["version"]
        if identity not in expected or expected[identity] != package.get("checksum"):
            raise RuntimeError(f"Probe dependency differs from the reviewed lockfile: {identity}")


def prepare(dependency, output, locked):
    fixture = output / "fixture"
    fixture.mkdir(parents=True, exist_ok=True)
    (fixture / "src").mkdir(exist_ok=True)
    binary = MAC_BINARY if dependency == "mac_address" else ICON_BINARY
    manifest = (
        f'[package]\nname = "{binary}"\nversion = "0.0.0"\n'
        'edition = "2024"\n\n[dependencies]\n'
    )
    inputs = {}
    if dependency == "mac_address":
        source = ROOT / "kuluu-session/src/auth_binary.rs"
        manifest += f'mac_address = "={version(locked, dependency)}"\n'
        manifest += f'thiserror = "={version(locked, "thiserror")}"\n'
        code = f'#[path = {json.dumps(source.as_posix())}]\nmod auth_binary;\n'
        code += '''
fn main() {
    assert!(std::env::var_os("FFXI_MAC").is_none(), "FFXI_MAC must be unset");
    let address = auth_binary::local_mac_string().expect("production adapter detection failed");
    assert!(address != auth_binary::NULL_MAC, "production adapter detection returned a null address");
    let text = std::str::from_utf8(&address).expect("production MAC output is not UTF-8");
    let _: mac_address::MacAddress = text.parse().expect("production MAC output is malformed");
    println!("production adapter detection returned a nonzero formatted address; value withheld");
}
'''
        inputs[str(source.relative_to(ROOT))] = digest(source)
    else:
        locked_version = version(locked, dependency)
        manifest += f'embed-resource = "={locked_version}"\n\n[build-dependencies]\n'
        manifest += f'embed-resource = "={locked_version}"\n'
        for relative in ["build.rs", "assets/branding/kuluu.rc", "assets/branding/kuluu.ico"]:
            source = ROOT / "kuluu" / relative
            target = fixture / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            inputs[str(source.relative_to(ROOT))] = digest(source)
        code = '''
fn main() {
    let compiler = embed_resource::find_windows_sdk_tool("rc.exe")
        .expect("production Windows SDK discovery failed");
    assert!(compiler.is_file(), "resource compiler does not exist");
    println!("resource_compiler={}", compiler.display());
}
'''
    (fixture / "Cargo.toml").write_text(manifest, encoding="utf-8")
    (fixture / "src/main.rs").write_text(code, encoding="utf-8")
    shutil.copyfile(ROOT / "Cargo.lock", fixture / "Cargo.lock")
    return fixture, binary, inputs


def icon_count(executable):
    shell = ctypes.WinDLL("shell32", use_last_error=True)
    extract = shell.ExtractIconExW
    extract.argtypes = [
        ctypes.c_wchar_p, ctypes.c_int, ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_void_p), ctypes.c_uint,
    ]
    extract.restype = ctypes.c_uint
    count = extract(str(executable), -1, None, None, 0)
    if count in (0, ctypes.c_uint(-1).value):
        raise RuntimeError("The linked executable has no readable icon resource")
    return count


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dependency", choices=["mac_address", "embed-resource"], required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    locked = packages(ROOT / "Cargo.lock")
    result = {
        "dependency": args.dependency, "version": version(locked, args.dependency),
        "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "platform": platform.platform(), "machine": platform.machine(), "status": "failed",
        "lockfile_sha256": digest(ROOT / "Cargo.lock"),
    }
    try:
        fixture, binary, result["production_inputs"] = prepare(args.dependency, output, locked)
        if args.prepare_only:
            result["status"] = "prepared"
            return
        if sys.platform != "win32":
            raise RuntimeError("Native Windows execution is required for this evidence")
        if os.environ.get("FFXI_MAC") is not None:
            raise RuntimeError("FFXI_MAC must be unset for production adapter detection")
        environment = dict(os.environ, CARGO_TARGET_DIR=str(output / "target"))
        environment["RUSTFLAGS"] = "-C target-feature=+crt-static"
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        cargo = ["cargo", f"+{toolchain}"]
        compiler = run(["rustc", f"+{toolchain}", "-vV"], fixture, output / "rustc.log", environment)
        result["compiler"] = compiler
        host = next(line.removeprefix("host: ") for line in compiler.splitlines() if line.startswith("host: "))
        run(cargo + ["build", "--release", "--manifest-path", str(fixture / "Cargo.toml")],
            fixture, output / "build.log", environment)
        resolved = packages(fixture / "Cargo.lock")
        check_resolution(locked, resolved, binary)
        result["resolved_packages"] = [
            {key: package[key] for key in ["name", "version", "checksum"] if key in package}
            for package in resolved
        ]
        executable = output / "target/release" / f"{binary}.exe"
        result["executable_sha256"] = digest(executable)
        environment.update(HOST=host, TARGET=host)
        observation = run([str(executable)], fixture, output / "runtime.log", environment).strip()
        result["observation"] = observation
        if args.dependency == "embed-resource":
            result["icon_resource_count"] = icon_count(executable)
        result["status"] = "passed"
    except Exception as error:
        result["error"] = str(error)
        raise
    finally:
        (output / "evidence.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result))


if __name__ == "__main__":
    main()
