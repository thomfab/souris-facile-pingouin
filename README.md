# Souris facile pingouin

A Linux app to move or resize the window under the pointer by holding
modifier keys and moving the mouse: no clicking, no dragging, no aiming for
a title bar or a window border.

It's the Linux version of [Souris facile
pomme](https://github.com/thomfab/souris-facile-pomme) for macOS, and works
with any window manager on X11. A single static executable, no
dependencies. Wayland support is planned; on GNOME, the [Souris Facile GNOME
Shell extension](https://github.com/thomfab/souris-facile) already works on
Wayland.

(Disclaimer: Claude Code helped me)

## How it works

Hold a key combination and move the mouse:

| Keys (default)           | Action                              |
| ------------------------ | ----------------------------------- |
| `Ctrl` + `Alt`           | Move the window under the pointer   |
| `Ctrl` + `Alt` + `Super` | Resize the window under the pointer |

Press and hold the click while holding the keys to snap to the grid.
Releasing it drops the window where it snapped and ends the move or resize:
release the keys and press them again to start another one.

Release the keys to stop. The combination must match exactly (Caps Lock and
Num Lock don't count), and both can be changed or turned off in the
settings. Since the resize keys include the move keys, adding `Super` while
moving switches to resizing, and releasing it switches back.

- **Moving** keeps the window at the same spot under the pointer. A
  maximized window is unmaximized as soon as it moves.
- **Resizing** pulls the corner nearest the pointer, so you can grow or
  shrink a window from any side. It can also be set to always use the
  bottom-right corner. Windows keep their minimum and maximum size.
- Fullscreen and minimized windows, panels, docks and the desktop are left
  alone.

### Snapping

Window edges snap to display edges (minus panels and docks) and to the
edges of neighbouring windows when they come within 16 pixels. The distance
is configurable, and `0` turns snapping off.

### Grid

Each display is divided into a grid of columns and rows. While you hold the
click, moved and resized edges always land on a grid line; the click itself
doesn't reach the app under the pointer. The grid follows the display under
the pointer, and its lines are shown while you hold the click, with the
other windows dimmed (with a compositor). Windows stacked above the one you
are moving stay bright, so turn on `raise-window` if you want everything
else dimmed.

Each display has its own grid size. A display seen for the first time gets a
grid suiting its physical size: 2 × 2 for 13–14″ laptops, 3 × 2 for 15–17″
laptops, 4 × 3 for larger displays, 8 × 4 for ultrawides and 2 × 3 for
portrait displays.

Grids are remembered per physical display (by its EDID), so plugging the
laptop into your monitors at home or at the office brings their grids back,
whatever the connector. `souris-facile-pingouin displays` lists the
connected displays with their grid, ready to paste into the settings, and
`souris-facile-pingouin displays --preview` briefly draws each grid on its
display to tell them apart.

### Sending a window to another display

While holding the move keys over a window:

- **Swipe two fingers** on a touchpad toward another display: the window
  goes to the display in that direction. One swipe moves it once.
- **Scroll the mouse wheel**: the window goes to the next or previous
  display, left to right.
- **Press an arrow key**: the window goes to the display in that direction.

The window keeps its relative place and size (a window on the left half of
the laptop screen lands on the left half of the monitor), fits the grid of
that display if you hold the click, and the pointer goes with it, so you can
keep moving it. Swipes, scrolls and arrow keys used this way don't reach the
app under the pointer. Both can be turned off in the settings.

### Window sizes per display

Each display remembers where each app's windows last were on it. A window
sent to a display, or dragged onto it, gets that size and place back: for
example Firefox half the laptop screen and a third of the office monitor.
Sizes are remembered when you move or resize a window with Souris facile
pingouin, and when a window leaves a display. X11 gives windows no ID that
survives an app restart, so this is per app (its `WM_CLASS`): an app's
windows share the same remembered size on a display. It can be turned off in
the settings.

## Settings

They're in `~/.config/souris-facile-pingouin/settings.toml`, written with
comments on first launch, and read again whenever the file changes: no need
to restart.

| Setting                  | Default                          |
| ------------------------ | -------------------------------- |
| `enabled`                | `true`                           |
| `move-modifiers`         | `["control", "alt"]`             |
| `resize-modifiers`       | `["control", "alt", "super"]`    |
| `raise-window`           | `false`                          |
| `resize-nearest-corner`  | `true`                           |
| `snap-threshold`         | `16` (pixels)                    |
| `snap-to-windows`        | `true`                           |
| `show-grid`              | `true`                           |
| `dim-windows`            | `true`                           |
| `send-with-scroll`       | `true`                           |
| `send-with-arrows`       | `true`                           |
| `remember-window-frames` | `true`                           |
| `[displays."<id>"]`      | `columns` and `rows` per display |

Modifiers are any of `shift`, `control`, `alt` and `super`. Combinations
that can't work well (the same keys for both, or a single key held for
ordinary shortcuts) are warned about when the settings are read.

The remembered window sizes are in
`~/.local/state/souris-facile-pingouin/window-frames.json`: delete it to
forget them.

## Installing

Download the latest release, or build it (see below), then:

```sh
make install     # into ~/.local/bin, with a desktop file
make autostart   # and start it with every session
```

Or just put the executable anywhere and run it. It needs an X11 session; on
Wayland it says so and quits.

If your window manager or desktop already uses `Ctrl` + `Alt` + a click,
scroll or arrow key (some use `Ctrl` + `Alt` + arrows to switch
workspaces), Souris facile pingouin says so on launch: pick other keys in
the settings, or change the desktop's shortcut.

## Building

Requires Rust (`rustup`) with the musl target, for a static executable:

```sh
rustup target add x86_64-unknown-linux-musl
make build     # target/x86_64-unknown-linux-musl/release/souris-facile-pingouin
make run       # build and launch
make test      # run the unit tests
```

`SOURIS_FACILE_DEBUG=1 souris-facile-pingouin` prints every X event it
receives.

### How it works inside

- `src/mover.rs` holds the logic (moving, resizing, snapping, the grid,
  sending to displays), written against the small `Platform` trait in
  `src/platform.rs`, and tested with a fake one.
- `src/x11/` is the X11 platform: XInput 2 raw events to follow the keys
  and the pointer everywhere, passive grabs to keep the clicks, scrolls and
  arrow keys used from the apps, EWMH (`_NET_MOVERESIZE_WINDOW`) to move
  windows through the window manager, RandR and EDID for the displays, and
  click-through override-redirect windows for the grid and the dimming.

### Releasing

Each release gets a section in [CHANGELOG.md](CHANGELOG.md), whose heading
is the version (`## 0.1`). Pushing the tag `v0.1` publishes the release with
that section as its notes, and fails if it's missing or doesn't match the
version in `Cargo.toml`.

`make release` builds `build/souris-facile-pingouin-<version>-x86_64.tar.gz`.

## License

GPL-2.0-or-later, see [LICENSE](LICENSE).
