//! Exercise the real single-process sandbox entry point, rather than applying
//! restrictions to the Cargo test runner or relying on mocked file operations.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn private_disk_watchdog_stops_regular_unlinked_and_many_file_writers_but_keeps_warm_jobs() {
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    let code = r"
import os, pathlib, sys, time
build, kind = pathlib.Path(sys.argv[1]), sys.argv[2]
if kind == 'warm':
    (build / 'result.txt').write_bytes(b'valid warm output')
    print('warm output verified')
    sys.exit(0)
handles = []
for index in range(400):
    path = build / ('disk-output-' + str(index))
    if kind == 'files':
        path.touch()
    else:
        handle = path.open('wb')
        handle.write(b'x' * 16384)
        handle.flush()
        if kind == 'unlinked':
            path.unlink()
            handles.append(handle)
        else:
            handle.close()
    time.sleep(.01)
raise AssertionError('the output watchdog did not stop the writer')
";
    for kind in ["regular", "unlinked", "files", "warm"] {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        let build = project.join(".texe/build");
        fs::create_dir_all(&build).unwrap();
        fs::write(project.join("main.tex"), b"research source").unwrap();
        fs::write(project.join("main.pdf"), b"last published PDF").unwrap();
        fs::write(build.join("existing-cache.bin"), vec![0_u8; 16 * 1024]).unwrap();
        let policy = serde_json::json!({
            "version": 1, "project":fs::canonicalize(&project).unwrap(),
            "write_roots":[fs::canonicalize(&build).unwrap()],
            "read_roots":[], "execute_roots":[],
            "private_output_bytes": if kind == "files" {1024 * 1024} else {64 * 1024},
            "private_output_entries": if kind == "files" {8} else {100_000}
        });
        let result = Command::new(env!("CARGO_BIN_EXE_texe"))
            .args([
                "__sandbox-run",
                "--policy",
                &policy.to_string(),
                "--executable",
                "/usr/bin/python3",
                "--",
                "-c",
                code,
            ])
            .arg(&build)
            .arg(kind)
            .process_group(0)
            .output()
            .unwrap();
        if kind == "warm" {
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                fs::read(build.join("result.txt")).unwrap(),
                b"valid warm output"
            );
        } else {
            assert!(!result.status.success());
            let reason = String::from_utf8_lossy(&result.stderr);
            assert!(
                reason.contains("disk budget") || reason.contains("too many files"),
                "{kind}: {reason}"
            );
            assert!(!reason.contains("watchdog did not stop"));
        }
        assert_eq!(
            fs::read(project.join("main.tex")).unwrap(),
            b"research source"
        );
        assert_eq!(
            fs::read(project.join("main.pdf")).unwrap(),
            b"last published PDF"
        );
        assert_eq!(
            fs::read(build.join("existing-cache.bin")).unwrap(),
            vec![0_u8; 16 * 1024]
        );
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn shipout_can_reserve_its_guard_arena_without_raising_committed_memory_limits() {
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("project");
    let build = project.join(".texe/build");
    fs::create_dir_all(&build).unwrap();
    let code = r"
import ctypes, errno, resource, sys
sparse = sys.argv[1] == 'true'
gib = 1024**3
assert resource.getrlimit(resource.RLIMIT_AS) == ((128 if sparse else 2)*gib,)*2
assert resource.getrlimit(resource.RLIMIT_DATA) == (2*gib,)*2
libc = ctypes.CDLL(None, use_errno=True)
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mprotect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
failed = ctypes.c_void_p(-1).value
arena = libc.mmap(None, 32*gib, 0, 0x02|0x20|0x4000, -1, 0)
if sparse:
    assert arena != failed, ctypes.get_errno()
    assert libc.mprotect(arena, 64*1024**2, 3) == 0, ctypes.get_errno()
    ctypes.memset(arena, 7, 4096)
    # An address reservation must not become a huge writable allocation.
    assert libc.mprotect(arena, 3*gib, 3) == -1
    assert ctypes.get_errno() == errno.ENOMEM
    assert libc.mmap(None, 2*gib, 3, 0x02|0x20, -1, 0) == failed
    assert ctypes.get_errno() == errno.ENOMEM
    assert libc.munmap(arena, 32*gib) == 0
else:
    assert arena == failed
    assert ctypes.get_errno() == errno.ENOMEM
print('sparse arena and committed memory limits verified')
";
    for sparse in [false, true] {
        let policy = serde_json::json!({
            "version": 1,
            "project": fs::canonicalize(&project).unwrap(),
            "write_roots": [fs::canonicalize(&build).unwrap()],
            "read_roots": [],
            "execute_roots": [],
            "shipout_sparse_arena": sparse
        });
        let result = Command::new(env!("CARGO_BIN_EXE_texe"))
            .args([
                "__sandbox-run",
                "--policy",
                &policy.to_string(),
                "--executable",
                "/usr/bin/python3",
                "--",
                "-c",
                code,
                if sparse { "true" } else { "false" },
            ])
            .process_group(0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[allow(clippy::too_many_lines)]
fn protected_compiler_preserves_sources_and_denies_network_and_metadata_mutation() {
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("project");
    let build = project.join(".texe/build");
    let resources = fixture.path().join("runtime");
    fs::create_dir_all(&build).unwrap();
    fs::create_dir_all(&resources).unwrap();
    let source = project.join("main.tex");
    let outside = fixture.path().join("outside.txt");
    fs::write(&source, "research source").unwrap();
    fs::write(&outside, "outside sentinel").unwrap();
    fs::write(resources.join("data.txt"), "runtime data").unwrap();
    let before = fs::metadata(&source).unwrap();
    let policy = serde_json::json!({
        "version": 1,
        "project": fs::canonicalize(&project).unwrap(),
        "write_roots": [fs::canonicalize(&build).unwrap()],
        "read_roots": [fs::canonicalize(&resources).unwrap()],
        "execute_roots": []
    });
    let code = r"
import ctypes, errno, fcntl, os, pathlib, socket, sys
project, build, runtime, outside = map(pathlib.Path, sys.argv[1:])
source = project / 'main.tex'
assert source.read_text() == 'research source'
assert (runtime / 'data.txt').read_text() == 'runtime data'
(build / 'result.txt').write_text('private output')
assert 'CITET_ACCESS_TOKEN' not in os.environ
assert 'AWS_SECRET_ACCESS_KEY' not in os.environ
assert pathlib.Path(os.environ['HOME']) == build

def denied(operation):
    try:
        operation()
    except OSError as failure:
        assert failure.errno in (errno.EACCES, errno.EPERM, errno.EXDEV)
        return
    raise AssertionError('unsafe operation was allowed')

denied(lambda: source.write_text('destroyed'))
denied(lambda: outside.write_text('destroyed'))
denied(lambda: outside.read_text())
denied(lambda: os.chmod(source, 0))
denied(lambda: os.utime(source, (1, 1)))
with source.open('rb') as stream:
    denied(lambda: os.fchmod(stream.fileno(), 0))
    denied(lambda: fcntl.fcntl(stream.fileno(), fcntl.F_SETOWN, os.getppid()))
    denied(lambda: fcntl.fcntl(stream.fileno(), 1024, fcntl.F_RDLCK))
denied(lambda: socket.socket(socket.AF_INET, socket.SOCK_STREAM))
denied(lambda: os.setsid())
denied(lambda: os.setpgid(0, 0))
denied(lambda: os.link(source, build / 'linked-source'))
# x32 uses the same audit architecture as x86_64 and must not bypass seccomp.
libc = ctypes.CDLL(None, use_errno=True)
assert libc.prctl(15, ctypes.c_char_p(b'texe-biber'), 0, 0, 0) == 0
assert libc.prctl(1, 0, 0, 0, 0) == -1
assert ctypes.get_errno() == errno.EPERM
with source.open('rb') as stream:
    flag = ctypes.c_int(1)
    assert libc.ioctl(stream.fileno(), 0x5421, ctypes.byref(flag)) == 0
    assert libc.ioctl(stream.fileno(), 0x40086602, ctypes.byref(flag)) == -1
    assert ctypes.get_errno() == errno.EPERM
assert libc.syscall(0x40000000 + 41, 2, 1, 0) == -1
assert ctypes.get_errno() == errno.ENOSYS
assert libc.syscall(999) == -1
assert ctypes.get_errno() == errno.EPERM
print('protected compiler verified')
";
    let result = Command::new(env!("CARGO_BIN_EXE_texe"))
        .args([
            "__sandbox-run",
            "--policy",
            &policy.to_string(),
            "--executable",
            "/usr/bin/python3",
            "--",
            "-c",
            code,
        ])
        .args([&project, &build, &resources, &outside])
        .env("CITET_ACCESS_TOKEN", "must-not-reach-compiler")
        .env("AWS_SECRET_ACCESS_KEY", "must-not-reach-compiler")
        .process_group(0)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "research source");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside sentinel");
    assert_eq!(
        fs::read_to_string(build.join("result.txt")).unwrap(),
        "private output"
    );
    assert_eq!(
        fs::metadata(&source).unwrap().permissions(),
        before.permissions()
    );
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn killing_the_parent_stops_the_compiler_and_its_background_helper() {
    use std::fs;
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("project");
    let build = project.join(".texe/environments/test-environment/build");
    fs::create_dir_all(&build).unwrap();
    let pids = build.join("pids.txt");
    let policy = serde_json::json!({
        "version": 1,
        "project": fs::canonicalize(&project).unwrap(),
        "write_roots": [fs::canonicalize(&build).unwrap()],
        "read_roots": [],
        "execute_roots": []
    });
    let driver = r#"
import pathlib, subprocess, sys, time
binary, policy, pids = sys.argv[1:]
code = "import os,pathlib,sys,time; pid=os.fork(); pathlib.Path(sys.argv[1]).write_text(str(os.getpid())+','+str(pid)) if pid else None; time.sleep(60)"
child = subprocess.Popen([binary, '__sandbox-run', '--policy', policy, '--executable', '/usr/bin/python3', '--', '-c', code, pids], process_group=0, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(200):
    if pathlib.Path(pids).exists():
        print('ready', flush=True)
        time.sleep(60)
        break
    if child.poll() is not None:
        raise RuntimeError('compiler exited before creating helper')
    time.sleep(.01)
else:
    raise RuntimeError('compiler did not become ready')
"#;
    let mut parent = Command::new("/usr/bin/python3")
        .args([
            "-c",
            driver,
            env!("CARGO_BIN_EXE_texe"),
            &policy.to_string(),
        ])
        .arg(&pids)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    std::io::BufReader::new(parent.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if ready.trim() != "ready" {
        let result = parent.wait_with_output().unwrap();
        panic!("{}", String::from_utf8_lossy(&result.stderr));
    }
    let ids = fs::read_to_string(&pids).unwrap();
    let ids = ids
        .split(',')
        .map(|value| value.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.iter().all(|id| *id > 1));
    parent.kill().unwrap();
    parent.wait().unwrap();
    let stopped = |pid: u32| {
        fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with('Z'))
        })
    };
    let start = Instant::now();
    while !ids.iter().all(|pid| stopped(*pid)) && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ids.iter().all(|pid| stopped(*pid)),
        "a compiler helper survived parent cancellation"
    );
}
