//! Helpers shared by this crate's unit tests.

/// Whether a unit test that starts a real VM or container (or another process that can
/// outlive the test) may run: only when the variable `var` is `1`. Such a test is also
/// `#[ignore]`d with a reason that names `var`, so a plain `cargo test` lists it as
/// ignored and why; this gate keeps `--ignored` alone from starting anything.
pub(crate) fn may_start(var: &str, test: &str) -> bool {
    may_start_with(std::env::var(var).ok().as_deref(), var, test)
}

fn may_start_with(value: Option<&str>, var: &str, test: &str) -> bool {
    let run = value == Some("1");
    if !run {
        eprintln!("skipped {test}: it starts a real VM or container; set {var}=1 to run it");
    }
    run
}

/// Makes `path`, a script a test just wrote, executable with `mode` as a new file.
///
/// A test that writes a script and then runs it races every other test thread: a fork
/// in another thread between the write and the close copies the write handle, and the
/// exec then fails with "Text file busy" (ETXTBSY) until that child execs. A child
/// process here copies the script to a new file, sets its mode and renames it over the
/// old name, so the file that runs was never open for writing in this process.
#[cfg(unix)]
pub(crate) fn seal_exec(path: impl AsRef<std::path::Path>, mode: u32) {
    let path = path.as_ref();
    let status = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "cp \"$1\" \"$1.sealed\" && chmod \"$2\" \"$1.sealed\" && mv -f \"$1.sealed\" \"$1\"",
            "seal_exec",
        ])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(std::process::Stdio::null())
        .status()
        .expect("run /bin/sh to seal a test script");
    assert!(
        status.success(),
        "could not seal test script {}",
        path.display()
    );
}

#[cfg(test)]
mod gate_tests {
    use super::may_start_with;

    #[test]
    fn only_the_value_1_lets_a_vm_test_run() {
        assert!(!may_start_with(None, "CUA_VMM_TEST_QEMU", "t"));
        assert!(!may_start_with(Some(""), "CUA_VMM_TEST_QEMU", "t"));
        assert!(!may_start_with(Some("0"), "CUA_VMM_TEST_QEMU", "t"));
        assert!(!may_start_with(Some("true"), "CUA_VMM_TEST_QEMU", "t"));
        assert!(may_start_with(Some("1"), "CUA_VMM_TEST_QEMU", "t"));
    }

    /// Every unit test ignored because it starts a VM or container names its variable
    /// in the reason and asks the gate before it starts anything.
    #[test]
    fn every_vm_starting_unit_test_is_gated() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = 0;
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    let l = line.trim_start();
                    if !(l.starts_with("#[ignore = \"starts")
                        || l.starts_with("#[ignore = \"boots"))
                    {
                        continue;
                    }
                    seen += 1;
                    let var = l
                        .split_whitespace()
                        .find(|w| w.starts_with("CUA_VMM_TEST_"))
                        .map(|w| {
                            w.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .next()
                                .unwrap()
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "{}:{} names no CUA_VMM_TEST_ variable",
                                path.display(),
                                i + 1
                            )
                        });
                    let body: String = lines[i..lines.len().min(i + 12)]
                        .concat()
                        .split_whitespace()
                        .collect();
                    assert!(
                        body.contains(&format!("may_start(\"{var}\"")),
                        "{}:{} does not ask crate::testutil::may_start(\"{var}\", ..)",
                        path.display(),
                        i + 1
                    );
                }
            }
        }
        assert!(seen >= 3, "found {seen} gated tests");
    }
}
