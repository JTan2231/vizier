use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;

const DISALLOWED: &[&str] = &[
    "git2",
    "tokio",
    "reqwest",
    "ignore",
    "grep-searcher",
    "lazy_static",
    "once_cell",
    "parking_lot",
];

#[test]
fn kernel_has_no_disallowed_dependencies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .expect("kernel crate should live under the workspace root");

    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(workspace_root)
        .output()
        .expect("run cargo metadata");

    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata JSON");
    let packages = metadata
        .get("packages")
        .and_then(|value| value.as_array())
        .expect("metadata packages array");

    let mut id_to_name = HashMap::new();
    let mut kernel_id = None;

    for pkg in packages {
        let id = pkg
            .get("id")
            .and_then(|value| value.as_str())
            .expect("package id");
        let name = pkg
            .get("name")
            .and_then(|value| value.as_str())
            .expect("package name");
        id_to_name.insert(id.to_string(), name.to_string());
        if name == "vizier-kernel" {
            kernel_id = Some(id.to_string());
        }
    }

    let kernel_id = kernel_id.expect("vizier-kernel package id");
    let nodes = metadata
        .get("resolve")
        .and_then(|value| value.get("nodes"))
        .and_then(|value| value.as_array())
        .expect("metadata resolve nodes");

    let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
    for node in nodes {
        let id = node
            .get("id")
            .and_then(|value| value.as_str())
            .expect("resolve node id")
            .to_string();
        let mut deps = Vec::new();

        if let Some(dep_list) = node.get("deps").and_then(|value| value.as_array()) {
            for dep in dep_list {
                if let Some(pkg_id) = dep.get("pkg").and_then(|value| value.as_str()) {
                    deps.push(pkg_id.to_string());
                }
            }
        } else if let Some(dep_list) = node.get("dependencies").and_then(|value| value.as_array()) {
            for dep in dep_list {
                if let Some(pkg_id) = dep.as_str() {
                    deps.push(pkg_id.to_string());
                }
            }
        }

        deps_map.insert(id, deps);
    }

    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(kernel_id.clone());

    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(deps) = deps_map.get(&id) {
            for dep in deps {
                queue.push_back(dep.clone());
            }
        }
    }

    let mut banned = Vec::new();
    for id in &seen {
        if id == &kernel_id {
            continue;
        }
        if let Some(name) = id_to_name.get(id)
            && DISALLOWED.iter().any(|blocked| blocked == name)
        {
            banned.push(name.clone());
        }
    }

    banned.sort();
    banned.dedup();
    assert!(
        banned.is_empty(),
        "disallowed dependencies found in vizier-kernel: {:?}",
        banned
    );
}
