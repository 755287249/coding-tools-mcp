//! The authenticated browser administrator selects folders on the host, not the
//! visitor's device. This route is never part of the MCP tool catalog.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn browse(target: Option<&str>, boundary: Option<&Path>) -> Result<Value, String> {
    let home = dirs::home_dir().ok_or("Home directory unavailable")?;
    let root = boundary
        .map(std::fs::canonicalize)
        .transpose()
        .map_err(|e| e.to_string())?;
    let requested = target
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.clone().unwrap_or(home.clone()));
    if !requested.is_absolute() {
        return Err("Use an absolute host directory path".into());
    }
    let path = std::fs::canonicalize(&requested).map_err(|e| e.to_string())?;
    if !path.is_dir() || root.as_ref().is_some_and(|root| !path.starts_with(root)) {
        return Err("Directory is outside the selected workspace or unavailable".into());
    }
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
        let Ok(entry) = entry else { continue };
        let Ok(child) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if child.is_dir() && root.as_ref().is_none_or(|root| child.starts_with(root)) {
            rows.push((entry.file_name().to_string_lossy().into_owned(), child));
        }
    }
    rows.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    let total = rows.len();
    let parent = path
        .parent()
        .filter(|parent| root.as_ref().is_none_or(|root| parent.starts_with(root)));
    let mut roots = vec![root.clone().unwrap_or(home)];
    if root.is_none() {
        #[cfg(windows)]
        for drive in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", drive as char));
            if p.is_dir() {
                roots.push(p);
            }
        }
        #[cfg(not(windows))]
        roots.push(PathBuf::from("/"));
    }
    roots.sort();
    roots.dedup();
    Ok(json!({"ok":true,"path":path,"parent":parent,"roots":roots,
        "directories":rows.into_iter().take(2000).map(|(name,path)|json!({"name":name,"path":path})).collect::<Vec<_>>(),
        "totalDirectories":total,"truncated":total>2000}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returns_directories_and_confines_workspace_selection() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("file"), "data").unwrap();
        let result = browse(None, Some(root.path())).unwrap();
        assert_eq!(result["directories"].as_array().unwrap().len(), 1);
        assert!(result["parent"].is_null());
        assert!(browse(Some("relative"), Some(root.path())).is_err());
        assert!(browse(root.path().parent().unwrap().to_str(), Some(root.path())).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn scoped_browsing_never_follows_links_outside_the_project() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(browse(root.path().join("escape").to_str(), Some(root.path())).is_err());
        assert_eq!(
            browse(None, Some(root.path())).unwrap()["totalDirectories"],
            0
        );
    }
}
