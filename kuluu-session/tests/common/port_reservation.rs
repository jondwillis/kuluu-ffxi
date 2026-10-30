use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::net::UdpSocket;
use std::path::Path;

use super::{LOCAL_PORT_BASE, LOCAL_PORT_SPAN, TOMBSTONE_TTL_SECS};

const LOCK_FILE: &str = "ports.lock";
const STATE_FILE: &str = "ports.json";

pub(super) fn reserve(directory: &Path, seed: u32, now: u64) -> io::Result<u16> {
    reserve_with_probe(directory, seed, now, |port| {
        UdpSocket::bind(("0.0.0.0", port))
    })
}

fn reserve_with_probe<T>(
    directory: &Path,
    seed: u32,
    now: u64,
    mut probe: impl FnMut(u16) -> io::Result<T>,
) -> io::Result<u16> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(LOCK_FILE))?;
    lock.lock()?;
    let state_path = directory.join(STATE_FILE);
    let mut reserved: BTreeMap<u16, u64> = match File::open(&state_path) {
        Ok(file) => serde_json::from_reader(file).map_err(io::Error::other)?,
        Err(error) if error.kind() == ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error),
    };
    reserved.retain(|_, expires| *expires >= now);
    for offset in 0..LOCAL_PORT_SPAN {
        let port = LOCAL_PORT_BASE + ((seed % LOCAL_PORT_SPAN + offset) % LOCAL_PORT_SPAN) as u16;
        if reserved.contains_key(&port) {
            continue;
        }
        let Ok(socket) = probe(port) else {
            continue;
        };
        reserved.insert(port, now.saturating_add(u64::from(TOMBSTONE_TTL_SECS)));
        let temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(&temporary, &reserved).map_err(io::Error::other)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&state_path).map_err(io::Error::other)?;
        drop(socket);
        return Ok(port);
    }
    Err(io::Error::new(
        ErrorKind::AddrNotAvailable,
        "no unreserved bindable fixture UDP endpoint",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    const TEST_NOW: u64 = 1;
    const TEST_SEED: u32 = 0;
    const WORKER_COUNT: usize = 4;
    const WORKER_PATH_ENV: &str = "KULUU_PORT_TEST_DIRECTORY";
    const WORKER_NAME: &str = "common::port_reservation::tests::reservation_worker";
    const WORKER_OUTPUT: &str = "fixture-port=";

    fn reserve_available(directory: &Path, seed: u32, now: u64) -> io::Result<u16> {
        reserve_with_probe(directory, seed, now, |_| Ok(()))
    }

    #[test]
    fn bindable_endpoint_remains_reserved_until_server_budget_expires() {
        let directory = tempfile::tempdir().unwrap();
        let first = reserve_available(directory.path(), TEST_SEED, TEST_NOW).unwrap();
        let second = reserve_available(directory.path(), TEST_SEED, TEST_NOW).unwrap();
        assert_ne!(first, second);
        let expiry = TEST_NOW + u64::from(TOMBSTONE_TTL_SECS);
        let at_expiry = reserve_available(directory.path(), TEST_SEED, expiry).unwrap();
        assert_ne!(first, at_expiry);
        assert_eq!(
            reserve_available(directory.path(), TEST_SEED, expiry + 1).unwrap(),
            first
        );
    }

    #[test]
    fn concurrent_processes_reserve_distinct_remote_endpoints() {
        let directory = tempfile::tempdir().unwrap();
        let mut children = Vec::new();
        for _ in 0..WORKER_COUNT {
            children.push(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", WORKER_NAME, "--ignored", "--nocapture"])
                    .env(WORKER_PATH_ENV, directory.path())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        let mut ports = std::collections::BTreeSet::new();
        for child in children {
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            let port: u16 = text
                .lines()
                .find_map(|line| line.strip_prefix(WORKER_OUTPUT))
                .expect("worker returned a reserved endpoint")
                .parse()
                .unwrap();
            assert!(ports.insert(port), "reused remote endpoint {port}");
        }
        assert_eq!(ports.len(), WORKER_COUNT);
        let next = reserve_available(directory.path(), TEST_SEED, TEST_NOW).unwrap();
        assert!(!ports.contains(&next));
    }

    #[test]
    fn corrupt_reservation_state_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(STATE_FILE), "{").unwrap();
        assert!(reserve_available(directory.path(), TEST_SEED, TEST_NOW).is_err());
    }

    #[test]
    #[ignore = "child process for concurrent endpoint reservation"]
    fn reservation_worker() {
        let directory = std::env::var_os(WORKER_PATH_ENV).expect("worker directory");
        let port = reserve_available(Path::new(&directory), TEST_SEED, TEST_NOW).unwrap();
        println!("{WORKER_OUTPUT}{port}");
    }
}
