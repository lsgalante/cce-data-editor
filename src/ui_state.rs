//! Where the editor was, so it comes back there: the open file, the selected
//! key and the tree's scroll. The file is saved whenever it changes (opened,
//! saved as), the key and scroll when the window exits (`on_exit`, which
//! `ccectl restart-app` runs as any close does); a launch reopens it — after a
//! restart, at login, or when reopened.
//!
//! **A restart replays the same command line**, so a window opened as
//! `cce-data-editor input.kdl --select binds` would always come back on that
//! file and key, whatever was opened since. The file therefore also records
//! what the command line asked for (`launched`: its file and `--select`): the
//! same command line again, or one asking for nothing, is this window
//! reopening, and the saved place wins; a command line asking for something
//! else is a new request, and it wins. A saved file that has gone is not
//! reopened, and the key and scroll only go back into the file they came
//! from. Restoring only reads: nothing here creates or writes a document.
//!
//! `cce_state_dir()/data-editor/state.json`, written whole to a temporary file
//! and renamed over, so a crash mid-write leaves the last one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// What a command line asked for.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launched {
    /// The file it named, absolute.
    pub file: Option<PathBuf>,
    /// Its `--select <flat.path>`.
    pub select: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Saved {
    /// The open file, absolute.
    file: Option<PathBuf>,
    /// What the command line asked for when this was saved.
    #[serde(default)]
    launched: Launched,
    /// The selected key's flat path.
    #[serde(default)]
    key: Option<String>,
    /// The tree's scroll.
    #[serde(default)]
    scroll_y: f32,
}

/// What a launch opens.
#[derive(Debug, Default, PartialEq)]
pub struct Place {
    pub file: Option<PathBuf>,
    /// A saved key to select again, quietly (it may have been deleted since).
    pub key: Option<String>,
    pub scroll_y: f32,
    /// The command line's `--select`, reported if it names nothing.
    pub select: Option<String>,
}

/// What this run's command line asked for.
static LAUNCHED: OnceLock<Launched> = OnceLock::new();

fn path() -> PathBuf {
    cce_ui::config::cce_state_dir().join("data-editor").join("state.json")
}

pub fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// What to open, given what the command line asked for: the saved place when
/// this command line is the one it was saved under (or asks for nothing),
/// else the command line's file and key.
pub fn initial(launched: Launched) -> Place {
    let _ = LAUNCHED.set(launched.clone());
    let saved = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
    choose(&launched, saved.as_ref(), Path::exists)
}

fn choose(launched: &Launched, saved: Option<&Saved>, exists: impl Fn(&Path) -> bool) -> Place {
    let asks_nothing = *launched == Launched::default();
    let reopening = saved.filter(|s| asks_nothing || s.launched == *launched);
    if let Some(s) = reopening.filter(|s| s.file.as_deref().is_none_or(&exists)) {
        return Place { file: s.file.clone(), key: s.key.clone(), scroll_y: s.scroll_y, select: None };
    }
    // As launched; the saved key only if it was saved in this very file and
    // the command line names no key of its own.
    let file = launched.file.clone();
    let select = launched.select.clone();
    match saved.filter(|s| file.is_some() && s.file == file && select.is_none()) {
        Some(s) => Place { file, key: s.key.clone(), scroll_y: s.scroll_y, select },
        None => Place { file, select, ..Place::default() },
    }
}

/// The open file changed, or the window is closing with the key and scroll
/// where they are.
pub fn save(file: Option<&Path>, key: Option<&str>, scroll_y: f32) {
    let saved = Saved {
        file: file.map(absolute),
        launched: LAUNCHED.get().cloned().unwrap_or_default(),
        key: key.map(String::from),
        scroll_y,
    };
    let path = path();
    let Some(dir) = path.parent() else { return };
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, serde_json::to_vec(&saved).unwrap_or_default()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        eprintln!("[cce-data-editor] cannot save the open file to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(file: Option<&str>, select: Option<&str>) -> Launched {
        Launched { file: file.map(PathBuf::from), select: select.map(String::from) }
    }

    fn saved(file: Option<&str>, launched: Launched) -> Saved {
        Saved { file: file.map(PathBuf::from), launched, key: Some("style.gap".into()), scroll_y: 120.0 }
    }

    fn restored(file: &str) -> Place {
        Place { file: Some(file.into()), key: Some("style.gap".into()), scroll_y: 120.0, select: None }
    }

    const ALL: fn(&Path) -> bool = |_| true;

    #[test]
    fn the_same_command_line_reopens_where_it_was() {
        // Launched on a.kdl with a key, opened b.kdl, restarted with the same argv.
        let argv = launch(Some("/a.kdl"), Some("binds"));
        let s = saved(Some("/b.kdl"), argv.clone());
        assert_eq!(choose(&argv, Some(&s), ALL), restored("/b.kdl"));
        // A plain launch reopens whatever was open, however that was launched.
        assert_eq!(choose(&Launched::default(), Some(&s), ALL), restored("/b.kdl"));
    }

    #[test]
    fn a_command_line_asking_for_something_else_wins() {
        let s = saved(Some("/b.kdl"), launch(Some("/a.kdl"), None));
        let argv = launch(Some("/c.kdl"), None);
        assert_eq!(choose(&argv, Some(&s), ALL), Place { file: Some("/c.kdl".into()), ..Place::default() });
        // Naming the open file keeps its key, unless it names a key too.
        let argv = launch(Some("/b.kdl"), None);
        assert_eq!(choose(&argv, Some(&s), ALL), restored("/b.kdl"));
        let argv = launch(Some("/b.kdl"), Some("binds"));
        let want = Place { file: Some("/b.kdl".into()), select: Some("binds".into()), ..Place::default() };
        assert_eq!(choose(&argv, Some(&s), ALL), want);
    }

    #[test]
    fn nothing_saved_opens_as_launched() {
        let argv = launch(Some("/c.kdl"), Some("binds"));
        let want = Place { file: Some("/c.kdl".into()), select: Some("binds".into()), ..Place::default() };
        assert_eq!(choose(&argv, None, ALL), want);
        assert_eq!(choose(&Launched::default(), None, ALL), Place::default());
    }

    #[test]
    fn a_saved_file_that_has_gone_is_not_reopened() {
        let gone = |p: &Path| p != Path::new("/b.kdl");
        let argv = launch(Some("/a.kdl"), None);
        let s = saved(Some("/b.kdl"), argv.clone());
        assert_eq!(choose(&argv, Some(&s), gone), Place { file: Some("/a.kdl".into()), ..Place::default() });
        assert_eq!(choose(&Launched::default(), Some(&s), gone), Place::default());
    }
}
