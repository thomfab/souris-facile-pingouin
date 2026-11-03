# What's new

The section of a version becomes the notes of its GitHub release.

## 0.3

- **No more click to snap to the grid,** like the macOS version 0.6. While
  moving or resizing, the grid is shown (unless turned off) with the other
  windows dimmed, and window edges snap to its lines when they come close,
  like they do to display edges. The click goes to the app again. Sending a
  window to another display keeps its relative place and size instead of
  fitting the grid.
- An About item in the tray menu, opening the project page.

## 0.2

- A tray icon, with the macOS version's icon, and a menu to turn it on or
  off, open the settings and quit. StatusNotifierItem where the desktop
  supports it, the X11 system tray otherwise.
- A settings window like the macOS version's, with a page per display to
  set and preview its grid, forget remembered window sizes and launch at
  login. Also with `souris-facile-pingouin settings`. Changes go to the
  settings file, comments kept.
- The app icon in the desktop file, installed with `make install`.

## 0.1

- First release, for X11 sessions. Hold Ctrl + Alt and move the mouse to
  move the window under the pointer; add Super to resize it from the
  nearest corner.
- Window edges snap to display edges and nearby windows. Hold the click to
  snap to the grid of the display, with the grid lines shown and the other
  windows dimmed.
- While holding the move keys, swipe on a touchpad, turn the mouse wheel or
  press an arrow key to send the window to another display.
- Window sizes remembered per app and display.
- A single static executable, with its settings in a text file read again
  whenever it changes.
