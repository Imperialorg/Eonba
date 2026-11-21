#![cfg(target_os = "linux")]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct BuildDir(PathBuf);

impl Drop for BuildDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Each invocation gets its own process group and hard wall-clock limit, including
// native initialization and blocking pipe I/O. No invoking shell or inherited options.
fn tool(dir: &Path, binary: &Path) -> Command {
    let mut cmd = Command::new("/usr/bin/timeout");
    cmd.args(["--kill-after=2s", "30s"])
        .arg(binary)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("HOME", dir)
        .env("TMPDIR", dir)
        .current_dir(dir)
        .stdin(Stdio::null());
    cmd
}

#[test]
#[ignore = "requires Linux with JDI and transport headers (verified OpenJDK 17), cc, setpriv and timeout"]
fn real_jdi_counts_at_only_the_second_visit() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let builds = manifest
        .join("../..")
        .canonicalize()
        .unwrap()
        .join("target");
    fs::create_dir_all(&builds).unwrap();
    assert_eq!(
        builds.canonicalize().unwrap(),
        builds,
        "symlinked build root"
    );
    let path = builds.join(format!("owned-jvm-{}", std::process::id()));
    DirBuilder::new().mode(0o700).create(&path).unwrap();
    let dir = BuildDir(path);
    for (name, source) in [
        (
            "ownedpipe.c",
            include_str!("../fixtures/owned_jvm/ownedpipe.c"),
        ),
        (
            "OwnedFixture.java",
            include_str!("../fixtures/owned_jvm/OwnedFixture.java"),
        ),
        (
            "Inspect.java",
            include_str!("../fixtures/owned_jvm/Inspect.java"),
        ),
    ] {
        fs::write(dir.0.join(name), source).unwrap();
    }
    let java = Path::new("/usr/bin/java").canonicalize().unwrap();
    let jdk = java.parent().unwrap().parent().unwrap();
    let status = tool(&dir.0, Path::new("/usr/bin/cc"))
        .args([
            "-std=c11",
            "-D_POSIX_C_SOURCE=200809L",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "-Werror",
            "-O2",
            "-fPIC",
            "-shared",
            "-I",
        ])
        .arg(jdk.join("include"))
        .arg("-I")
        .arg(jdk.join("include/linux"))
        .arg(dir.0.join("ownedpipe.c"))
        .arg("-o")
        .arg(dir.0.join("libownedpipe.so"))
        .status()
        .unwrap();
    assert!(status.success(), "private JDWP transport build: {status}");

    let status = tool(&dir.0, &jdk.join("bin/javac"))
        .args([
            "-J-XX:-UsePerfData",
            "-J-XX:+DisableAttachMechanism",
            "-J-XX:-CreateCoredumpOnCrash",
            "-J-XX:ActiveProcessorCount=1",
            "-J-Xmx96m",
            "-g:source,lines",
            "-proc:none",
            "--add-modules",
            "jdk.jdi",
            "-d",
        ])
        .arg(&dir.0)
        .arg(format!("-J-Djava.io.tmpdir={}", dir.0.display()))
        .arg(dir.0.join("OwnedFixture.java"))
        .arg(dir.0.join("Inspect.java"))
        .status()
        .unwrap();
    assert!(status.success(), "fixture class build: {status}");

    let status = tool(&dir.0, &java)
        .args([
            "-XX:-UsePerfData",
            "-XX:+DisableAttachMechanism",
            "-XX:-CreateCoredumpOnCrash",
            "-XX:ActiveProcessorCount=1",
            "-Xmx64m",
            "--add-modules",
            "jdk.jdi",
            "-cp",
        ])
        .arg(&dir.0)
        .arg(format!("-Djava.io.tmpdir={}", dir.0.display()))
        .arg("Inspect")
        .status()
        .unwrap();
    assert!(status.success(), "owned JVM inspection: {status}");
    let path = dir.0.clone();
    drop(dir);
    assert!(
        !path.try_exists().unwrap(),
        "fixture build directory survived"
    );
}
