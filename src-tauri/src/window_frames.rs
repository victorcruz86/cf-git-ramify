// Where each project window sits, after IntelliJ's per-project frame bounds
// (`WindowStateService`): the frame of a repository's window is saved as the window moves and
// resizes, and put back when a window opens that repository, so a start reopens every project
// where it was. Frames are in logical points. The file lives in Ramify's data folder, never in a
// repository.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::storage;

pub const FILE: &str = "window-frames.json";
const CORRUPT: &str = "window-frames.damaged.json";
// Past this many projects the one saved longest ago is forgotten.
pub const MAX: usize = 100;
// Held around a save's read and write, so that saves from several threads of this process do
// not overwrite each other.
pub static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
// How long a window has to stay still before its frame is written.
pub const SETTLE: Duration = Duration::from_millis(500);

// The part of the title bar that must be on a screen for a frame to be usable: a strip as wide as
// the frame and this tall at its top, of which at least `MIN_REACHABLE` points (width, height)
// must show.
const STRIP_HEIGHT: i32 = 30;
const MIN_REACHABLE: (i32, i32) = (50, 20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub fullscreen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    path: String,
    frame: Frame,
}

// The latest saved first.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    frames: Vec<Saved>,
}

fn load(dir: &Path) -> io::Result<Stored> {
    storage::load_json(dir, FILE, CORRUPT)
}

// The frame saved for `root`. A missing file has none; a damaged one is set aside.
pub fn get(dir: &Path, root: &str) -> io::Result<Option<Frame>> {
    Ok(load(dir)?
        .frames
        .into_iter()
        .find(|saved| saved.path == root)
        .map(|saved| saved.frame))
}

// The frame of `root` goes first. The file is read again right before writing, so what another
// Ramify process saved stays.
pub fn record(dir: &Path, root: &str, frame: Frame) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut stored = load(dir)?;
    stored.frames.retain(|saved| saved.path != root);
    stored.frames.insert(
        0,
        Saved {
            path: root.to_string(),
            frame,
        },
    );
    stored.frames.truncate(MAX);
    let bytes = serde_json::to_vec_pretty(&stored)?;
    storage::write_atomic(&dir.join(FILE), &bytes)
}

// A screen's usable area (without the menu bar and the Dock): x, y, width, height.
pub type Monitor = (i32, i32, u32, u32);

// Whether `frame` can be used with these screens: enough of the strip at its top lies inside one
// of them to grab the window and move it. Otherwise (a screen that is gone) it is ignored.
pub fn fits(frame: &Frame, monitors: &[Monitor]) -> bool {
    let strip_width = frame.width.min(i32::MAX as u32) as i32;
    let need_width = strip_width.min(MIN_REACHABLE.0);
    monitors.iter().any(|&(x, y, width, height)| {
        let left = frame.x.max(x);
        let top = frame.y.max(y);
        let right = frame
            .x
            .saturating_add(strip_width)
            .min(x.saturating_add(width.min(i32::MAX as u32) as i32));
        let bottom = frame
            .y
            .saturating_add(STRIP_HEIGHT)
            .min(y.saturating_add(height.min(i32::MAX as u32) as i32));
        right.saturating_sub(left) >= need_width && bottom.saturating_sub(top) >= MIN_REACHABLE.1
    })
}

// What to save for a window: a maximized or fullscreen window keeps the last normal bounds, so
// that leaving the mode comes back to them, and only the flags change. Without earlier bounds the
// current ones are the best there is.
pub fn to_save(
    previous: Option<Frame>,
    current: Frame,
    maximized: bool,
    fullscreen: bool,
) -> Frame {
    let base = if maximized || fullscreen {
        previous.unwrap_or(current)
    } else {
        current
    };
    // macOS can report a fullscreen window as maximized too; fullscreen wins, so that leaving it
    // comes back to the normal bounds rather than to a zoomed window.
    Frame {
        maximized: maximized && !fullscreen,
        fullscreen,
        ..base
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Wait {
    Remaining(Duration),
    Due,
    Cancelled,
}

// The saves waiting for their window to stay still, one per project window: a burst of moves
// writes once, `SETTLE` after the last of them.
#[derive(Debug, Default)]
pub struct Pending {
    last: HashMap<String, Instant>,
}

impl Pending {
    // A move or resize of `owner` at `now`. `true` when no save was waiting, so the caller starts
    // the one that will wait for it.
    pub fn touch(&mut self, owner: &str, now: Instant) -> bool {
        self.last.insert(owner.to_string(), now).is_none()
    }

    // What the save of `owner` does at `now`: waits, is due (and nothing waits any more), or was
    // cancelled.
    pub fn check(&mut self, owner: &str, now: Instant) -> Wait {
        let Some(&last) = self.last.get(owner) else {
            return Wait::Cancelled;
        };
        let due = last + SETTLE;
        if now >= due {
            self.last.remove(owner);
            Wait::Due
        } else {
            Wait::Remaining(due - now)
        }
    }

    // The window closed or its frame was saved: nothing is waiting for it.
    pub fn cancel(&mut self, owner: &str) {
        self.last.remove(owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn frame(x: i32, y: i32) -> Frame {
        Frame {
            x,
            y,
            width: 1200,
            height: 800,
            maximized: false,
            fullscreen: false,
        }
    }

    // A 1440x900 main screen whose work area starts under the menu bar.
    const MAIN: Monitor = (0, 25, 1440, 875);

    #[test]
    fn a_saved_frame_comes_back() {
        let dir = TempDir::new().unwrap();
        let saved = Frame {
            maximized: true,
            ..frame(40, 60)
        };
        assert_eq!(get(dir.path(), "/work/a").unwrap(), None);

        record(dir.path(), "/work/a", saved).unwrap();
        record(dir.path(), "/work/b", frame(1, 2)).unwrap();

        assert_eq!(get(dir.path(), "/work/a").unwrap(), Some(saved));
        assert_eq!(get(dir.path(), "/work/b").unwrap(), Some(frame(1, 2)));
        assert_eq!(get(dir.path(), "/work/c").unwrap(), None);
    }

    #[test]
    fn saving_again_replaces_the_frame() {
        let dir = TempDir::new().unwrap();
        record(dir.path(), "/work/a", frame(1, 1)).unwrap();
        record(dir.path(), "/work/a", frame(2, 2)).unwrap();

        assert_eq!(get(dir.path(), "/work/a").unwrap(), Some(frame(2, 2)));
        assert_eq!(load(dir.path()).unwrap().frames.len(), 1);
    }

    #[test]
    fn a_damaged_file_is_set_aside() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE), b"{ not json").unwrap();

        assert_eq!(get(dir.path(), "/work/a").unwrap(), None);

        let kept = fs::read(dir.path().join("window-frames.damaged.json")).unwrap();
        assert_eq!(kept, b"{ not json");
        assert!(!dir.path().join(FILE).exists());
        // And the next save starts a fresh file.
        record(dir.path(), "/work/a", frame(3, 3)).unwrap();
        assert_eq!(get(dir.path(), "/work/a").unwrap(), Some(frame(3, 3)));
    }

    #[test]
    fn the_least_recently_saved_frame_goes_past_the_cap() {
        let dir = TempDir::new().unwrap();
        for index in 0..MAX + 5 {
            record(
                dir.path(),
                &format!("/work/{index}"),
                frame(index as i32, 0),
            )
            .unwrap();
        }
        // Saving the oldest survivor again makes it the latest.
        record(dir.path(), "/work/5", frame(-1, 0)).unwrap();
        record(dir.path(), "/work/new", frame(0, 0)).unwrap();

        let stored = load(dir.path()).unwrap();
        assert_eq!(stored.frames.len(), MAX);
        assert_eq!(stored.frames[0].path, "/work/new");
        assert_eq!(get(dir.path(), "/work/0").unwrap(), None);
        assert_eq!(get(dir.path(), "/work/5").unwrap(), Some(frame(-1, 0)));
        assert_eq!(get(dir.path(), "/work/6").unwrap(), None);
        assert!(get(dir.path(), "/work/7").unwrap().is_some());
    }

    #[test]
    fn a_frame_inside_a_monitor_fits() {
        assert!(fits(&frame(100, 100), &[MAIN]));
        // Its edge on the work area's own.
        assert!(fits(&frame(0, 25), &[MAIN]));
    }

    #[test]
    fn a_frame_partly_off_screen_fits_while_its_title_bar_shows() {
        // Mostly to the right and below the screen, but 200 points of the title bar show.
        assert!(fits(&frame(1240, 800), &[MAIN]));
        // Dragged left, 80 points of its title bar remain.
        assert!(fits(&frame(-1120, 100), &[MAIN]));
    }

    #[test]
    fn a_frame_whose_title_bar_cannot_be_reached_does_not_fit() {
        // Only 20 points of the title bar show on the right.
        assert!(!fits(&frame(1420, 100), &[MAIN]));
        // The title bar is under the menu bar.
        assert!(!fits(&frame(100, -20), &[MAIN]));
        // The title bar is below the screen.
        assert!(!fits(&frame(100, 890), &[MAIN]));
        assert!(!fits(&frame(100, 100), &[]));
    }

    #[test]
    fn a_frame_on_a_disconnected_monitor_does_not_fit() {
        // It was on a screen to the right of the main one, which is gone.
        assert!(!fits(&frame(1800, 100), &[MAIN]));
        // With that screen back it fits.
        assert!(fits(&frame(1800, 100), &[MAIN, (1440, 0, 1920, 1080)]));
    }

    #[test]
    fn negative_coordinates_fit_on_a_monitor_left_of_the_main_one() {
        let left: Monitor = (-1920, 0, 1920, 1080);
        assert!(fits(&frame(-1500, 50), &[left, MAIN]));
        // Across the two screens.
        assert!(fits(&frame(-600, 50), &[left, MAIN]));
        // Without the left screen it is gone.
        assert!(!fits(&frame(-1500, 50), &[MAIN]));
    }

    #[test]
    fn a_narrow_frame_needs_its_whole_width_on_screen() {
        let narrow = Frame {
            width: 40,
            ..frame(1390, 100)
        };
        assert!(fits(&narrow, &[MAIN]));
        assert!(!fits(&Frame { x: 1410, ..narrow }, &[MAIN]));
    }

    #[test]
    fn a_maximized_window_keeps_its_last_normal_bounds() {
        let normal = frame(40, 60);
        let screen_sized = Frame {
            width: 1440,
            height: 875,
            ..frame(0, 25)
        };

        let saved = to_save(Some(normal), screen_sized, true, false);
        assert_eq!(
            saved,
            Frame {
                maximized: true,
                ..normal
            }
        );
        let saved = to_save(Some(normal), screen_sized, false, true);
        assert_eq!(
            saved,
            Frame {
                fullscreen: true,
                ..normal
            }
        );
        // Nothing saved before: the current bounds are kept.
        assert_eq!(
            to_save(None, screen_sized, true, false),
            Frame {
                maximized: true,
                ..screen_sized
            }
        );
        // Leaving the mode saves the new bounds and clears the flags.
        assert_eq!(
            to_save(Some(normal), frame(5, 5), false, false),
            frame(5, 5)
        );
        // Fullscreen reported as maximized too: fullscreen wins.
        assert_eq!(
            to_save(Some(normal), screen_sized, true, true),
            Frame {
                fullscreen: true,
                ..normal
            }
        );
    }

    #[test]
    fn a_burst_of_moves_is_saved_once_after_the_last() {
        let mut pending = Pending::default();
        let start = Instant::now();

        assert!(pending.touch("main", start));
        // The window keeps moving: nobody else starts a save.
        assert!(!pending.touch("main", start + Duration::from_millis(300)));
        assert!(!pending.touch("main", start + Duration::from_millis(600)));
        // 500 ms after the first move it is not due: the last was 200 ms ago.
        let at = start + Duration::from_millis(800);
        assert_eq!(
            pending.check("main", at),
            Wait::Remaining(Duration::from_millis(300))
        );
        // Due 500 ms after the last one, and then nothing waits.
        assert_eq!(
            pending.check("main", start + Duration::from_millis(1100)),
            Wait::Due
        );
        assert_eq!(
            pending.check("main", start + Duration::from_millis(1200)),
            Wait::Cancelled
        );
        assert!(pending.touch("main", start + Duration::from_secs(5)));
    }

    #[test]
    fn each_window_waits_on_its_own_and_a_close_cancels() {
        let mut pending = Pending::default();
        let start = Instant::now();

        assert!(pending.touch("main", start));
        assert!(pending.touch("main-2", start));
        pending.cancel("main");

        assert_eq!(pending.check("main", start + SETTLE), Wait::Cancelled);
        assert!(pending.touch("main", start + SETTLE));
        assert_eq!(pending.check("main-2", start), Wait::Remaining(SETTLE));
    }
}
