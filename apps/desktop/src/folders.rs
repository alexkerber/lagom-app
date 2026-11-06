//! Custom folders: the tree in the sidebar and "Move to" in ⌘K.
//! Source: "Lagom · Folders" in Figma (GrDzJzlEBfhe2n9gAhJTor, node 6:2). Server folders from
//! the store, or the sample folders in demo mode.

/// Folders with a place of their own in the sidebar nav stay out of the folder group and ⌘K.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Custom,
    Trash,
    Sent,
    /// The archive: "Done" in the nav.
    Done,
    /// Snoozed mail. Local, no server folder.
    Later,
}

pub struct Folder {
    pub name: String,
    pub parent: Option<usize>,
    /// Message ids, newest first. For server folders, only the open one is loaded.
    pub items: Vec<usize>,
    /// Subfolders show. Kept in memory only.
    pub open: bool,
    pub kind: Kind,
    pub unread: usize,
    /// Store folder id. `None` for demo folders, Later and a folder still being created.
    pub store: Option<i64>,
    /// The IMAP name with its path, "Travel/Trains".
    pub full: String,
    /// The server no longer lists it. Kept, so folder indices in undo history stay valid.
    pub gone: bool,
}

impl Folder {
    pub fn new(name: &str, parent: Option<usize>, items: &[usize]) -> Self {
        Self { name: name.into(), parent, items: items.to_vec(), open: true, kind: Kind::Custom, unread: 0, store: None, full: name.into(), gone: false }
    }

    /// Listed in the folder group and as a "Move to" target.
    pub fn listed(&self) -> bool {
        !self.gone && matches!(self.kind, Kind::Custom | Kind::Trash)
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

/// Updates `folders` from the store in place. Indices never move (undo history and the open
/// list hold them): new folders go last, deleted ones are marked gone.
pub fn merge(folders: &mut Vec<Folder>, server: &[lagom_core::store::Folder]) {
    use lagom_core::store::FolderRole as Role;
    for f in folders.iter_mut().filter(|f| f.store.is_some()) {
        f.gone = true;
    }
    for s in server {
        let kind = match s.role {
            Role::Inbox => continue,
            Role::Sent => Kind::Sent,
            Role::Archive => Kind::Done,
            Role::Trash => Kind::Trash,
            _ => Kind::Custom,
        };
        let name = s.delimiter.as_deref().and_then(|d| s.name.rsplit_once(d)).map_or(s.name.as_str(), |(_, n)| n);
        // A folder ⌘K created locally is adopted once the server lists it.
        let found = folders.iter().position(|f| f.store == Some(s.id)).or_else(|| folders.iter().position(|f| f.store.is_none() && f.kind == Kind::Custom && f.full == s.name));
        let i = found.unwrap_or_else(|| {
            folders.push(Folder::new(name, None, &[]));
            folders.len() - 1
        });
        let f = &mut folders[i];
        (f.name, f.full, f.kind, f.unread, f.store, f.gone) = (name.into(), s.name.clone(), kind, s.unread as usize, Some(s.id), false);
    }
    for s in server {
        let Some(i) = folders.iter().position(|f| f.store == Some(s.id)) else { continue };
        let parent = s.delimiter.as_deref().and_then(|d| s.name.rsplit_once(d)).and_then(|(p, _)| folders.iter().position(|f| f.store.is_some() && !f.gone && f.full == p));
        folders[i].parent = parent;
    }
}

/// (folder, depth) in tree order. Children of a closed folder are left out unless `all`.
pub fn tree(folders: &[Folder], all: bool) -> Vec<(usize, usize)> {
    fn walk(folders: &[Folder], parent: Option<usize>, depth: usize, all: bool, out: &mut Vec<(usize, usize)>) {
        for (f, folder) in folders.iter().enumerate().filter(|(_, x)| x.parent == parent && x.listed()) {
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
    folders.iter().any(|x| x.parent == Some(f) && x.listed())
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
    fn merge_keeps_indices_and_nests_by_delimiter() {
        use lagom_core::store::{Folder as F, FolderRole as R};
        let f = |id, name: &str, role, unread| F { id, account: "a".into(), name: name.into(), delimiter: Some("/".into()), role, uidvalidity: None, uidnext: None, last_uid: 0, highest_modseq: None, unread };
        let mut folders = vec![Folder { kind: Kind::Later, ..Folder::new("Later", None, &[]) }, Folder::new("Taxes", None, &[])];
        merge(&mut folders, &[f(1, "INBOX", R::Inbox, 4), f(2, "Archive", R::Archive, 0), f(3, "Travel", R::Other, 0), f(4, "Travel/Trains", R::Other, 2), f(5, "Taxes", R::Other, 0)]);
        let names: Vec<_> = folders.iter().map(|x| (x.name.as_str(), x.kind, x.parent)).collect();
        // INBOX stays out; the local "Taxes" is adopted, not doubled; Trains sits under Travel.
        assert_eq!(names, [("Later", Kind::Later, None), ("Taxes", Kind::Custom, None), ("Archive", Kind::Done, None), ("Travel", Kind::Custom, None), ("Trains", Kind::Custom, Some(3))]);
        assert_eq!(folders[4].unread, 2);
        // Travel goes on the server: marked gone, indices unchanged, Trains is top level.
        merge(&mut folders, &[f(2, "Archive", R::Archive, 0), f(4, "Travel/Trains", R::Other, 0), f(5, "Taxes", R::Other, 0)]);
        assert!(folders[3].gone && !folders[3].listed());
        assert_eq!((folders.len(), folders[4].parent), (5, None));
        assert_eq!(tree(&folders, true).iter().map(|r| r.0).collect::<Vec<_>>(), [1, 4]);
    }

    #[test]
    fn more_than_six_rows_show_five() {
        assert_eq!(shown(6, false), 6);
        assert_eq!(shown(7, false), 5);
        assert_eq!(shown(7, true), 7);
        assert_eq!(shown(0, false), 0);
    }
}
