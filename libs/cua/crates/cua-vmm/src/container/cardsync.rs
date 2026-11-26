//! Keeps the GPU cards of running personal Spaces equal to the host's cards
//! (#3282, design section 3): one kernel-event watcher per process, and a
//! reconcile at each Space start. It never runs nvidia-smi and never opens a
//! card; it reads `/dev`, `/proc` and runsc's state files.
//!
//! runsc and its device gofer exist only on Linux. Elsewhere a Space has no
//! card nodes to follow: [`reconcile`] finds nothing and no watcher starts.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bollard::Docker;

use crate::devwatch::{self, DeviceSource, NodeStep};

/// The first kernel with `open_tree` and `move_mount` (5.2). Below it a card
/// that arrives while a Space runs reaches the Space at its next start.
const KERNEL_FLOOR: (u32, u32) = (5, 2);

/// Whether kernel release `release` ("6.8.0-49-generic") has the mount API.
pub(crate) fn kernel_has_mount_api(release: &str) -> bool {
    let mut parts = release
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major, minor) >= KERNEL_FLOOR
}

#[cfg(target_os = "linux")]
fn kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default()
}

/// The card numbers of the nodes in directory `dev`.
fn dev_cards(dev: &Path) -> BTreeSet<u32> {
    let names: Vec<String> = std::fs::read_dir(dev)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    devwatch::card_minors(names.iter().map(String::as_str))
}

/// The three card sets of one running Space.
#[cfg(target_os = "linux")]
struct Live {
    gofer_pid: i32,
    space: BTreeSet<u32>,
}

#[cfg(target_os = "linux")]
impl DeviceSource for Live {
    fn host(&self) -> BTreeSet<u32> {
        dev_cards(Path::new("/dev"))
    }
    fn gofer(&self) -> BTreeSet<u32> {
        dev_cards(&PathBuf::from(format!("/proc/{}/root/dev", self.gofer_pid)))
    }
    fn space(&self) -> BTreeSet<u32> {
        self.space.clone()
    }
}

/// The `--root` of a runsc sandbox process for container `id`, from its
/// arguments (`runsc --root=R ... boot ... <id>`); `None` for any other process.
pub(crate) fn runsc_boot_root(args: &[String], id: &str) -> Option<PathBuf> {
    let program = Path::new(args.first()?).file_name()?.to_string_lossy();
    if !program.contains("runsc") || !args.iter().any(|a| a == "boot") || args.last()? != id {
        return None;
    }
    args.iter().enumerate().find_map(|(i, a)| {
        a.strip_prefix("--root=").map(PathBuf::from).or_else(|| {
            (a == "--root")
                .then(|| args.get(i + 1).map(PathBuf::from))
                .flatten()
        })
    })
}

/// The sandbox process of container `id` and its runsc root as that process
/// sees it.
#[cfg(target_os = "linux")]
fn find_sandbox(id: &str) -> Option<(i32, PathBuf)> {
    std::fs::read_dir("/proc").ok()?.flatten().find_map(|e| {
        let pid: i32 = e.file_name().to_str()?.parse().ok()?;
        let raw = std::fs::read(e.path().join("cmdline")).ok()?;
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).to_string())
            .collect();
        runsc_boot_root(&args, id).map(|root| (pid, root))
    })
}

/// Brings one running personal Space's card nodes to the host's cards and
/// returns the steps it carried out (none when the Space is not under runsc,
/// not running, or already equal).
#[cfg(target_os = "linux")]
pub async fn reconcile(docker: &Docker, name: &str) -> std::result::Result<Vec<NodeStep>, String> {
    let info = match docker.inspect_container(name, None).await {
        Ok(i) => i,
        Err(e) => return Err(e.to_string()),
    };
    let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
    let personal = info
        .config
        .as_ref()
        .and_then(|c| c.labels.as_ref())
        .is_some_and(|l| crate::accel::is_personal(l.iter()));
    let Some(id) = info.id.clone() else {
        return Ok(vec![]);
    };
    if !running || !personal {
        return Ok(vec![]);
    }
    let found = {
        let id = id.clone();
        tokio::task::spawn_blocking(move || find_sandbox(&id))
            .await
            .map_err(|e| e.to_string())?
    };
    let Some((sandbox_pid, root)) = found else {
        return Ok(vec![]);
    };
    let root_on_host = PathBuf::from(format!("/proc/{sandbox_pid}/root"))
        .join(root.strip_prefix("/").unwrap_or(&root));
    let gofer_pid = devwatch::gofer_pid(&root_on_host, &id)
        .ok_or_else(|| format!("no device gofer in {}", root_on_host.display()))?;
    let exec = super::DockerExec {
        docker: docker.clone(),
        container: name.to_string(),
    };
    let listing = crate::exec::GuestExec::exec(
        &exec,
        crate::exec::ExecRequest::sh("ls -1 /dev").timeout(std::time::Duration::from_secs(20)),
    )
    .await
    .map_err(|e| e.to_string())?;
    let names = String::from_utf8_lossy(&listing.stdout).to_string();
    let space = devwatch::card_minors(names.lines());
    let steps = devwatch::plan(&Live { gofer_pid, space });
    if steps.is_empty() {
        return Ok(steps);
    }
    let moves_nodes = steps.iter().any(|s| !matches!(s, NodeStep::RefreshSpace));
    if moves_nodes && !kernel_has_mount_api(&kernel_release()) {
        tracing::warn!(
            name,
            "this kernel has no open_tree/move_mount: the Space gets the changed cards at its next start"
        );
        return Ok(vec![]);
    }
    let runsc = std::fs::read_link(format!("/proc/{sandbox_pid}/exe"))
        .map_err(|e| format!("runsc of the Space: {e}"))?;
    let nodes = devwatch::GoferNodes {
        gofer_pid,
        runsc,
        runsc_root: root,
        container_id: id,
        enter_pid: Some(sandbox_pid),
    };
    let done = steps.clone();
    tokio::task::spawn_blocking(move || nodes.apply(&steps))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(done)
}

/// Without runsc no Space has card nodes to follow.
#[cfg(not(target_os = "linux"))]
pub async fn reconcile(
    _docker: &Docker,
    _name: &str,
) -> std::result::Result<Vec<NodeStep>, String> {
    Ok(vec![])
}

/// What a card event changes: NVIDIA's CDI spec and our class in the VM
/// service's CDI folder, then the nodes of every running personal Space.
pub async fn on_cards_changed(docker: &Docker) {
    if let Some(dir) = crate::cdiclass::dir() {
        let out = dir.join(crate::cdiclass::NVIDIA_FILE);
        match tokio::process::Command::new("nvidia-ctk")
            .args(["cdi", "generate", "--quiet"])
            .arg(format!("--output={}", out.display()))
            .stdin(std::process::Stdio::null())
            .output()
            .await
        {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                tracing::warn!(stderr = %String::from_utf8_lossy(&o.stderr), "nvidia-ctk cdi generate failed")
            }
            Err(e) => {
                tracing::warn!(error = %e, "nvidia-ctk is not available to rewrite the CDI spec")
            }
        }
        let plan = crate::accel::plan(&super::machine_facts_of(docker).await);
        let served = plan.served().filter_map(|c| c.minor).collect();
        if let Err(e) = crate::cdiclass::write(&dir, &served) {
            tracing::warn!(error = %e, "could not write the Space's accelerator class");
        }
    }
    let filter = format!("{}={}", crate::accel::LABEL_SPACE, crate::accel::PERSONAL);
    let list = docker
        .list_containers(Some(bollard::query_parameters::ListContainersOptions {
            all: false,
            filters: Some(std::collections::HashMap::from([(
                "label".to_string(),
                vec![filter],
            )])),
            ..Default::default()
        }))
        .await;
    for c in list.unwrap_or_default() {
        let Some(name) = c.names.and_then(|n| n.first().cloned()) else {
            continue;
        };
        let name = name.trim_start_matches('/').to_string();
        match reconcile(docker, &name).await {
            Ok(steps) if !steps.is_empty() => {
                tracing::info!(name, ?steps, "the Space follows the host's cards")
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(name, error = %e, "the Space could not follow the host's cards")
            }
        }
    }
}

/// Starts the kernel-event watcher once per process. Events come in bursts
/// (bind, then the driver's own events), so one reconcile runs after the
/// burst is over.
#[cfg(target_os = "linux")]
pub fn start_watcher(docker: Docker) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("cua-cardwatch".into())
            .spawn(move || {
                if let Err(e) = devwatch::watch_kernel(|| {
                    let _ = tx.send(());
                }) {
                    tracing::warn!(error = %e, "the card watcher stopped; Spaces get changed cards at their next start");
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not start the card watcher");
            return;
        }
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                while rx.try_recv().is_ok() {}
                on_cards_changed(&docker).await;
            }
        });
    });
}

/// The kernel's device events are a Linux socket; elsewhere there is nothing to watch.
#[cfg(not(target_os = "linux"))]
pub fn start_watcher(_docker: Docker) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_kernel_floor_is_five_two() {
        assert!(kernel_has_mount_api("6.8.0-49-generic"));
        assert!(kernel_has_mount_api("5.2.0"));
        assert!(!kernel_has_mount_api("5.1.21"));
        assert!(!kernel_has_mount_api("4.19.0-27-amd64"));
        assert!(!kernel_has_mount_api(""));
    }

    #[test]
    fn the_sandbox_process_names_its_root() {
        let id = "c0ffee";
        assert_eq!(
            runsc_boot_root(
                &args(&[
                    "/usr/bin/runsc",
                    "--root=/run/user/1001/docker/runtime-runsc/moby",
                    "--nvproxy",
                    "boot",
                    "--bundle",
                    "/b",
                    id
                ]),
                id
            ),
            Some(PathBuf::from("/run/user/1001/docker/runtime-runsc/moby"))
        );
        assert_eq!(
            runsc_boot_root(&args(&["runsc-sandbox", "--root", "/r", "boot", id]), id),
            Some(PathBuf::from("/r"))
        );
        // The gofer, another container's sandbox, and other programs are not it.
        assert_eq!(
            runsc_boot_root(&args(&["runsc-gofer", "--root=/r", "gofer", id]), id),
            None
        );
        assert_eq!(
            runsc_boot_root(&args(&["runsc", "--root=/r", "boot", "other"]), id),
            None
        );
        assert_eq!(
            runsc_boot_root(&args(&["bash", "--root=/r", "boot", id]), id),
            None
        );
    }
}
