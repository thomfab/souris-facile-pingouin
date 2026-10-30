// The app icon, the same as the macOS version's, at the sizes trays and
// window managers ask for.

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

const SIZES: [(u32, &[u8]); 8] = [
    (16, include_bytes!("../resources/icons/16.png")),
    (22, include_bytes!("../resources/icons/22.png")),
    (24, include_bytes!("../resources/icons/24.png")),
    (32, include_bytes!("../resources/icons/32.png")),
    (48, include_bytes!("../resources/icons/48.png")),
    (64, include_bytes!("../resources/icons/64.png")),
    (128, include_bytes!("../resources/icons/128.png")),
    (256, include_bytes!("../resources/icons/256.png")),
];

// The icon at `size` pixels, scaled down from the nearest larger one.
pub fn pixmap(size: u32) -> Pixmap {
    let size = size.max(1);
    let &(source_size, png) = SIZES.iter().find(|s| s.0 >= size).unwrap_or(&SIZES[SIZES.len() - 1]);
    let source = Pixmap::decode_png(png).expect("the embedded icons are valid PNGs");
    if source_size == size {
        return source;
    }
    let mut pixmap = Pixmap::new(size, size).expect("a non-empty size");
    let scale = size as f32 / source_size as f32;
    let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
    pixmap.draw_pixmap(0, 0, source.as_ref(), &paint, Transform::from_scale(scale, scale), None);
    pixmap
}

// Non-premultiplied ARGB, one u32 per pixel, as _NET_WM_ICON and
// StatusNotifierItem want them.
pub fn argb(size: u32) -> Vec<u32> {
    pixmap(size)
        .pixels()
        .iter()
        .map(|p| {
            let c = p.demultiply();
            u32::from_be_bytes([c.alpha(), c.red(), c.green(), c.blue()])
        })
        .collect()
}
