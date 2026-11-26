//! The Space's own CDI device class, `flip.engineering/accelerators` (#3282,
//! design section 3).
//!
//! A personal Space is made once with one standing request, `CLASS_ALL`. The
//! engine resolves it from this class's spec file at each start, so the
//! request names no vendor and is valid on a machine with no card. The VM
//! service writes the file from the machine as it is: the device edits of
//! NVIDIA's generated spec (`nvidia.yaml`, written by `nvidia-ctk cdi
//! generate`) for the cards the GPU proxy serves, and nothing of a card it
//! cannot serve. With no served card the class holds one harmless edit, so the
//! spec stays valid and the Space starts without a GPU.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};

/// The class's CDI kind.
pub const KIND: &str = "flip.engineering/accelerators";
/// The Space's standing request: every device of the class.
pub const CLASS_ALL: &str = "flip.engineering/accelerators=all";
/// The class's spec file in the CDI directory.
pub const FILE: &str = "flip.engineering-accelerators.yaml";
/// NVIDIA's generated spec in the same directory (the updater writes it).
pub const NVIDIA_FILE: &str = "nvidia.yaml";
/// Names the CDI directory the engine reads, where the class file lives.
pub const DIR_ENV: &str = "CUA_CDI_SPEC_DIR";

/// The CDI directory of the class, when the VM service has one.
pub fn dir() -> Option<std::path::PathBuf> {
    std::env::var_os(DIR_ENV)
        .map(std::path::PathBuf::from)
        .filter(|d| d.is_dir())
}

/// The class spec for the served cards (their device minors) from NVIDIA's
/// spec (`None` when there is none). Each NVIDIA device whose own card node
/// is a served card gives all its edits; the spec's common edits come with
/// the first served card. Edits that repeat are kept once.
pub fn class_spec(nvidia: Option<&Value>, served: &BTreeSet<u32>) -> Value {
    let version = nvidia
        .and_then(|n| n.get("cdiVersion"))
        .cloned()
        .unwrap_or_else(|| json!("0.6.0"));
    let mut edits = Map::new();
    let mut common = None;
    let mut cards = 0usize;
    if let Some(spec) = nvidia {
        for device in spec
            .get("devices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(own) = device.get("containerEdits") else {
                continue;
            };
            let minors = card_minors(own);
            // A device of exactly one card that is served (not NVIDIA's "all").
            if minors.len() == 1 && served.contains(&minors[0]) {
                merge(&mut edits, own);
                cards += 1;
            }
        }
        if cards > 0 {
            common = spec.get("containerEdits").cloned();
        }
    }
    let mut device_edits = edits;
    if let Some(Value::Object(common)) = &common {
        merge(&mut device_edits, &Value::Object(common.clone()));
    }
    // A device needs at least one edit; this one also tells a program in the
    // Space how many cards it has.
    let env = device_edits
        .entry("env")
        .or_insert_with(|| Value::Array(vec![]));
    if let Value::Array(env) = env {
        env.push(json!(format!("FLIP_ACCELERATORS={cards}")));
    }
    json!({
        "cdiVersion": version,
        "kind": KIND,
        "devices": [{"name": "all", "containerEdits": Value::Object(device_edits)}],
    })
}

/// The `/dev/nvidia<N>` minors among an edit set's device nodes.
fn card_minors(edits: &Value) -> Vec<u32> {
    edits
        .get("deviceNodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| n.get("path").and_then(Value::as_str))
        .filter_map(|p| p.strip_prefix("/dev/nvidia"))
        .filter(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|rest| rest.parse().ok())
        .collect()
}

/// Adds `from`'s edit lists to `into`, keeping each entry once.
fn merge(into: &mut Map<String, Value>, from: &Value) {
    let Some(from) = from.as_object() else {
        return;
    };
    for (key, value) in from {
        let Some(items) = value.as_array() else {
            into.entry(key.clone()).or_insert_with(|| value.clone());
            continue;
        };
        let list = into
            .entry(key.clone())
            .or_insert_with(|| Value::Array(vec![]));
        if let Value::Array(list) = list {
            for item in items {
                if !list.contains(item) {
                    list.push(item.clone());
                }
            }
        }
    }
}

/// Writes the class spec into `dir` for the served cards, reading NVIDIA's
/// spec from the same directory. The file is replaced in one rename, so the
/// engine never reads half of it.
pub fn write(dir: &Path, served: &BTreeSet<u32>) -> std::io::Result<()> {
    let nvidia: Option<Value> = std::fs::read_to_string(dir.join(NVIDIA_FILE))
        .ok()
        .and_then(|text| serde_yaml_ng::from_str(&text).ok());
    let spec = class_spec(nvidia.as_ref(), served);
    let text = serde_yaml_ng::to_string(&spec).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".{FILE}.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia_spec() -> Value {
        serde_yaml_ng::from_str(
            r#"
cdiVersion: 0.5.0
kind: nvidia.com/gpu
devices:
- name: "0"
  containerEdits:
    deviceNodes:
    - path: /dev/nvidia0
    - path: /dev/dri/card1
- name: "1"
  containerEdits:
    deviceNodes:
    - path: /dev/nvidia1
- name: all
  containerEdits:
    deviceNodes:
    - path: /dev/nvidia0
    - path: /dev/nvidia1
    - path: /dev/dri/card1
containerEdits:
  deviceNodes:
  - path: /dev/nvidiactl
  - path: /dev/nvidia-uvm
  hooks:
  - hookName: createContainer
    path: /usr/bin/nvidia-ctk
    args: [nvidia-ctk, hook, update-ldcache]
  mounts:
  - hostPath: /usr/lib/x86_64-linux-gnu/libcuda.so.580.126.16
    containerPath: /usr/lib/x86_64-linux-gnu/libcuda.so.580.126.16
"#,
        )
        .unwrap()
    }

    fn nodes(spec: &Value) -> Vec<String> {
        spec["devices"][0]["containerEdits"]["deviceNodes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| n["path"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_class_holds_only_the_served_cards_and_the_common_edits() {
        let spec = class_spec(Some(&nvidia_spec()), &BTreeSet::from([0]));
        assert_eq!(spec["kind"], KIND);
        assert_eq!(spec["devices"][0]["name"], "all");
        assert_eq!(
            nodes(&spec),
            [
                "/dev/nvidia0",
                "/dev/dri/card1",
                "/dev/nvidiactl",
                "/dev/nvidia-uvm"
            ]
        );
        let edits = &spec["devices"][0]["containerEdits"];
        assert_eq!(edits["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(edits["mounts"].as_array().unwrap().len(), 1);
        assert!(
            edits["env"]
                .as_array()
                .unwrap()
                .contains(&json!("FLIP_ACCELERATORS=1"))
        );
    }

    #[test]
    fn a_card_the_proxy_cannot_serve_stays_out() {
        let spec = class_spec(Some(&nvidia_spec()), &BTreeSet::from([1]));
        assert!(!nodes(&spec).contains(&"/dev/nvidia0".to_string()));
        assert!(nodes(&spec).contains(&"/dev/nvidia1".to_string()));
    }

    #[test]
    fn with_no_served_card_the_class_is_valid_and_gives_no_device() {
        for nvidia in [Some(nvidia_spec()), None] {
            let spec = class_spec(nvidia.as_ref(), &BTreeSet::new());
            assert!(nodes(&spec).is_empty());
            assert_eq!(
                spec["devices"][0]["containerEdits"]["env"],
                json!(["FLIP_ACCELERATORS=0"])
            );
            assert!(spec["devices"][0]["containerEdits"].get("hooks").is_none());
        }
    }

    #[test]
    fn the_file_is_written_from_nvidias_spec_in_the_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(NVIDIA_FILE),
            serde_yaml_ng::to_string(&nvidia_spec()).unwrap(),
        )
        .unwrap();
        write(dir.path(), &BTreeSet::from([0, 1])).unwrap();
        let back: Value =
            serde_yaml_ng::from_str(&std::fs::read_to_string(dir.path().join(FILE)).unwrap())
                .unwrap();
        assert_eq!(back["kind"], KIND);
        assert_eq!(back["cdiVersion"], "0.5.0");
        assert!(nodes(&back).contains(&"/dev/nvidia1".to_string()));
        // Only the class file and NVIDIA's are left: the temporary file was renamed.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
