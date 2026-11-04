use super::*;
use std::process::Stdio;

/// Snapshot only regular source files; never execute Cargo/build scripts on the host.
pub fn build_strategy(project: &Path, sdk: &Path, bin: &str, output: &Path) -> Result<Value> {
    build_inner(project, sdk, bin, output, false)
}
#[derive(Debug)]
pub(crate) struct BuildFailure {
    pub code: &'static str,
    pub diagnostics: String,
}
impl std::fmt::Display for BuildFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code)
    }
}
impl std::error::Error for BuildFailure {}
pub(crate) fn build_captured(
    project: &Path,
    sdk: &Path,
    bin: &str,
    output: &Path,
) -> Result<Value> {
    build_inner(project, sdk, bin, output, true)
}
fn build_inner(
    project: &Path,
    sdk: &Path,
    bin: &str,
    output: &Path,
    capture: bool,
) -> Result<Value> {
    let staging = tempfile::tempdir()?;
    let source = staging.path().join("source");
    snapshot(project, &source)?;
    ensure!(
        source.join("Cargo.lock").is_file(),
        "isolated builds require Cargo.lock"
    );
    let trusted = staging.path().join("sdk");
    fs::create_dir_all(&trusted)?;
    fs::copy(sdk.join("Cargo.toml"), trusted.join("Cargo.toml"))?;
    for name in ["tc-core", "tc-sdk"] {
        snapshot(
            &sdk.join("crates").join(name),
            &trusted.join("crates").join(name),
        )?;
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".cargo")))
        .context("Cargo cache location unavailable")?;
    let mut sandbox = tc_sandbox::Sandbox::build(
        &source,
        &trusted,
        &tc_sandbox::toolchain()?,
        &cargo_home.join("registry"),
        bin,
    )?;
    sandbox
        .command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if capture {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    let mut child = sandbox.spawn()?;
    let diagnostics = child.child.stderr.take().map(|mut stream| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0; 8192];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let keep = n.min(65_536_usize.saturating_sub(kept.len()));
                        kept.extend_from_slice(&buffer[..keep]);
                    }
                }
            }
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    let mut bytes = Vec::new();
    // Bound host memory even if the build tries to return an enormous artifact.
    child
        .child
        .stdout
        .take()
        .context("missing compiler output")?
        .take(67_108_865)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 67_108_864,
        "compiled artifact exceeds 64 MiB"
    );
    let status = child.child.wait()?;
    let diagnostics = diagnostics
        .map(|t| {
            t.join()
                .unwrap_or_else(|_| "diagnostic reader failed".into())
        })
        .unwrap_or_default();
    if !status.success() {
        return Err(BuildFailure {
            code: if status.code() == Some(124) {
                "build_timeout"
            } else {
                "compile_failed"
            },
            diagnostics,
        }
        .into());
    }
    ensure!(
        bytes.starts_with(b"\x7fELF"),
        "build did not produce a Linux executable"
    );
    tc_data::atomic_write(output, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(output, fs::Permissions::from_mode(0o755))?;
    }
    Ok(
        json!({"output":output,"sha256":digest(&bytes),"bytes":bytes.len(),"sandbox":"bubblewrap-cgroup-v1","network":"none","offline":true,"diagnostics":diagnostics}),
    )
}
fn snapshot(from: &Path, to: &Path) -> Result<()> {
    ensure!(from.is_dir(), "source directory missing");
    let mut files = vec![];
    collect_source(from, from, &mut files)?;
    ensure!(
        files.len() <= 10_000
            && files.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() <= 104_857_600,
        "source snapshot exceeds limits"
    );
    for (name, bytes) in files {
        tc_data::atomic_write(&to.join(name), &bytes)?;
    }
    Ok(())
}

pub fn sandbox_check() -> Result<Value> {
    let mut sandbox = tc_sandbox::Sandbox::strategy(Path::new("/usr/bin/true"), "{}", None)?;
    sandbox.command.stdin(Stdio::null()).stdout(Stdio::null());
    let mut child = sandbox.spawn()?;
    ensure!(
        child.child.wait()?.success(),
        "sandbox probe failed; check Bubblewrap AppArmor permissions and delegated systemd user cgroups"
    );
    Ok(
        json!({"ready":true,"backend":"bubblewrap-cgroup-v1","network":"none","memory_bytes":536870912,"tasks":32,"cpu_percent":100}),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn snapshots_reject_symlinks_and_special_files() -> Result<()> {
        let from = tempfile::tempdir()?;
        let to = tempfile::tempdir()?;
        std::os::unix::fs::symlink("/etc/passwd", from.path().join("leak"))?;
        assert!(snapshot(from.path(), to.path()).is_err());
        fs::remove_file(from.path().join("leak"))?;
        let _socket = std::os::unix::net::UnixListener::bind(from.path().join("socket"))?;
        assert!(snapshot(from.path(), to.path()).is_err());
        Ok(())
    }
}
