// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The machine's accelerators, found at run time, and how a sandbox reaches them.
//!
//! A caller states one need: "this sandbox uses the machine's accelerators"
//! ([`MACHINE`] as the start spec's `gpu`). It names no runtime, no vendor and
//! no card. [`discover`] reads what the machine has (PCI devices, drivers,
//! render nodes, the container engine's runtimes and what the sandbox runtime
//! says it supports); [`plan`] decides, from those facts alone, the strongest
//! sandbox that can hold the devices and which cards it gets. Every card the
//! machine has appears in the plan, served or not, with the reason.
//!
//! Nothing here names a card family, a driver version or a count. A card is
//! served when the runtime that would hold it says it can serve it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The start spec's `gpu` for "the machine's accelerators, placed by the runtime".
pub const MACHINE: &str = "machine";

/// PCI vendor ids, as the kernel reports them, of vendors whose devices a
/// container runtime knows how to pass. Other vendors' devices are listed and
/// reported as not passed, with the reason.
pub const PCI_NVIDIA: &str = "0x10de";

/// One accelerator of the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// PCI address (`0000:41:00.0`).
    pub pci: String,
    /// PCI vendor and device id (`0x10de`, `0x1db5`).
    pub vendor: String,
    pub device: String,
    /// The kernel driver bound to it (`nvidia`, `amdgpu`, `i915`, `vfio-pci`), if any.
    #[serde(default)]
    pub driver: Option<String>,
    /// Its DRM render node (`/dev/dri/renderD128`), if any.
    #[serde(default)]
    pub render_node: Option<String>,
    /// The vendor driver's id for it (NVIDIA: the GPU UUID), if the driver reports one.
    #[serde(default)]
    pub uuid: Option<String>,
}

/// What the sandbox runtime (gVisor's `runsc`) says about itself on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRuntime {
    /// The engine registers a runtime with exactly this name.
    pub name: String,
    /// The driver versions its GPU proxy supports (`nvproxy list-supported-drivers`).
    #[serde(default)]
    pub supported_drivers: Vec<String>,
    /// The driver capabilities its GPU proxy allows (from its runtime arguments;
    /// `compute,utility` when none are named).
    #[serde(default)]
    pub driver_capabilities: Vec<String>,
    /// Per card PCI address: whether the runtime says it serves that card, and
    /// why not. `None`: the runtime cannot say per card.
    #[serde(default)]
    pub card_support: Option<BTreeMap<String, Result<(), String>>>,
}

/// What [`discover`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineFacts {
    /// `linux`, `macos`, `windows`.
    pub os: String,
    pub cards: Vec<Card>,
    /// The NVIDIA kernel module's version (`/sys/module/nvidia/version`), if loaded.
    #[serde(default)]
    pub nvidia_driver: Option<String>,
    /// The engine has the NVIDIA container runtime (its device requests reach cards).
    pub nvidia_container_runtime: bool,
    /// gVisor, when the engine registers it.
    #[serde(default)]
    pub sandbox_runtime: Option<SandboxRuntime>,
}

/// How one card fares in a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPlan {
    pub pci: String,
    pub vendor: String,
    pub served: bool,
    /// Why it is not served (empty when served). `unverified` notes go here too.
    #[serde(default)]
    pub reason: String,
}

/// The placement of a sandbox that uses the machine's accelerators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccelPlan {
    /// The OCI runtime the sandbox runs on (`runsc`), or `None` when the
    /// machine has no sandbox runtime and the engine's own choice stands.
    pub runtime: Option<String>,
    /// NVIDIA cards to request, by the driver's id (empty: none).
    pub nvidia_devices: Vec<String>,
    /// `NVIDIA_DRIVER_CAPABILITIES` for the sandbox, when it gets NVIDIA cards.
    pub nvidia_capabilities: Option<String>,
    /// Every card of the machine, served or not.
    pub cards: Vec<CardPlan>,
}

impl AccelPlan {
    pub fn served(&self) -> impl Iterator<Item = &CardPlan> {
        self.cards.iter().filter(|c| c.served)
    }
}

/// Decides, from the facts alone, how a sandbox reaches the machine's
/// accelerators: the strongest sandbox first, never a weaker one for a card.
///
/// - gVisor (`runsc`) when the engine registers it: NVIDIA cards go in when
///   the engine has the NVIDIA container runtime, the host driver is in the
///   proxy's supported list, and the runtime does not say it cannot serve the
///   card. The capabilities are those the proxy allows.
/// - Without gVisor, no card goes in: a card through a plain container would
///   hand the host's driver to the sandbox. The reason says what is missing.
/// - A vendor no runtime here passes is listed with that reason.
pub fn plan(facts: &MachineFacts) -> AccelPlan {
    let sandbox = facts.sandbox_runtime.as_ref();
    let mut cards = Vec::new();
    let mut nvidia_devices = Vec::new();
    for card in &facts.cards {
        let refuse = |reason: String| CardPlan {
            pci: card.pci.clone(),
            vendor: card.vendor.clone(),
            served: false,
            reason,
        };
        let decided = if facts.os != "linux" {
            refuse(format!(
                "The machine runs {}; its sandboxes reach no PCI device",
                facts.os
            ))
        } else if card.vendor != PCI_NVIDIA {
            refuse("The sandbox runtime has no proxy for this vendor's devices".into())
        } else if sandbox.is_none() {
            refuse(
                "The machine has no gVisor runtime; a card goes only into a gVisor sandbox".into(),
            )
        } else if !facts.nvidia_container_runtime {
            refuse("The container engine has no NVIDIA container runtime".into())
        } else if card.driver.as_deref() != Some("nvidia") {
            refuse(format!(
                "The card is bound to {}, not to the NVIDIA driver",
                card.driver.as_deref().unwrap_or("no driver")
            ))
        } else if card.uuid.is_none() {
            refuse("The NVIDIA driver reports no id for this card".into())
        } else {
            let Some(sandbox) = sandbox else {
                unreachable!("a machine without gVisor is refused above")
            };
            let driver = facts.nvidia_driver.as_deref().unwrap_or("");
            if !sandbox.supported_drivers.iter().any(|d| d == driver) {
                refuse(format!(
                    "The gVisor GPU proxy does not know NVIDIA driver {}",
                    if driver.is_empty() {
                        "(none loaded)"
                    } else {
                        driver
                    }
                ))
            } else {
                match sandbox.card_support.as_ref().map(|all| all.get(&card.pci)) {
                    Some(Some(Err(why))) => refuse(why.clone()),
                    Some(None) => refuse("The gVisor runtime did not report this card".into()),
                    support => {
                        nvidia_devices.push(card.uuid.clone().expect("checked above"));
                        CardPlan {
                            pci: card.pci.clone(),
                            vendor: card.vendor.clone(),
                            served: true,
                            reason: if support.is_none() {
                                "unverified: the gVisor runtime cannot check a card before use"
                                    .into()
                            } else {
                                String::new()
                            },
                        }
                    }
                }
            }
        };
        cards.push(decided);
    }
    let nvidia_capabilities = (!nvidia_devices.is_empty()).then(|| {
        let allowed = &sandbox
            .expect("devices imply a sandbox")
            .driver_capabilities;
        if allowed.is_empty() {
            "compute,utility".to_string()
        } else {
            allowed.join(",")
        }
    });
    AccelPlan {
        runtime: sandbox.map(|s| s.name.clone()),
        nvidia_devices,
        nvidia_capabilities,
        cards,
    }
}

/// The machine's accelerators as a GPU report row (runtime `machine`): one option
/// per card, by its PCI address, supported when the plan serves it, with the reason
/// when not. The existing GPU report carries it to a caller unchanged.
pub fn machine_support(facts: &MachineFacts) -> crate::gpu::GpuSupport {
    let planned = plan(facts);
    let by_pci: BTreeMap<&str, &Card> = facts.cards.iter().map(|c| (c.pci.as_str(), c)).collect();
    crate::gpu::GpuSupport {
        runtime: MACHINE.into(),
        options: planned
            .cards
            .iter()
            .map(|c| {
                let card = by_pci.get(c.pci.as_str());
                crate::gpu::GpuOption {
                    id: c.pci.clone(),
                    label: format!(
                        "{} {}{}",
                        c.vendor,
                        card.map(|k| k.device.as_str()).unwrap_or(""),
                        card.and_then(|k| k.uuid.as_deref())
                            .map(|u| format!(" {u}"))
                            .unwrap_or_default()
                    ),
                    experimental: false,
                    supported: c.served,
                    reason: c.reason.clone(),
                    learn_more: None,
                    usd_per_hour: None,
                }
            })
            .collect(),
        reason: if facts.cards.is_empty() {
            "The machine has no accelerators".into()
        } else {
            String::new()
        },
    }
}

/// The driver capabilities a `runsc` entry's arguments allow its GPU proxy.
pub fn allowed_capabilities(runtime_args: &[String]) -> Vec<String> {
    const FLAG: &str = "--nvproxy-allowed-driver-capabilities";
    let mut found = None;
    let mut args = runtime_args.iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix(&format!("{FLAG}=")) {
            found = Some(value.to_string());
        } else if arg == FLAG {
            found = args.next().cloned();
        }
    }
    found
        .map(|v| {
            v.split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The driver versions in `runsc nvproxy list-supported-drivers` output.
pub fn supported_drivers(output: &str) -> Vec<String> {
    output
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .filter(|w| w.contains('.'))
        .map(str::to_string)
        .collect()
}

/// PCI class codes (`/sys/bus/pci/devices/*/class`, upper byte) of
/// accelerators: display controllers (0x03) and processing accelerators (0x12).
fn accelerator_class(class: &str) -> bool {
    let class = class.trim().trim_start_matches("0x");
    class.starts_with("03") || class.starts_with("12")
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// The machine's accelerators under `sys` (`/sys` on a real machine) and the
/// NVIDIA driver's own records under `proc_nvidia` (`/proc/driver/nvidia`).
/// Reads files only; runs nothing and opens no device.
pub fn cards(sys: &Path, proc_nvidia: &Path) -> Vec<Card> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(sys.join("bus/pci/devices")) else {
        return found;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for dev in paths {
        let Some(class) = read(&dev.join("class")) else {
            continue;
        };
        if !accelerator_class(&class) {
            continue;
        }
        let pci = dev
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let driver = std::fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        let render_node = std::fs::read_dir(dev.join("drm")).ok().and_then(|nodes| {
            let mut names: Vec<String> = nodes
                .filter_map(|n| n.ok())
                .map(|n| n.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("renderD"))
                .collect();
            names.sort();
            names.first().map(|n| format!("/dev/dri/{n}"))
        });
        let uuid =
            read(&proc_nvidia.join("gpus").join(&pci).join("information")).and_then(|info| {
                info.lines()
                    .find_map(|l| l.strip_prefix("GPU UUID:").map(|v| v.trim().to_string()))
            });
        found.push(Card {
            pci,
            vendor: read(&dev.join("vendor")).unwrap_or_default(),
            device: read(&dev.join("device")).unwrap_or_default(),
            driver,
            render_node,
            uuid,
        });
    }
    found
}

/// The NVIDIA kernel module's version, if loaded.
pub fn nvidia_driver(sys: &Path) -> Option<String> {
    read(&sys.join("module/nvidia/version")).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia(pci: &str, uuid: &str) -> Card {
        Card {
            pci: pci.into(),
            vendor: PCI_NVIDIA.into(),
            device: "0x0000".into(),
            driver: Some("nvidia".into()),
            render_node: None,
            uuid: Some(uuid.into()),
        }
    }

    fn gvisor(drivers: &[&str]) -> SandboxRuntime {
        SandboxRuntime {
            name: "runsc".into(),
            supported_drivers: drivers.iter().map(|d| d.to_string()).collect(),
            driver_capabilities: vec![],
            card_support: None,
        }
    }

    fn linux(cards: Vec<Card>, sandbox: Option<SandboxRuntime>) -> MachineFacts {
        MachineFacts {
            os: "linux".into(),
            cards,
            nvidia_driver: Some("1.2.3".into()),
            nvidia_container_runtime: true,
            sandbox_runtime: sandbox,
        }
    }

    #[test]
    fn a_machine_without_accelerators_plans_none_and_keeps_its_sandbox() {
        let p = plan(&linux(vec![], Some(gvisor(&["1.2.3"]))));
        assert_eq!(p.runtime.as_deref(), Some("runsc"));
        assert!(p.nvidia_devices.is_empty() && p.cards.is_empty());
        assert_eq!(p.nvidia_capabilities, None);
    }

    #[test]
    fn every_nvidia_card_the_proxy_knows_goes_into_the_gvisor_sandbox() {
        let p = plan(&linux(
            vec![
                nvidia("0000:01:00.0", "GPU-a"),
                nvidia("0000:02:00.0", "GPU-b"),
            ],
            Some(gvisor(&["1.2.3"])),
        ));
        assert_eq!(p.runtime.as_deref(), Some("runsc"));
        assert_eq!(p.nvidia_devices, vec!["GPU-a", "GPU-b"]);
        assert_eq!(p.nvidia_capabilities.as_deref(), Some("compute,utility"));
        // No per-card answer from the runtime: served, marked unverified.
        assert!(
            p.cards
                .iter()
                .all(|c| c.served && c.reason.starts_with("unverified"))
        );
    }

    #[test]
    fn a_driver_the_proxy_does_not_know_keeps_every_card_out_with_the_reason() {
        let p = plan(&linux(
            vec![nvidia("0000:01:00.0", "GPU-a")],
            Some(gvisor(&["9.9.9"])),
        ));
        assert!(p.nvidia_devices.is_empty());
        assert!(!p.cards[0].served);
        assert!(
            p.cards[0]
                .reason
                .contains("does not know NVIDIA driver 1.2.3")
        );
    }

    #[test]
    fn a_card_the_runtime_says_it_cannot_serve_stays_out_and_the_others_go_in() {
        let mut sandbox = gvisor(&["1.2.3"]);
        sandbox.card_support = Some(BTreeMap::from([
            ("0000:01:00.0".to_string(), Ok(())),
            (
                "0000:02:00.0".to_string(),
                Err("The proxy has no classes for this card".to_string()),
            ),
        ]));
        let p = plan(&linux(
            vec![
                nvidia("0000:01:00.0", "GPU-a"),
                nvidia("0000:02:00.0", "GPU-b"),
                nvidia("0000:03:00.0", "GPU-c"),
            ],
            Some(sandbox),
        ));
        assert_eq!(p.nvidia_devices, vec!["GPU-a"]);
        assert_eq!((p.cards[0].served, p.cards[0].reason.as_str()), (true, ""));
        assert_eq!(p.cards[1].reason, "The proxy has no classes for this card");
        assert_eq!(
            p.cards[2].reason,
            "The gVisor runtime did not report this card"
        );
    }

    #[test]
    fn without_gvisor_no_card_goes_into_a_plain_container() {
        let p = plan(&linux(vec![nvidia("0000:01:00.0", "GPU-a")], None));
        assert_eq!(p.runtime, None);
        assert!(p.nvidia_devices.is_empty());
        assert!(p.cards[0].reason.contains("no gVisor runtime"));
    }

    #[test]
    fn other_vendors_are_listed_and_not_passed() {
        let amd = Card {
            pci: "0000:05:00.0".into(),
            vendor: "0x1002".into(),
            device: "0x744c".into(),
            driver: Some("amdgpu".into()),
            render_node: Some("/dev/dri/renderD129".into()),
            uuid: None,
        };
        let p = plan(&linux(
            vec![amd, nvidia("0000:01:00.0", "GPU-a")],
            Some(gvisor(&["1.2.3"])),
        ));
        assert_eq!(p.nvidia_devices, vec!["GPU-a"]);
        assert!(!p.cards[0].served && p.cards[0].reason.contains("no proxy for this vendor"));
    }

    #[test]
    fn a_card_bound_to_vfio_or_without_the_container_runtime_stays_out() {
        let mut vfio = nvidia("0000:01:00.0", "GPU-a");
        vfio.driver = Some("vfio-pci".into());
        let p = plan(&linux(vec![vfio], Some(gvisor(&["1.2.3"]))));
        assert!(p.cards[0].reason.contains("bound to vfio-pci"));
        let mut facts = linux(
            vec![nvidia("0000:01:00.0", "GPU-a")],
            Some(gvisor(&["1.2.3"])),
        );
        facts.nvidia_container_runtime = false;
        assert!(
            plan(&facts).cards[0]
                .reason
                .contains("no NVIDIA container runtime")
        );
    }

    #[test]
    fn a_mac_or_windows_machine_passes_no_pci_device() {
        let mut facts = linux(
            vec![nvidia("0000:01:00.0", "GPU-a")],
            Some(gvisor(&["1.2.3"])),
        );
        facts.os = "windows".into();
        assert!(plan(&facts).cards[0].reason.contains("runs windows"));
    }

    #[test]
    fn the_capabilities_are_those_the_proxy_allows() {
        assert_eq!(
            allowed_capabilities(&[
                "--allow-suid".into(),
                "--nvproxy-allowed-driver-capabilities=compute,utility,video".into()
            ]),
            vec!["compute", "utility", "video"]
        );
        assert_eq!(
            allowed_capabilities(&[
                "--nvproxy-allowed-driver-capabilities".into(),
                "graphics".into()
            ]),
            vec!["graphics"]
        );
        assert!(allowed_capabilities(&[]).is_empty());
        let mut sandbox = gvisor(&["1.2.3"]);
        sandbox.driver_capabilities = vec!["compute".into(), "utility".into(), "graphics".into()];
        let p = plan(&linux(vec![nvidia("0000:01:00.0", "GPU-a")], Some(sandbox)));
        assert_eq!(
            p.nvidia_capabilities.as_deref(),
            Some("compute,utility,graphics")
        );
    }

    #[test]
    fn the_report_row_names_every_card_served_or_not() {
        let mut sandbox = gvisor(&["1.2.3"]);
        sandbox.card_support = Some(BTreeMap::from([
            ("0000:01:00.0".to_string(), Ok(())),
            ("0000:02:00.0".to_string(), Err("no classes".to_string())),
        ]));
        let row = machine_support(&linux(
            vec![
                nvidia("0000:01:00.0", "GPU-a"),
                nvidia("0000:02:00.0", "GPU-b"),
            ],
            Some(sandbox),
        ));
        assert_eq!(row.runtime, MACHINE);
        assert_eq!(row.options.len(), 2);
        assert!(row.options[0].supported && row.options[0].label.contains("GPU-a"));
        assert_eq!(
            (row.options[1].supported, row.options[1].reason.as_str()),
            (false, "no classes")
        );
        let none = machine_support(&linux(vec![], None));
        assert_eq!(none.reason, "The machine has no accelerators");
    }

    #[test]
    fn the_supported_driver_list_is_read_from_the_runtime_output() {
        let out = "580.126.20\n580.178.04\n580.196.07\n";
        assert_eq!(
            supported_drivers(out),
            vec!["580.126.20", "580.178.04", "580.196.07"]
        );
        assert!(supported_drivers("no drivers").is_empty());
    }

    #[test]
    fn cards_are_read_from_sysfs_and_the_driver_records() {
        let dir = tempfile::tempdir().unwrap();
        let sys = dir.path().join("sys");
        let procn = dir.path().join("proc");
        let dev = sys.join("bus/pci/devices/0000:41:00.0");
        std::fs::create_dir_all(dev.join("drm/renderD128")).unwrap();
        std::fs::write(dev.join("class"), "0x030200\n").unwrap();
        std::fs::write(dev.join("vendor"), "0x10de\n").unwrap();
        std::fs::write(dev.join("device"), "0x20b5\n").unwrap();
        let drivers = sys.join("bus/pci/drivers/nvidia");
        std::fs::create_dir_all(&drivers).unwrap();
        std::os::unix::fs::symlink(&drivers, dev.join("driver")).unwrap();
        let nic = sys.join("bus/pci/devices/0000:01:00.0");
        std::fs::create_dir_all(&nic).unwrap();
        std::fs::write(nic.join("class"), "0x020000\n").unwrap();
        std::fs::create_dir_all(procn.join("gpus/0000:41:00.0")).unwrap();
        std::fs::write(
            procn.join("gpus/0000:41:00.0/information"),
            "Model: X\nGPU UUID: GPU-1234\n",
        )
        .unwrap();
        std::fs::create_dir_all(sys.join("module/nvidia")).unwrap();
        std::fs::write(sys.join("module/nvidia/version"), "1.2.3\n").unwrap();
        let found = cards(&sys, &procn);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].driver.as_deref(), Some("nvidia"));
        assert_eq!(found[0].render_node.as_deref(), Some("/dev/dri/renderD128"));
        assert_eq!(found[0].uuid.as_deref(), Some("GPU-1234"));
        assert_eq!(nvidia_driver(&sys).as_deref(), Some("1.2.3"));
    }
}
