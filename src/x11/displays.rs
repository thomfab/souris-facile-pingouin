use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

use super::{values32, Res, X11};
use crate::geometry::{Rect, Size};
use crate::platform::Display;
use crate::settings::{Settings, GRID_PRESETS};

// A display's identity, from its EDID: the maker, the model and the
// serial number, so that it's recognised on any connector.
#[derive(Debug, PartialEq)]
struct Edid {
    id: String,
    name: Option<String>,
}

fn parse_edid(edid: &[u8]) -> Option<Edid> {
    if edid.len() < 128 || edid[..8] != [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0] {
        return None;
    }
    let maker = u16::from_be_bytes([edid[8], edid[9]]);
    let letter = |shift: u16| (b'@' + ((maker >> shift) & 0x1F) as u8) as char;
    let maker: String = [letter(10), letter(5), letter(0)].into_iter().collect();
    let product = u16::from_le_bytes([edid[10], edid[11]]);
    let serial = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);

    // Four 18-byte descriptors, some of which hold text.
    let text = |tag: u8| {
        (0..4).map(|i| &edid[54 + 18 * i..72 + 18 * i]).find(|d| d[..3] == [0, 0, 0] && d[3] == tag).map(|d| {
            let text: String = d[5..].iter().take_while(|&&b| b != b'\n' && b != 0).map(|&b| b as char).collect();
            text.trim().to_string()
        })
    };
    let serial_text = text(0xFF).filter(|s| !s.is_empty());
    let id = match (serial, serial_text) {
        (0, Some(text)) => format!("{maker}-{product:04X}-{text}"),
        _ => format!("{maker}-{product:04X}-{serial:08X}"),
    };
    Some(Edid { id, name: text(0xFC).filter(|s| !s.is_empty()) })
}

// Space reserved by a panel or dock along an edge of the whole screen:
// left, right, top, bottom, then the start and end of each along the edge
// (_NET_WM_STRUT_PARTIAL).
type Strut = [u32; 12];

fn overlaps(start: u32, end: u32, low: f64, high: f64) -> bool {
    (start as f64) < high && (end as f64 + 1.0) > low
}

// A display's frame minus the panels and docks on it.
fn work_area(monitor: Rect, screen: Size, struts: &[Strut], workarea: Option<Rect>) -> Rect {
    let m = monitor;
    let (mut left, mut right, mut top, mut bottom) = (m.min_x(), m.max_x(), m.min_y(), m.max_y());
    for s in struts {
        let [l, r, t, b, ly0, ly1, ry0, ry1, tx0, tx1, bx0, bx1] = *s;
        if l > 0 && overlaps(ly0, ly1, m.min_y(), m.max_y()) && m.min_x() < l as f64 {
            left = left.max(l as f64);
        }
        let edge = screen.width - r as f64;
        if r > 0 && overlaps(ry0, ry1, m.min_y(), m.max_y()) && m.max_x() > edge {
            right = right.min(edge);
        }
        if t > 0 && overlaps(tx0, tx1, m.min_x(), m.max_x()) && m.min_y() < t as f64 {
            top = top.max(t as f64);
        }
        let edge = screen.height - b as f64;
        if b > 0 && overlaps(bx0, bx1, m.min_x(), m.max_x()) && m.max_y() > edge {
            bottom = bottom.min(edge);
        }
    }
    let area = Rect::new(left, top, (right - left).max(1.0), (bottom - top).max(1.0));
    // The window manager's own work area accounts for panels it draws
    // itself, but is a single rectangle for all displays.
    workarea.and_then(|w| area.intersection(&w)).unwrap_or(area)
}

impl X11 {
    fn struts(&self) -> Vec<Strut> {
        let windows = self.root_property(self.atoms._NET_CLIENT_LIST).unwrap_or_default();
        let prop = |w, atom| self.conn.get_property(false, w, atom, AtomEnum::CARDINAL, 0, 12);
        let cookies: Vec<_> = windows
            .iter()
            .map(|&w| (prop(w, self.atoms._NET_WM_STRUT_PARTIAL), prop(w, self.atoms._NET_WM_STRUT)))
            .collect();
        cookies
            .into_iter()
            .filter_map(|(partial, full)| {
                if let Some(v) = partial.ok().and_then(|c| values32(c.reply())).filter(|v| v.len() == 12) {
                    return v.try_into().ok();
                }
                let v = full.ok().and_then(|c| values32(c.reply())).filter(|v| v.len() == 4)?;
                Some([v[0], v[1], v[2], v[3], 0, u32::MAX, 0, u32::MAX, 0, u32::MAX, 0, u32::MAX])
            })
            .collect()
    }

    fn workarea(&self) -> Option<Rect> {
        let areas = self.root_property(self.atoms._NET_WORKAREA)?;
        let i = 4 * self.current_desktop().unwrap_or(0) as usize;
        let a = areas.get(i..i + 4).or(areas.get(0..4))?;
        Some(Rect::new(a[0] as i32 as f64, a[1] as i32 as f64, a[2] as f64, a[3] as f64))
    }
}

pub fn displays(x: &X11) -> Res<Vec<Display>> {
    let monitors = x.conn.randr_get_monitors(x.root, true)?.reply()?.monitors;
    let screen = &x.conn.setup().roots[x.screen];
    let screen_size = Size::new(screen.width_in_pixels as f64, screen.height_in_pixels as f64);
    let struts = x.struts();
    let workarea = x.workarea();

    let mut displays = Vec::new();
    for m in &monitors {
        let connector = String::from_utf8_lossy(&x.conn.get_atom_name(m.name)?.reply()?.name).into_owned();
        let edid = match m.outputs.first() {
            Some(&output) => x
                .conn
                .randr_get_output_property(output, x.atoms.EDID, AtomEnum::ANY, 0, 64, false, false)?
                .reply()
                .ok()
                .and_then(|r| parse_edid(&r.data)),
            None => None,
        };
        let frame = Rect::new(m.x as f64, m.y as f64, m.width as f64, m.height as f64);
        let builtin = ["eDP", "LVDS", "DSI"].iter().any(|c| connector.starts_with(c));
        let fallback = if builtin { "Built-in Display".to_string() } else { connector.clone() };
        let display = Display {
            id: edid.as_ref().map_or(connector, |e| e.id.clone()),
            name: edid.and_then(|e| e.name).unwrap_or(fallback),
            frame,
            work_area: work_area(frame, screen_size, &struts, workarea),
            size_mm: Size::new(m.width_in_millimeters as f64, m.height_in_millimeters as f64),
            builtin,
        };
        if m.primary {
            displays.insert(0, display);
        } else {
            displays.push(display);
        }
    }
    Ok(displays)
}

// Lists the displays with their grid, ready to paste into the settings.
// With `preview`, also shows each display's grid for a few seconds.
pub fn print_displays(settings: &Settings, preview: bool) -> Res<()> {
    let x = X11::connect()?;
    let displays = displays(&x)?;
    for (i, d) in displays.iter().enumerate() {
        let (columns, rows) = settings.grid(d);
        let r = d.frame;
        let w = d.work_area;
        let set = if settings.displays.contains_key(&d.id) { "set" } else { "suits its size" };
        println!("# {}{}", d.name, if i == 0 { " (primary)" } else { "" });
        println!("#   {}×{} at {},{}, {}×{} mm", r.width, r.height, r.x, r.y, d.size_mm.width, d.size_mm.height);
        println!("#   work area {}×{} at {},{}", w.width, w.height, w.x, w.y);
        println!("#   grid {columns} × {rows} ({set})");
        println!("[displays.\"{}\"]\nname = \"{}\"\ncolumns = {columns}\nrows = {rows}\n", d.id, d.name);
    }
    let presets: Vec<String> = GRID_PRESETS.iter().map(|p| format!("{} {} × {}", p.name, p.columns, p.rows)).collect();
    println!("# Presets: {}.", presets.join(", "));
    if preview {
        let mut overlays = super::overlays::Overlays::new(&x.conn, x.screen)?;
        for d in &displays {
            let (columns, rows) = settings.grid(d);
            overlays.add_grid(&x.conn, x.root, d.work_area, columns, rows);
        }
        x.conn.flush()?;
        std::thread::sleep(Duration::from_secs(3));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edid(maker: &[u8; 2], product: u16, serial: u32, descriptors: &[(u8, &str)]) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        e[8..10].copy_from_slice(maker);
        e[10..12].copy_from_slice(&product.to_le_bytes());
        e[12..16].copy_from_slice(&serial.to_le_bytes());
        for (i, (tag, text)) in descriptors.iter().enumerate() {
            let d = &mut e[54 + 18 * i..72 + 18 * i];
            d[3] = *tag;
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(b'\n');
            bytes.resize(13, b' ');
            d[5..].copy_from_slice(&bytes);
        }
        e
    }

    // "DEL" is 0x10AC.
    const DELL: [u8; 2] = [0x10, 0xAC];

    #[test]
    fn edid_ids_are_maker_product_and_serial() {
        let e = edid(&DELL, 0xA0FE, 0x12345678, &[(0xFC, "DELL U3421WE")]);
        assert_eq!(parse_edid(&e), Some(Edid { id: "DEL-A0FE-12345678".into(), name: Some("DELL U3421WE".into()) }));
    }

    #[test]
    fn edid_serial_text_stands_in_for_a_missing_serial() {
        let e = edid(&DELL, 0xA0FE, 0, &[(0xFF, "7XYZ123"), (0xFC, "DELL U3421WE")]);
        assert_eq!(parse_edid(&e).unwrap().id, "DEL-A0FE-7XYZ123");
    }

    #[test]
    fn bad_edids_are_ignored() {
        assert_eq!(parse_edid(&[]), None);
        assert_eq!(parse_edid(&[0u8; 128]), None);
    }

    const SCREEN: Size = Size { width: 3840.0, height: 1440.0 };
    // A laptop on the left, its top at 0, and a taller monitor on the right.
    const LAPTOP: Rect = Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
    const MONITOR: Rect = Rect { x: 1920.0, y: 0.0, width: 1920.0, height: 1440.0 };

    #[test]
    fn struts_only_take_from_the_displays_they_cover() {
        // A 32 px top panel over the laptop, a 48 px dock at the bottom of the monitor.
        let top = [0, 0, 32, 0, 0, 0, 0, 0, 0, 1919, 0, 0];
        let bottom = [0, 0, 0, 48, 0, 0, 0, 0, 0, 0, 1920, 3839];
        assert_eq!(work_area(LAPTOP, SCREEN, &[top, bottom], None), Rect::new(0.0, 32.0, 1920.0, 1048.0));
        assert_eq!(work_area(MONITOR, SCREEN, &[top, bottom], None), Rect::new(1920.0, 0.0, 1920.0, 1392.0));
    }

    #[test]
    fn a_bottom_strut_reaches_up_from_the_bottom_of_the_screen() {
        // The laptop's bottom, at 1080, is above a 48 px strut from 1440.
        let bottom = [0, 0, 0, 48, 0, 0, 0, 0, 0, 0, 0, 3839];
        assert_eq!(work_area(LAPTOP, SCREEN, &[bottom], None), LAPTOP);
    }

    #[test]
    fn the_window_managers_work_area_applies_too() {
        let workarea = Rect::new(0.0, 27.0, 3840.0, 1413.0);
        assert_eq!(work_area(LAPTOP, SCREEN, &[], Some(workarea)), Rect::new(0.0, 27.0, 1920.0, 1053.0));
    }
}
