//! Custom folders: the tree in the sidebar and "Move to" in ⌘K.
//! Source: "Lagom · Folders" in Figma (GrDzJzlEBfhe2n9gAhJTor, node 6:2). Sample data, in memory.

pub struct Folder {
    pub name: String,
    pub parent: Option<usize>,
    /// Message ids, newest first.
    pub items: Vec<usize>,
    /// Subfolders show. Kept in memory only.
    pub open: bool,
}

impl Folder {
    pub fn new(name: &str, parent: Option<usize>, items: &[usize]) -> Self {
        Self { name: name.into(), parent, items: items.to_vec(), open: true }
    }
}

/// Sample folders. Message ids point into `main_window::MESSAGES`.
pub fn sample() -> Vec<Folder> {
    let mut folders = vec![
        Folder::new("Receipts", None, &[7, 8, 9]),
        Folder::new("Travel", None, &[10]),
        Folder::new("Trains", Some(1), &[12]),
        Folder::new("Japan 2026", Some(1), &[11]),
        Folder::new("Projects", None, &[]),
        Folder::new("Lagom", Some(4), &[13]),
        Folder::new("Garden", Some(4), &[14]),
        Folder::new("Housing co-op", None, &[15]),
        Folder::new("Newsletters", None, &[16, 17]),
    ];
    folders[4].open = false;
    folders
}

/// (folder, depth) in tree order. Children of a closed folder are left out unless `all`.
pub fn tree(folders: &[Folder], all: bool) -> Vec<(usize, usize)> {
    fn walk(folders: &[Folder], parent: Option<usize>, depth: usize, all: bool, out: &mut Vec<(usize, usize)>) {
        for (f, folder) in folders.iter().enumerate().filter(|(_, x)| x.parent == parent) {
            out.push((f, depth));
            if all || folder.open {
                walk(folders, Some(f), depth + 1, all, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(folders, None, 0, all, &mut out);
    out
}

pub fn has_children(folders: &[Folder], f: usize) -> bool {
    folders.iter().any(|x| x.parent == Some(f))
}

/// "Travel / Trains".
pub fn path(folders: &[Folder], f: usize) -> String {
    match folders[f].parent {
        Some(p) => format!("{} / {}", path(folders, p), folders[f].name),
        None => folders[f].name.clone(),
    }
}

/// More than this many rows collapse to `SHOWN` rows and an "N more" row.
const MAX_ROWS: usize = 6;
const SHOWN: usize = 5;

/// How many of `rows` visible rows the sidebar shows.
pub fn shown(rows: usize, expanded: bool) -> usize {
    if expanded || rows <= MAX_ROWS { rows } else { SHOWN }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_folders_hide_their_children() {
        let mut folders = sample();
        let names = |rows: Vec<(usize, usize)>| rows.iter().map(|&(f, d)| (path(&folders, f), d)).collect::<Vec<_>>();
        let visible = names(tree(&folders, false));
        assert_eq!(visible[2], ("Travel / Trains".to_string(), 1));
        assert!(!visible.iter().any(|(p, _)| p.starts_with("Projects /")));
        assert_eq!(visible.len(), 7);
        // The palette lists every folder, closed or not.
        assert_eq!(tree(&folders, true).len(), folders.len());
        folders[1].open = false;
        assert_eq!(tree(&folders, false).len(), 5);
        assert!(has_children(&folders, 1) && !has_children(&folders, 0));
    }

    #[test]
    fn more_than_six_rows_show_five() {
        assert_eq!(shown(6, false), 6);
        assert_eq!(shown(7, false), 5);
        assert_eq!(shown(7, true), 7);
        assert_eq!(shown(0, false), 0);
    }
}
