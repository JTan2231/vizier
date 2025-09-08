use ignore::WalkBuilder;

pub struct FileTree {
    path: std::path::PathBuf,
    children: Vec<FileTree>,
}

pub fn build_tree() -> std::io::Result<FileTree> {
    let walker = WalkBuilder::new(std::env::current_dir().unwrap())
        .add_custom_ignore_filename("vizier.db")
        .build();

    let mut root = FileTree {
        path: std::env::current_dir().unwrap(),
        children: Vec::new(),
    };

    for entry in walker.filter_map(Result::ok) {
        if entry.path() == root.path {
            continue;
        }

        let relative = entry.path().strip_prefix(&root.path).unwrap();
        insert_path(&mut root, relative);
    }

    Ok(root)
}

fn insert_path(tree: &mut FileTree, path: &std::path::Path) {
    if let Some(first) = path.components().next() {
        let first_path = tree.path.join(first);

        if let Some(child) = tree.children.iter_mut().find(|c| c.path == first_path) {
            if path.components().count() > 1 {
                insert_path(child, &path.strip_prefix(first).unwrap());
            }
        } else {
            let mut new_tree = FileTree {
                path: first_path,
                children: Vec::new(),
            };

            if path.components().count() > 1 {
                insert_path(&mut new_tree, &path.strip_prefix(first).unwrap());
            }

            tree.children.push(new_tree);
        }
    }
}

pub fn tree_to_string(tree: &FileTree, prefix: &str) -> String {
    let mut result = String::new();
    let name = tree
        .path
        .file_name()
        .unwrap_or_else(|| tree.path.as_os_str())
        .to_string_lossy();

    result.push_str(&format!("{}- {}\n", prefix, name));

    for child in &tree.children {
        result.push_str(&tree_to_string(child, &format!("{}  ", prefix)));
    }

    result
}
