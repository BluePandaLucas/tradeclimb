//! Linux strategy confinement. No trusted-code fallback is available.
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

/// The user manager owns the cgroup; untrusted code never sees its control files.
pub struct Sandbox {
    pub command: Command,
    unit: String,
}
pub struct SandboxedChild {
    pub child: Child,
    unit: String,
}
fn manager(program: &str) -> Result<Command> {
    ensure!(
        cfg!(target_os = "linux"),
        "strategy isolation requires Linux"
    );
    let mut command = Command::new(program);
    control_environment(&mut command)?;
    Ok(command)
}
fn control_environment(command: &mut Command) -> Result<()> {
    command.env_clear().env("PATH", "/usr/bin:/bin");
    command.env(
        "XDG_RUNTIME_DIR",
        std::env::var_os("XDG_RUNTIME_DIR")
            .context("XDG_RUNTIME_DIR required: start TC in a systemd user session")?,
    );
    if let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
        command.env("DBUS_SESSION_BUS_ADDRESS", bus);
    }
    Ok(())
}
impl Sandbox {
    fn base(build: bool) -> Result<Self> {
        for tool in [
            "/usr/bin/bwrap",
            "/usr/bin/prlimit",
            "/usr/bin/systemd-run",
            "/usr/bin/systemctl",
            "/usr/bin/timeout",
        ] {
            ensure!(
                Path::new(tool).is_file(),
                "sandbox prerequisite missing: {tool}"
            );
        }
        let unit = format!("tc-sandbox-{}.scope", uuid::Uuid::new_v4().simple());
        let mut command = manager("/usr/bin/systemd-run")?;
        command.args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--expand-environment=no",
            "--unit",
            &unit,
        ]);
        for property in if build {
            [
                "MemoryMax=4G",
                "MemorySwapMax=0",
                "TasksMax=128",
                "CPUQuota=200%",
            ]
        } else {
            [
                "MemoryMax=512M",
                "MemorySwapMax=0",
                "TasksMax=32",
                "CPUQuota=100%",
            ]
        } {
            command.args(["--property", property]);
        }
        if build {
            command.args(["--property", "RuntimeMaxSec=1800s"]);
        }
        command
            .arg("--")
            .arg("/usr/bin/prlimit")
            .args(["--core=0:0", "--nofile=128:128"]);
        command.arg(if build {
            "--fsize=536870912:536870912"
        } else {
            "--fsize=16777216:16777216"
        });
        command.arg("--").arg("/usr/bin/bwrap").args([
            "--unshare-all",
            "--unshare-user",
            "--die-with-parent",
            "--new-session",
            "--disable-userns",
            "--assert-userns-disabled",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--ro-bind",
            "/usr/bin",
            "/usr/bin",
            "--ro-bind",
            "/usr/lib",
            "/usr/lib",
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--size",
            if build { "268435456" } else { "16777216" },
            "--tmpfs",
            "/tmp",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "HOME",
            "/nonexistent",
            "--setenv",
            "LANG",
            "C.UTF-8",
        ]);
        if Path::new("/usr/lib64").exists() {
            command.args(["--ro-bind", "/usr/lib64", "/usr/lib64"]);
        }
        if build {
            command.args(["--ro-bind", "/usr/include", "/usr/include"]);
            if Path::new("/usr/libexec").is_dir() {
                command.args(["--ro-bind", "/usr/libexec", "/usr/libexec"]);
            }
            // Ubuntu's compiler aliases point through /etc/alternatives; expose
            // only these public tools, never the host's /etc directory.
            for name in ["cc", "c++"] {
                let path = Path::new("/usr/bin").join(name);
                if path.exists() {
                    let target = path.canonicalize()?;
                    ensure!(
                        target.starts_with("/usr/bin") || target.starts_with("/usr/lib"),
                        "compiler must be in the trusted OS runtime image"
                    );
                    command
                        .arg("--symlink")
                        .arg(target)
                        .arg(format!("/etc/alternatives/{name}"));
                }
            }
        }
        if Path::new("/etc/ld.so.cache").is_file() {
            command.args(["--ro-bind", "/etc/ld.so.cache", "/etc/ld.so.cache"]);
        }
        // The scope launcher remains the process-group leader for paper recovery.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        Ok(Self { command, unit })
    }
    pub fn strategy(executable: &Path, spec: &str, broker: Option<&Path>) -> Result<Self> {
        let executable = executable
            .canonicalize()
            .context("strategy executable missing")?;
        ensure!(executable.is_file(), "strategy must be a regular file");
        let mut sandbox = Self::base(false)?;
        sandbox
            .command
            .arg("--ro-bind")
            .arg(executable)
            .arg("/strategy");
        sandbox.command.args(["--setenv", "TC_STRATEGY_SPEC", spec]);
        if let Some(broker) = broker {
            sandbox
                .command
                .arg("--ro-bind")
                .arg(broker.canonicalize()?)
                .arg("/broker")
                .args(["--setenv", "TC_MODEL_SOCKET", "/broker/model.sock"]);
        }
        sandbox
            .command
            .args(["--chdir", "/tmp", "--remount-ro", "/", "--", "/strategy"]);
        Ok(sandbox)
    }
    /// Build source and SDK are read-only. Cargo output exists only on bounded tmpfs.
    pub fn build(
        source: &Path,
        sdk: &Path,
        toolchain: &Path,
        registry: &Path,
        bin: &str,
    ) -> Result<Self> {
        ensure!(
            !bin.is_empty()
                && bin.len() <= 100
                && bin
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid binary name"
        );
        let mut sandbox = Self::base(true)?;
        for (from, to) in [
            (source.to_path_buf(), "/workspace/examples/strategy"),
            (sdk.join("Cargo.toml"), "/workspace/Cargo.toml"),
            (sdk.join("crates/tc-core"), "/workspace/crates/tc-core"),
            (sdk.join("crates/tc-sdk"), "/workspace/crates/tc-sdk"),
            (toolchain.to_path_buf(), "/toolchain"),
        ] {
            sandbox
                .command
                .arg("--ro-bind")
                .arg(from.canonicalize()?)
                .arg(to);
        }
        sandbox.command.args([
            "--size",
            "2147483648",
            "--tmpfs",
            "/build",
            "--size",
            "134217728",
            "--tmpfs",
            "/cargo",
        ]);
        for name in ["src", "cache", "index"] {
            let path = registry.join(name);
            ensure!(
                path.is_dir(),
                "Cargo registry {name} cache missing; provision dependencies before isolated builds"
            );
            sandbox
                .command
                .arg("--ro-bind")
                .arg(path.canonicalize()?)
                .arg(format!("/cargo/registry/{name}"));
        }
        sandbox.command.args(["--setenv", "CARGO_HOME", "/cargo", "--setenv", "RUSTC", "/toolchain/bin/rustc",
            "--setenv", "PATH", "/toolchain/bin:/usr/bin:/bin", "--setenv", "CARGO_BUILD_JOBS", "2",
            "--chdir", "/workspace/examples/strategy", "--remount-ro", "/", "--", "/usr/bin/timeout", "--kill-after=2", "1800",
            "/bin/sh", "-c", "/toolchain/bin/cargo build --release --offline --frozen --bin \"$1\" --target-dir /build >&2 && exec /bin/cat \"/build/release/$1\"", "tc-build", bin]);
        Ok(sandbox)
    }
    pub fn spawn(mut self) -> Result<SandboxedChild> {
        // Also sanitize the namespace init's /proc environment, not just exec's.
        control_environment(&mut self.command)?;
        let child = self.command.spawn().context(
            "cannot start sandbox; Bubblewrap and delegated systemd user cgroups are required",
        )?;
        Ok(SandboxedChild {
            child,
            unit: self.unit,
        })
    }
}
impl Drop for SandboxedChild {
    fn drop(&mut self) {
        // Kill the complete cgroup, including descendants that called setsid().
        if let Ok(mut command) = manager("/usr/bin/timeout") {
            let _ = command
                .args([
                    "2s",
                    "/usr/bin/systemctl",
                    "--user",
                    "kill",
                    "--signal=KILL",
                    "--kill-whom=all",
                    &self.unit,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        // A launcher cancelled during StartTransientUnit can leave an empty
        // scope active. Stop it too, so the user manager can collect metadata.
        if let Ok(mut command) = manager("/usr/bin/timeout") {
            let _ = command
                .args([
                    "2s",
                    "/usr/bin/systemctl",
                    "--user",
                    "stop",
                    "--no-block",
                    &self.unit,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Resolve the trusted compiler, never by running Cargo in an untrusted project.
pub fn toolchain() -> Result<PathBuf> {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()?;
    ensure!(output.status.success(), "cannot locate Rust compiler");
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Read;
    fn run(script: &str) -> (bool, String) {
        let mut sandbox = Sandbox::strategy(Path::new("/usr/bin/python3"), "{}", None).unwrap();
        sandbox
            .command
            .args(["-c", script])
            .env("TC_SANDBOX_SENTINEL", "owner-secret")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = sandbox.spawn().unwrap();
        let mut output = String::new();
        child
            .child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        let ok = child.child.wait().unwrap().success();
        (ok, output)
    }
    #[test]
    #[ignore = "requires Bubblewrap and delegated systemd user cgroups"]
    fn cancelling_during_scope_start_does_not_leave_active_units() {
        for _ in 0..5 {
            let mut sandbox = Sandbox::strategy(Path::new("/bin/sh"), "{}", None).unwrap();
            let unit = sandbox.unit.clone();
            sandbox.command.args(["-c", "sleep 300"]);
            drop(sandbox.spawn().unwrap());
            let mut active = true;
            for _ in 0..100 {
                active = manager("/usr/bin/systemctl")
                    .unwrap()
                    .args(["--user", "is-active", "--quiet", &unit])
                    .status()
                    .unwrap()
                    .success();
                if !active {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert!(!active, "cancelled scope still active");
        }
    }
    #[test]
    #[ignore = "requires Bubblewrap, cgroups, and the trusted C toolchain"]
    fn build_image_compiler_can_link_using_only_public_tool_files() {
        let mut sandbox = Sandbox::base(true).unwrap();
        sandbox.command.args(["--remount-ro", "/", "--", "/bin/sh", "-c", "printf '#include <stdio.h>\nint main(void){puts(\"compiler-ready\");return 0;}' > /tmp/probe.c && cc /tmp/probe.c -o /tmp/probe && /tmp/probe"])
            .stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = sandbox.spawn().unwrap();
        let mut output = String::new();
        child
            .child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(child.child.wait().unwrap().success());
        assert!(output.contains("compiler-ready"));
    }
    #[test]
    #[ignore = "subprocess helper for parent death test"]
    fn parent_death_helper() {
        let Some(marker) = std::env::var_os("TC_SANDBOX_PARENT_MARKER") else {
            return;
        };
        let mut sandbox = Sandbox::strategy(Path::new("/bin/sh"), "{}", None).unwrap();
        let unit = sandbox.unit.clone();
        sandbox
            .command
            .args(["-c", "sleep 300 & echo ready; wait"])
            .stdout(Stdio::piped());
        let mut child = sandbox.spawn().unwrap();
        child
            .child
            .stdout
            .take()
            .unwrap()
            .read_exact(&mut [0; 6])
            .unwrap();
        std::fs::write(marker, unit).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    #[test]
    #[ignore = "requires Bubblewrap and delegated systemd user cgroups"]
    fn killing_owner_cleans_up_namespace_without_rust_destructors() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("unit");
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "tests::parent_death_helper"])
            .env("TC_SANDBOX_PARENT_MARKER", &marker)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..200 {
            if marker.exists() {
                break;
            }
            assert!(
                owner.try_wait().unwrap().is_none(),
                "sandbox owner exited before launch"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !marker.exists() {
            let _ = owner.kill();
            panic!("sandbox owner did not start");
        }
        let unit = std::fs::read_to_string(&marker).unwrap();
        let output = manager("/usr/bin/systemctl")
            .unwrap()
            .args(["--user", "show", "-p", "ControlGroup", "--value", &unit])
            .output()
            .unwrap();
        let path = Path::new("/sys/fs/cgroup").join(
            String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .trim_start_matches('/'),
        );
        owner.kill().unwrap();
        owner.wait().unwrap();
        for _ in 0..200 {
            if !path.exists()
                || std::fs::read_to_string(path.join("cgroup.events"))
                    .is_ok_and(|s| s.contains("populated 0"))
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = manager("/usr/bin/systemctl")
            .unwrap()
            .args(["--user", "kill", "--signal=KILL", &unit])
            .status();
        panic!("sandbox survived abrupt owner death");
    }
    #[test]
    #[ignore = "requires Bubblewrap and delegated systemd user cgroups"]
    fn hides_host_files_environment_and_network() {
        let temp = tempfile::tempdir().unwrap();
        let secret = temp.path().join("validation-secret");
        std::fs::write(&secret, "owner-secret").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let script = format!(
            r#"
import os,socket,errno
assert not os.path.exists({secret:?})
assert 'TC_SANDBOX_SENTINEL' not in os.environ
assert b'owner-secret' not in open('/proc/1/environ','rb').read()
assert not os.path.exists('/sys/fs/cgroup')
for path in ['/strategy','/host-write']:
 try:
  open(path,'wb').write(b'x')
  raise AssertionError('writable root')
 except OSError as e: assert e.errno in (errno.EROFS,errno.EACCES,errno.ETXTBSY)
s=socket.socket();s.settimeout(.2)
assert s.connect_ex(('127.0.0.1',{port})) != 0
print('isolated')
"#,
            secret = secret.to_str().unwrap()
        );
        let (ok, output) = run(&script);
        assert!(ok);
        assert!(output.contains("isolated"));
        assert_eq!(std::fs::read_to_string(secret).unwrap(), "owner-secret");
    }
    #[test]
    #[ignore = "requires Bubblewrap and delegated systemd user cgroups"]
    fn limits_total_memory_and_scratch_space() {
        let (ok, output) = run(r#"
import errno
try:
 open('/tmp/a','wb').write(b'x'*(12*1024*1024))
 open('/tmp/b','wb').write(b'x'*(12*1024*1024))
 raise AssertionError('tmpfs unbounded')
except OSError as e: assert e.errno == errno.ENOSPC
print('bounded')
"#);
        assert!(ok);
        assert!(output.contains("bounded"));
        // Each child is below MemoryMax, but the combined allocation is above it.
        let (ok, _) = run(r#"
import subprocess,sys
code='import time; x=bytearray(350*1024*1024); time.sleep(1)'
a=subprocess.Popen([sys.executable,'-c',code]); b=subprocess.Popen([sys.executable,'-c',code])
sys.exit(0 if a.wait()==0 and b.wait()==0 else 42)
"#);
        assert!(!ok, "aggregate memory limit not enforced");
    }
    #[test]
    #[ignore = "requires Bubblewrap and delegated systemd user cgroups"]
    fn caps_process_count_and_kills_detached_descendants() {
        let (ok, output) = run(r#"
import subprocess,errno
children=[]
try:
 for _ in range(100): children.append(subprocess.Popen(['/bin/sleep','2'],start_new_session=True))
 raise AssertionError('task limit not enforced')
except OSError as e: assert e.errno == errno.EAGAIN
print(len(children))
"#);
        assert!(ok);
        assert!(output.trim().parse::<usize>().unwrap() < 32);
        // Dropping the launcher must clear the entire cgroup, not just its leader.
        let mut sandbox = Sandbox::strategy(Path::new("/bin/sh"), "{}", None).unwrap();
        let unit = sandbox.unit.clone();
        sandbox
            .command
            .args(["-c", "sleep 300 & echo ready; wait"])
            .stdout(Stdio::piped());
        let mut child = sandbox.spawn().unwrap();
        let mut ready = [0; 6];
        child
            .child
            .stdout
            .take()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        let group = manager("/usr/bin/systemctl")
            .unwrap()
            .args(["--user", "show", "-p", "ControlGroup", "--value", &unit])
            .output()
            .unwrap();
        let path = Path::new("/sys/fs/cgroup").join(
            String::from_utf8(group.stdout)
                .unwrap()
                .trim()
                .trim_start_matches('/'),
        );
        assert_eq!(
            std::fs::read_to_string(path.join("memory.max"))
                .unwrap()
                .trim(),
            "536870912"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("pids.max"))
                .unwrap()
                .trim(),
            "32"
        );
        assert!(
            std::fs::read_to_string(path.join("cpu.max"))
                .unwrap()
                .starts_with("100000 ")
        );
        drop(child);
        for _ in 0..100 {
            if !path.exists()
                || std::fs::read_to_string(path.join("cgroup.events"))
                    .is_ok_and(|s| s.contains("populated 0"))
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("sandbox descendants survived cleanup");
    }
    #[test]
    #[ignore = "requires Bubblewrap and delegated systemd user cgroups"]
    fn missing_manager_fails_closed() {
        let mut sandbox = Sandbox::strategy(Path::new("/bin/sh"), "{}", None).unwrap();
        sandbox.command = manager("/usr/bin/systemd-run").unwrap();
        sandbox
            .command
            .args([
                "--user",
                "--scope",
                "--property=TCInvalidProperty=1",
                "--",
                "/bin/sh",
                "-c",
                "echo escaped",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = sandbox.spawn().unwrap();
        assert!(!child.child.wait().unwrap().success());
    }
}
