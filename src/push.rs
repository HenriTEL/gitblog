use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    time::Duration,
};

use opendal::{EntryMode, Operator, blocking, layers::RetryLayer};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PushConfig {
    scheme: String,
    #[serde(default)]
    options: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFile {
    path: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushSummary {
    pub uploaded_files: usize,
    pub deleted_files: usize,
}

pub fn push_directory(
    root: &Path,
    config_path: &Path,
    delete_extras: bool,
) -> Result<PushSummary, Box<dyn std::error::Error>> {
    let config = read_push_config(config_path)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let _guard = runtime.enter();
    let target = build_push_target(&config)?;
    let op = &target.op;

    let local_files = collect_local_files(root)?;
    println!("push: collected {} local files to sync", local_files.len());
    for file in &local_files {
        let remote_path = target.remote_path(&file.path);
        println!("push: uploading {}", remote_path);
        op.write(remote_path.as_str(), file.bytes.clone())?;
        println!("push: uploaded {}", remote_path);
    }

    let deleted_files = if delete_extras {
        let remote_files = list_remote_files(op, target.list_base())?;
        let local_paths: HashSet<String> = local_files
            .iter()
            .map(|f| target.remote_path(&f.path))
            .collect();
        let deletions = compute_deletions(&local_paths, &remote_files, delete_extras);
        for path in &deletions {
            println!("push: deleting {}", path);
            op.delete(path)?;
        }
        deletions.len()
    } else {
        0
    };

    Ok(PushSummary {
        uploaded_files: local_files.len(),
        deleted_files,
    })
}

fn read_push_config(path: &Path) -> Result<PushConfig, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: PushConfig = toml::from_str(&content)?;
    Ok(config)
}

struct PushTarget {
    op: blocking::Operator,
    key_prefix: String,
}

impl PushTarget {
    fn remote_path(&self, relative_path: &str) -> String {
        format!("{}{}", self.key_prefix, relative_path)
    }

    fn list_base(&self) -> &str {
        if self.key_prefix.is_empty() {
            "/"
        } else {
            self.key_prefix.as_str()
        }
    }
}

fn build_push_target(config: &PushConfig) -> Result<PushTarget, Box<dyn std::error::Error>> {
    let mut options = config.options.clone();
    let mut key_prefix = String::new();

    if config.scheme.eq_ignore_ascii_case("ftp") {
        let ftp_root = options.remove("root").unwrap_or_else(|| "/".to_string());
        key_prefix = normalize_ftp_root_prefix(&ftp_root);
        println!(
            "push: using FTP path prefix '{}' (OpenDAL root disabled to avoid FTP cwd/mkdir loop)",
            if key_prefix.is_empty() {
                "/"
            } else {
                key_prefix.as_str()
            }
        );
    }

    let op = Operator::via_iter(config.scheme.as_str(), options)?.layer(
        RetryLayer::new()
            .with_max_times(3)
            .with_notify(|err: &opendal::Error, delay: Duration| {
                eprintln!(
                    "push: retrying OpenDAL operation in {:?} after error: {}",
                    delay, err
                );
            }),
    );
    Ok(PushTarget {
        op: blocking::Operator::new(op)?,
        key_prefix,
    })
}

fn collect_local_files(root: &Path) -> Result<Vec<LocalFile>, Box<dyn std::error::Error>> {
    fn walk(
        root: &Path,
        current: &Path,
        files: &mut Vec<LocalFile>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, files)?;
            } else if path.is_file() {
                let rel = path.strip_prefix(root)?;
                files.push(LocalFile {
                    path: to_remote_path(rel),
                    bytes: fs::read(path)?,
                });
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn list_remote_files(
    op: &blocking::Operator,
    start_dir: &str,
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let mut files = HashSet::new();
    let mut pending_dirs = vec![start_dir.to_string()];
    while let Some(dir) = pending_dirs.pop() {
        for item in op.lister(dir.as_str())? {
            let entry: opendal::Entry = item?;
            match entry.metadata().mode() {
                EntryMode::FILE => {
                    files.insert(normalize_remote_entry_path(entry.path()));
                }
                EntryMode::DIR => pending_dirs.push(entry.path().to_string()),
                _ => {}
            }
        }
    }
    Ok(files)
}

fn compute_deletions(
    local_paths: &HashSet<String>,
    remote_paths: &HashSet<String>,
    delete_extras: bool,
) -> Vec<String> {
    if !delete_extras {
        return Vec::new();
    }

    let mut deletions = remote_paths
        .iter()
        .filter(|path| !local_paths.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    deletions.sort();
    deletions
}

fn to_remote_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_remote_entry_path(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

fn normalize_ftp_root_prefix(root: &str) -> String {
    let trimmed = root.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collect_local_files, compute_deletions, normalize_ftp_root_prefix, read_push_config,
    };
    use std::{collections::HashSet, fs};
    use tempfile::tempdir;

    #[test]
    fn parse_push_config_success() {
        let temp = tempdir().expect("create temp dir");
        let config_path = temp.path().join("push.toml");
        fs::write(
            &config_path,
            r#"
scheme = "s3"

[options]
bucket = "my-bucket"
region = "eu-west-1"
"#,
        )
        .expect("write config");

        let config = read_push_config(&config_path).expect("parse config");
        assert_eq!(config.scheme, "s3");
        assert_eq!(config.options.get("bucket"), Some(&"my-bucket".to_string()));
    }

    #[test]
    fn parse_push_config_failure_when_missing_scheme() {
        let temp = tempdir().expect("create temp dir");
        let config_path = temp.path().join("push.toml");
        fs::write(
            &config_path,
            r#"
[options]
bucket = "my-bucket"
"#,
        )
        .expect("write config");

        assert!(read_push_config(&config_path).is_err());
    }

    #[test]
    fn collect_local_files_uses_relative_posix_paths() {
        let temp = tempdir().expect("create temp dir");
        fs::create_dir_all(temp.path().join("posts")).expect("create posts dir");
        fs::write(temp.path().join("index.html"), "<html></html>").expect("write index");
        fs::write(temp.path().join("posts").join("a.md"), "# title").expect("write post");

        let files = collect_local_files(temp.path()).expect("collect files");
        let paths = files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec!["index.html".to_string(), "posts/a.md".to_string()]
        );
    }

    #[test]
    fn compute_deletions_respects_delete_flag() {
        let local = HashSet::from(["index.html".to_string()]);
        let remote = HashSet::from(["index.html".to_string(), "old.html".to_string()]);

        let deletions_without_delete = compute_deletions(&local, &remote, false);
        assert!(deletions_without_delete.is_empty());

        let deletions_with_delete = compute_deletions(&local, &remote, true);
        assert_eq!(deletions_with_delete, vec!["old.html".to_string()]);
    }

    #[test]
    fn collect_local_files_empty_directory() {
        let temp = tempdir().expect("create temp dir");
        let files = collect_local_files(temp.path()).expect("collect files");
        assert!(files.is_empty());
    }

    #[test]
    fn normalize_ftp_root_prefix_handles_common_inputs() {
        assert_eq!(normalize_ftp_root_prefix("/"), "");
        assert_eq!(normalize_ftp_root_prefix(""), "");
        assert_eq!(normalize_ftp_root_prefix("blog"), "blog/");
        assert_eq!(normalize_ftp_root_prefix("/blog/"), "blog/");
        assert_eq!(normalize_ftp_root_prefix(" /blog/assets/ "), "blog/assets/");
    }
}
