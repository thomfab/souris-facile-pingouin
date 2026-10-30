use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use x11rb::connection::Connection;
use x11rb::protocol::xinput::{
    self, ConnectionExt as _, Device, EventMode, GrabMode22, GrabOwner, GrabType, XIEventMask,
    XIGetPropertyItems,
};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateWindowAux, GrabMode, GrabStatus, Keycode, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::{COPY_FROM_PARENT, CURRENT_TIME};

use super::{Res, X11};
use crate::geometry::Direction;
use crate::memory::MemoryStore;
use crate::mover::Mover;
use crate::settings::{self, Modifier, Mods, Settings};

const SHIFT: u16 = 1 << 0;
const LOCK: u16 = 1 << 1;
const CONTROL: u16 = 1 << 2;
const MOD1: u16 = 1 << 3;
const MOD2: u16 = 1 << 4;
const MOD4: u16 = 1 << 6;

// The keysyms needed, from X11/keysymdef.h.
const XK_LEFT: u32 = 0xFF51;
const XK_UP: u32 = 0xFF52;
const XK_RIGHT: u32 = 0xFF53;
const XK_DOWN: u32 = 0xFF54;
const XK_NUM_LOCK: u32 = 0xFF7F;
const XK_ALT_L: u32 = 0xFFE9;
const XK_ALT_R: u32 = 0xFFEA;
const XK_SUPER_L: u32 = 0xFFEB;
const XK_SUPER_R: u32 = 0xFFEC;

// A touchpad scroll step, in the units of a swipe's travel.
const TOUCHPAD_STEP: f64 = 15.0;

// Which modifier bits the keys are on, and the arrow keys' keycodes.
struct Keymap {
    alt: u16,
    super_: u16,
    num_lock: u16,
    arrows: Vec<(Keycode, Direction)>,
}

impl Keymap {
    fn load(x: &X11) -> Res<Keymap> {
        let setup = x.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let mapping = x.conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let per_keycode = mapping.keysyms_per_keycode as usize;
        let keysyms = |code: Keycode| -> &[u32] {
            let i = (code.saturating_sub(min)) as usize * per_keycode;
            mapping.keysyms.get(i..i + per_keycode).unwrap_or(&[])
        };

        let modifiers = x.conn.get_modifier_mapping()?.reply()?;
        let per_modifier = modifiers.keycodes.len() / 8;
        let bit_of = |wanted: &[u32], default: u16| {
            (3..8)
                .find(|&m| {
                    modifiers.keycodes[m * per_modifier..(m + 1) * per_modifier]
                        .iter()
                        .any(|&code| code != 0 && keysyms(code).iter().any(|k| wanted.contains(k)))
                })
                .map_or(default, |m| 1 << m)
        };

        let arrows = (min..=max)
            .filter_map(|code| {
                let direction = match keysyms(code).first()? {
                    &XK_LEFT => Direction::Left,
                    &XK_RIGHT => Direction::Right,
                    &XK_UP => Direction::Up,
                    &XK_DOWN => Direction::Down,
                    _ => return None,
                };
                Some((code, direction))
            })
            .collect();

        Ok(Keymap {
            alt: bit_of(&[XK_ALT_L, XK_ALT_R], MOD1),
            super_: bit_of(&[XK_SUPER_L, XK_SUPER_R], MOD4),
            num_lock: bit_of(&[XK_NUM_LOCK], MOD2),
            arrows,
        })
    }

    // Other modifiers (Caps Lock, Num Lock, AltGr) are ignored.
    fn mods(&self, mask: u16) -> Mods {
        let mut mods = Mods::NONE;
        for (bit, modifier) in [(SHIFT, Modifier::Shift), (CONTROL, Modifier::Control), (self.alt, Modifier::Alt),
                                (self.super_, Modifier::Super)] {
            if mask & bit != 0 {
                mods = mods.with(modifier);
            }
        }
        mods
    }

    fn mask(&self, mods: Mods) -> u16 {
        mods.iter()
            .map(|m| match m {
                Modifier::Shift => SHIFT,
                Modifier::Control => CONTROL,
                Modifier::Alt => self.alt,
                Modifier::Super => self.super_,
            })
            .fold(0, |a, b| a | b)
    }

    // A grab needs every combination of the ignored modifiers.
    fn with_locks(&self, mask: u16) -> [u32; 4] {
        [0, LOCK, self.num_lock, LOCK | self.num_lock].map(|lock| (mask | lock) as u32)
    }
}

// How a scrolling device's steps translate into movement.
#[derive(Clone, Copy)]
struct ScrollDevice {
    touchpad: bool,
    // Natural scrolling inverts the steps.
    natural: bool,
}

struct Grab {
    detail: u32,
    kind: GrabType,
    modifiers: Vec<u32>,
}

struct Input {
    keymap: Keymap,
    grabs: Vec<Grab>,
    devices: HashMap<u16, ScrollDevice>,
    started: Instant,
}

impl Input {
    // Grabs the clicks, scrolls and arrow keys used with the keys held, so
    // that the app under the pointer doesn't get them. The click grab is
    // synchronous, to give the click back when it turns out not to be used.
    // The others aren't, so that the keyboard can never stay frozen.
    fn grab(&mut self, x: &X11, settings: &Settings) {
        self.ungrab(x);
        if !settings.enabled {
            return;
        }
        let k = &self.keymap;
        let combos = |mods: &[Mods]| -> Vec<u32> {
            mods.iter().filter(|m| !m.is_empty()).flat_map(|&m| k.with_locks(k.mask(m))).collect()
        };
        let both = combos(&[settings.move_mods(), settings.resize_mods()]);
        let moving = combos(&[settings.move_mods()]);

        let mut wanted = vec![(1, GrabType::BUTTON, GrabMode22::SYNC, both, "the click")];
        if settings.send_with_scroll {
            for button in 4..=7 {
                wanted.push((button, GrabType::BUTTON, GrabMode22::ASYNC, moving.clone(), "scrolling"));
            }
        }
        if settings.send_with_arrows {
            for &(code, _) in &k.arrows {
                wanted.push((code as u32, GrabType::KEYCODE, GrabMode22::ASYNC, moving.clone(), "the arrow keys"));
            }
        }

        let mut failed = Vec::new();
        for (detail, kind, mode, modifiers, what) in wanted {
            if modifiers.is_empty() {
                continue;
            }
            let mask = if kind == GrabType::BUTTON {
                XIEventMask::BUTTON_PRESS | XIEventMask::BUTTON_RELEASE
            } else {
                XIEventMask::KEY_PRESS | XIEventMask::KEY_RELEASE
            };
            let reply = x
                .conn
                .xinput_xi_passive_grab_device(CURRENT_TIME, x.root, 0, detail, Device::ALL_MASTER, kind,
                                               mode, GrabMode::ASYNC, GrabOwner::NO_OWNER,
                                               &[u32::from(mask)], &modifiers)
                .ok()
                .and_then(|c| c.reply().ok());
            if reply.is_some_and(|r| r.modifiers.iter().any(|m| m.status != GrabStatus::SUCCESS)) && !failed.contains(&what) {
                failed.push(what);
            }
            self.grabs.push(Grab { detail, kind, modifiers });
        }
        for what in failed {
            eprintln!("Another program already uses {what} with these keys: it may not always work.");
        }
        let _ = x.conn.flush();
    }

    fn ungrab(&mut self, x: &X11) {
        for grab in self.grabs.drain(..) {
            let _ = x.conn.xinput_xi_passive_ungrab_device(x.root, grab.detail, Device::ALL_MASTER, grab.kind,
                                                           &grab.modifiers);
        }
    }

    fn scroll_device(&mut self, x: &X11, id: u16) -> ScrollDevice {
        if let Some(&device) = self.devices.get(&id) {
            return device;
        }
        let name = x
            .conn
            .xinput_xi_query_device(id)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.infos.into_iter().next())
            .map(|i| String::from_utf8_lossy(&i.name).to_lowercase())
            .unwrap_or_default();
        let natural = x
            .conn
            .xinput_xi_get_property(id, false, x.atoms.NATURAL_SCROLLING, 0u32, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| matches!(r.items, XIGetPropertyItems::Data8(v) if v.first() == Some(&1)));
        let touchpad = ["touchpad", "trackpad", "magic mouse"].iter().any(|t| name.contains(t));
        let device = ScrollDevice { touchpad, natural };
        self.devices.insert(id, device);
        device
    }

    fn time(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn load_settings(path: &Path) -> Option<Settings> {
    match Settings::load(path) {
        Ok(settings) => {
            for warning in settings::warnings(&settings.move_modifiers, &settings.resize_modifiers) {
                eprintln!("{warning}");
            }
            Some(settings)
        }
        Err(e) => {
            eprintln!("{e}");
            None
        }
    }
}

// Makes sure only one instance runs, by owning a selection.
fn claim_instance(x: &X11) -> Res<()> {
    let owner = x.conn.get_selection_owner(x.atoms.INSTANCE)?.reply()?.owner;
    if owner != 0 {
        return Err("Souris facile pingouin is already running.".into());
    }
    let window = x.conn.generate_id()?;
    x.conn.create_window(COPY_FROM_PARENT as u8, window, x.root, -1, -1, 1, 1, 0, WindowClass::INPUT_ONLY,
                         COPY_FROM_PARENT, &CreateWindowAux::new().override_redirect(1))?;
    x.conn.set_selection_owner(window, x.atoms.INSTANCE, CURRENT_TIME)?;
    Ok(())
}

pub fn run(settings_path: &Path, frames_path: &Path) -> Res<()> {
    let mut x = X11::connect()?;
    claim_instance(&x)?;
    let version = x.conn.xinput_xi_query_version(2, 2)?.reply()?;
    if (version.major_version, version.minor_version) < (2, 2) {
        return Err("The X server needs XInput 2.2 or later.".into());
    }

    let settings = load_settings(settings_path).unwrap_or_default();
    let mut settings_time = modified(settings_path);
    let mut last_check = Instant::now();
    let mut input =
        Input { keymap: Keymap::load(&x)?, grabs: Vec::new(), devices: HashMap::new(), started: Instant::now() };
    input.grab(&x, &settings);

    // Every key and pointer change, wherever it goes.
    let raw = XIEventMask::RAW_MOTION
        | XIEventMask::RAW_KEY_PRESS
        | XIEventMask::RAW_KEY_RELEASE
        | XIEventMask::RAW_BUTTON_PRESS
        | XIEventMask::RAW_BUTTON_RELEASE;
    // Raw events come from the physical devices, which also report devices
    // coming and going.
    let mask = xinput::EventMask { deviceid: Device::ALL.into(), mask: vec![raw | XIEventMask::HIERARCHY] };
    x.conn.xinput_xi_select_events(x.root, &[mask])?;
    x.conn.flush()?;

    eprintln!("Souris facile pingouin is running: hold {} and move the mouse to move a window, {} to resize it.",
              settings.move_mods().label(), settings.resize_mods().label());
    let mut mover: Mover<X11> = Mover::new(settings, MemoryStore::File(frames_path.to_path_buf()));

    let debug = std::env::var_os("SOURIS_FACILE_DEBUG").is_some();
    loop {
        let mut event = Some(x.conn.wait_for_event()?);
        let mut changed = false;
        while let Some(e) = event {
            if debug {
                eprintln!("{e:?}");
            }
            match e {
                Event::XinputRawMotion(_)
                | Event::XinputRawKeyPress(_)
                | Event::XinputRawKeyRelease(_)
                | Event::XinputRawButtonPress(_)
                | Event::XinputRawButtonRelease(_) => changed = true,

                Event::XinputButtonPress(e) => {
                    let mods = input.keymap.mods(e.mods.effective as u16);
                    match e.detail {
                        1 => {
                            let mode = if mover.click_down(mods) { EventMode::ASYNC_DEVICE } else { EventMode::REPLAY_DEVICE };
                            x.conn.xinput_xi_allow_events(e.time, e.deviceid, mode, 0, x.root)?;
                        }
                        4..=7 => {
                            let device = input.scroll_device(&x, e.sourceid);
                            let step = if device.touchpad { TOUCHPAD_STEP } else { 1.0 };
                            let sign = if device.natural { -step } else { step };
                            let (dx, dy) = match e.detail {
                                4 => (0.0, -sign),
                                5 => (0.0, sign),
                                6 => (-sign, 0.0),
                                _ => (sign, 0.0),
                            };
                            let time = input.time();
                            mover.scroll(&mut x, mods, dx, dy, device.touchpad, time);
                        }
                        _ => {}
                    }
                    changed = true;
                }
                Event::XinputButtonRelease(e) => {
                    if e.detail == 1 {
                        mover.click_up();
                    }
                    changed = true;
                }
                Event::XinputKeyPress(e) => {
                    let mods = input.keymap.mods(e.mods.effective as u16);
                    let repeat = e.flags.contains(xinput::KeyEventFlags::KEY_REPEAT);
                    let direction = input.keymap.arrows.iter().find(|a| a.0 as u32 == e.detail).map(|a| a.1);
                    if let Some(direction) = direction {
                        mover.arrow(&mut x, mods, direction, repeat);
                    }
                }
                Event::XinputHierarchy(_) => input.devices.clear(),
                Event::MappingNotify(_) => {
                    input.keymap = Keymap::load(&x)?;
                    input.grab(&x, &mover.settings);
                }
                Event::Error(e) => {
                    // Windows can vanish while being looked at.
                    if !matches!(e.error_kind, x11rb::protocol::ErrorKind::Window | x11rb::protocol::ErrorKind::Drawable) {
                        eprintln!("X11 error: {e:?}");
                    }
                }
                _ => {}
            }
            event = x.conn.poll_for_event()?;
        }

        // Bursts of events make a single update, so that a slow window
        // manager doesn't make the pointer lag.
        if changed {
            if let Some((point, mask)) = x.query_pointer() {
                mover.update(&mut x, point, input.keymap.mods(mask));
            }
        }

        // The settings change only between operations.
        if mover.is_idle() && last_check.elapsed() > Duration::from_secs(1) {
            last_check = Instant::now();
            let time = modified(settings_path);
            if time != settings_time {
                settings_time = time;
                if let Some(new) = load_settings(settings_path) {
                    eprintln!("Settings reloaded.");
                    mover.settings = new;
                    input.grab(&x, &mover.settings);
                }
            }
        }
        x.conn.flush()?;
    }
}
