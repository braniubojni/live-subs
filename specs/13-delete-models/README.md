# 13 Delete models

## Goal
The settings window can delete one downloaded model, or every file in the models folder. The list shows which catalogue rows are already on disk. A confirm dialog sits in front of either delete. Nothing outside the models folder is removed.

## Current state
Downloads land in [src/models/download.rs](../../src/models/download.rs) `model_dir()` (`~/Library/Application Support/live-subs/models` on macOS). The settings list in [src/setup.rs](../../src/setup.rs) shows name and id only. `SettingsState` has no remove event. A chosen file outside that folder (Browse) is left alone.

`remove_in` / `remove_all_in` delete only inside a given directory. `Event::Removed` marks that slot not present and clears its progress. The list appends "downloaded" when the catalogue file is on disk. Remove and "Delete all downloaded" open a confirm dialog; confirming stops capture first. The dialog itself still needs a manual run.

When [12](../12-settings-ui/README.md) replaces this window, the MUI form keeps the same two actions.

## Todo
- [x] **Delete one file inside the models dir.** `remove_in(dir, path)` deletes that file and a sibling `.partial`. A path outside `dir` is an error and the file stays. Accept: unit test, no window.
- [x] **Delete all.** `remove_all_in(dir)` deletes files in that directory and not in subdirectories. Accept: unit test.
- [x] **Downloaded mark.** A catalogue row whose file is on disk shows "downloaded" in the list.
- [x] **Remove, with a dialog.** The selected downloaded model has Remove. Confirm names the model. Cancel leaves the file. Confirm deletes it, marks the slot not present, and stops capture if it is running.
- [x] **Delete all, with a dialog.** One button. Confirm lists that every downloaded file goes. Same cancel / stop rule.
- [ ] **Manual run.** On this Mac, delete one downloaded model and delete all. Accept: the file is gone from the models folder, the row no longer says downloaded, and Cancel deletes nothing.

## Tests
`remove_in` and `remove_all_in` in `download.rs`. `Event::Removed` in `settings.rs`. No microphone. The dialog is the manual run above.

## Out of scope
Deleting a model the user browsed to from somewhere else. Uninstall of the app or the config file ([10](../10-packaging/README.md)). The subtitle overlay.
