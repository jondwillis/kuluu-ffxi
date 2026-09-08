use std::path::{Path, PathBuf};
use std::process::Command;

const SDK_PATH: &str = "vendor/DLSS";
const SDK_HEADER: &str = "include/nvsdk_ngx.h";
const SDK_LICENSE: &str = "LICENSE.txt";
const STAGED_LICENSE: &str = "DLSS-license.txt";
const SDK_GUIDE: &str = "doc/DLSS_Programming_Guide_Release.pdf";
const STAGED_GUIDE: &str = "DLSS-programming-guide.pdf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows,
    Linux,
}

impl Platform {
    fn detect(os: &str, arch: &str) -> Result<Self, String> {
        match (os, arch) {
            ("windows", "x86_64") => Ok(Self::Windows),
            ("linux", "x86_64") => Ok(Self::Linux),
            _ => Err(format!(
                "DLSS builds require x86_64 Windows or Linux (this host is {os}/{arch}). Use the normal cargo build -p kuluu --release on this host."
            )),
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Windows => "x86_64-pc-windows-msvc",
            Self::Linux => "x86_64-unknown-linux-gnu",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Windows => "kuluu.exe",
            Self::Linux => "kuluu",
        }
    }

    fn runtime_dir(self) -> &'static str {
        match self {
            Self::Windows => "lib/Windows_x86_64/rel",
            Self::Linux => "lib/Linux_x86_64/rel",
        }
    }

    fn link_inputs(self) -> &'static [&'static str] {
        // dlss_wgpu 4.0.0 build.rs selects the Windows CRT variant at compile time.
        match self {
            Self::Windows => &[
                "lib/Windows_x86_64/x64/nvsdk_ngx_d.lib",
                "lib/Windows_x86_64/x64/nvsdk_ngx_s.lib",
            ],
            Self::Linux => &["lib/Linux_x86_64/libnvsdk_ngx.a"],
        }
    }

    fn is_sr_runtime(self, name: &str) -> bool {
        match self {
            Self::Windows => name == "nvngx_dlss.dll",
            Self::Linux => name.starts_with("libnvidia-ngx-dlss.so."),
        }
    }
}

pub fn run(args: &[String], workspace: &Path) -> Result<(), String> {
    let build = match args {
        [command] if command == "check" => false,
        [command] if command == "build" => true,
        _ => return Err("usage: cargo xtask dlss <check|build>".into()),
    };
    let platform = Platform::detect(std::env::consts::OS, std::env::consts::ARCH)?;
    let vulkan = std::env::var_os("VULKAN_SDK").map(PathBuf::from).ok_or(
        "Install the Vulkan SDK and set VULKAN_SDK to its root; install libclang as well.",
    )?;
    let vulkan = vulkan
        .canonicalize()
        .map_err(|error| format!("VULKAN_SDK is unavailable at {}: {error}", vulkan.display()))?;
    if !["Include/vulkan/vulkan.h", "include/vulkan/vulkan.h"]
        .iter()
        .any(|header| vulkan.join(header).is_file())
    {
        return Err(format!(
            "VULKAN_SDK has no Vulkan headers: {}",
            vulkan.display()
        ));
    }
    let sdk = match std::env::var_os("DLSS_SDK") {
        Some(path) => PathBuf::from(path),
        None => {
            if build {
                run_command(
                    Command::new("git").current_dir(workspace).args([
                        "submodule",
                        "update",
                        "--init",
                        "--checkout",
                        "--depth",
                        "1",
                        "--",
                        SDK_PATH,
                    ]),
                    "initialize the pinned NVIDIA DLSS SDK",
                )?;
            }
            workspace.join(SDK_PATH)
        }
    };
    let sdk = sdk.canonicalize().map_err(|error| {
        format!(
            "DLSS SDK is unavailable at {}: {error}. Run cargo xtask dlss build to initialize the pinned SDK, or set DLSS_SDK.",
            sdk.display()
        )
    })?;
    let files = staging_files(&sdk, platform)?;
    if !build {
        println!(
            "DLSS SDK and Vulkan headers found for {}. The build also requires libclang (set LIBCLANG_PATH if necessary); GPU/driver support is checked at runtime.",
            platform.target()
        );
        return Ok(());
    }
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"));
    let target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        workspace.join(target_dir)
    };
    run_command(
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(workspace)
            .env("DLSS_SDK", &sdk)
            .env("VULKAN_SDK", &vulkan)
            .args([
                "build",
                "--locked",
                "-p",
                "kuluu",
                "--release",
                "--no-default-features",
                "--features",
                "native-window,dlss",
                "--target",
                platform.target(),
                "--target-dir",
            ])
            .arg(&target_dir),
        "build the opt-in DLSS client",
    )?;
    let output = target_dir.join(platform.target()).join("release");
    stage(&files, &output, platform)?;
    println!(
        "DLSS build ready: {}",
        output.join(platform.executable()).display()
    );
    Ok(())
}

fn run_command(command: &mut Command, action: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("Cannot {action}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Failed to {action}: {status}"))
    }
}

fn staging_files(sdk: &Path, platform: Platform) -> Result<Vec<(PathBuf, String)>, String> {
    for required in [SDK_HEADER, SDK_LICENSE, SDK_GUIDE]
        .into_iter()
        .chain(platform.link_inputs().iter().copied())
    {
        if !sdk.join(required).is_file() {
            return Err(format!(
                "DLSS SDK is missing {}",
                sdk.join(required).display()
            ));
        }
    }
    let runtime_dir = sdk.join(platform.runtime_dir());
    let mut runtimes = std::fs::read_dir(&runtime_dir)
        .map_err(|error| format!("Cannot read {}: {error}", runtime_dir.display()))?
        .map(|entry| entry.map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| platform.is_sr_runtime(&entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    if runtimes.len() != 1 {
        return Err(format!(
            "Expected one release SR runtime in {}, found {}",
            runtime_dir.display(),
            runtimes.len()
        ));
    }
    let runtime = runtimes.pop().expect("one runtime");
    let filename = runtime
        .file_name()
        .expect("runtime filename")
        .to_string_lossy()
        .into_owned();
    Ok(vec![
        (runtime, filename),
        (sdk.join(SDK_LICENSE), STAGED_LICENSE.into()),
        // dlss_wgpu 4.0.0 README requires the guide's copyright/license notices.
        (sdk.join(SDK_GUIDE), STAGED_GUIDE.into()),
    ])
}

fn stage(files: &[(PathBuf, String)], output: &Path, platform: Platform) -> Result<(), String> {
    if !output.join(platform.executable()).is_file() {
        return Err(format!(
            "Built executable is missing in {}",
            output.display()
        ));
    }
    for (source, filename) in files {
        std::fs::copy(source, output.join(filename))
            .map_err(|error| format!("Cannot stage {}: {error}", source.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "kuluu-dlss-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, path: impl AsRef<Path>, bytes: &[u8]) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unsupported_hosts_fail_before_sdk_setup() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("linux", "aarch64"),
            ("windows", "aarch64"),
        ] {
            assert!(Platform::detect(os, arch).is_err());
        }
        assert_eq!(
            Platform::detect("linux", "x86_64").unwrap(),
            Platform::Linux
        );
        assert_eq!(
            Platform::detect("windows", "x86_64").unwrap(),
            Platform::Windows
        );
    }

    #[test]
    fn stages_only_release_sr_and_license_on_each_platform() {
        for (platform, runtime, unrelated) in [
            (Platform::Windows, "nvngx_dlss.dll", "nvngx_dlssg.dll"),
            (
                Platform::Linux,
                "libnvidia-ngx-dlss.so.310.5.3",
                "libnvidia-ngx-dlssd.so.310.5.3",
            ),
        ] {
            let fixture = Fixture::new();
            fixture.write(SDK_HEADER, b"headers");
            fixture.write(SDK_LICENSE, b"license");
            fixture.write(SDK_GUIDE, b"guide with notices");
            for input in platform.link_inputs() {
                fixture.write(input, b"library");
            }
            fixture.write(Path::new(platform.runtime_dir()).join(runtime), b"runtime");
            fixture.write(Path::new(platform.runtime_dir()).join(unrelated), b"other");
            fixture.write(Path::new("output").join(platform.executable()), b"client");
            let output = fixture.0.join("output");
            stage(
                &staging_files(&fixture.0, platform).unwrap(),
                &output,
                platform,
            )
            .unwrap();
            assert_eq!(std::fs::read(output.join(runtime)).unwrap(), b"runtime");
            assert_eq!(
                std::fs::read(output.join(STAGED_LICENSE)).unwrap(),
                b"license"
            );
            assert_eq!(
                std::fs::read(output.join(STAGED_GUIDE)).unwrap(),
                b"guide with notices"
            );
            assert!(!output.join(unrelated).exists());
        }
    }

    #[test]
    fn incomplete_or_ambiguous_sdk_cannot_be_staged() {
        let fixture = Fixture::new();
        assert!(staging_files(&fixture.0, Platform::Linux).is_err());
        fixture.write(SDK_HEADER, b"headers");
        fixture.write(SDK_LICENSE, b"license");
        fixture.write(SDK_GUIDE, b"guide with notices");
        fixture.write("lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so.1", b"one");
        assert!(staging_files(&fixture.0, Platform::Linux).is_err());
        fixture.write("lib/Linux_x86_64/libnvsdk_ngx.a", b"library");
        assert!(staging_files(&fixture.0, Platform::Linux).is_ok());
        fixture.write("lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so.2", b"two");
        assert!(staging_files(&fixture.0, Platform::Linux).is_err());
    }
}
