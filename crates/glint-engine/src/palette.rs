// SPDX-License-Identifier: MIT
//! Reading the watched spot outside the hot loop (PLAN §12.9): *Sample center* reads it once, and
//! *Record 5 s* reads it for a while and groups the colors it saw. Both read the anchor pixel of
//! the pattern the hot thread watches, from the drawable it reads (the root, or the window for the
//! window-buffer backend), on connection C.

use std::time::{Duration, Instant};

use glint_core::Rgb;
use glint_core::pattern::{Rect, SamplePattern};
use glint_ipc::PaletteEntry;
use glint_x11::format::PixelFormat;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

/// How often a recording reads the spot.
const EVERY: Duration = Duration::from_millis(4);
/// The most colors a recording returns.
pub const MAX_ENTRIES: usize = 12;

/// Where the watched spot is: the pattern's rectangle in a drawable, and its pixel format.
#[derive(Clone, Copy, Debug)]
pub struct Spot {
    pub drawable: u32,
    pub format: PixelFormat,
    pub rect: Rect,
    pub pattern: SamplePattern,
}

impl Spot {
    /// The color at the pattern's anchor right now.
    pub fn read(&self, conn: &RustConnection) -> Result<Rgb, String> {
        let r = self.rect;
        let (x, y) = (
            i16::try_from(r.x).map_err(|e| e.to_string())?,
            i16::try_from(r.y).map_err(|e| e.to_string())?,
        );
        let reply = conn
            .get_image(ImageFormat::Z_PIXMAP, self.drawable, x, y, r.w, r.h, u32::MAX)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let mut px = vec![0u32; usize::from(r.w) * usize::from(r.h)];
        self.format
            .decode(&reply.data, r.w, r.h, &mut px)
            .map_err(|e| e.to_string())?;
        let anchor = self.pattern.anchor_pixel(&px).unwrap_or(px[0]);
        Ok(Rgb::from_u32(anchor))
    }
}

/// Reads `spot` every 4 ms for `length`, then groups what it saw.
pub fn record(conn: &RustConnection, spot: Spot, length: Duration) -> Result<Vec<PaletteEntry>, String> {
    let end = Instant::now() + length;
    let mut samples = Vec::with_capacity((length.as_millis() / EVERY.as_millis()) as usize + 1);
    let mut next = Instant::now();
    while Instant::now() < end {
        samples.push(spot.read(conn)?.to_u32());
        next += EVERY;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    Ok(group(&samples))
}

/// Groups near-identical colors (the same top 5 bits per channel), names each group by its most
/// frequent exact color, and returns the most frequent groups first, with their share.
pub fn group(samples: &[u32]) -> Vec<PaletteEntry> {
    use std::collections::HashMap;
    let mut groups: HashMap<u32, HashMap<u32, u32>> = HashMap::new();
    for &c in samples {
        let key = c & 0x00F8_F8F8;
        *groups.entry(key).or_default().entry(c & 0x00FF_FFFF).or_default() += 1;
    }
    let total = samples.len().max(1) as f32;
    let mut entries: Vec<(u32, u32)> = groups
        .into_values()
        .map(|exact| {
            let count = exact.values().sum::<u32>();
            // The most frequent exact color; ties go to the smaller value, so the result is stable.
            let rgb = exact
                .into_iter()
                .max_by_key(|&(c, n)| (n, std::cmp::Reverse(c)))
                .map_or(0, |(c, _)| c);
            (rgb, count)
        })
        .collect();
    entries.sort_by_key(|&(rgb, n)| (std::cmp::Reverse(n), rgb));
    entries
        .into_iter()
        .take(MAX_ENTRIES)
        .map(|(rgb, n)| PaletteEntry {
            rgb: Rgb::from_u32(rgb),
            share: n as f32 / total,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_identical_colors_group_under_the_most_frequent() {
        // A purple that wobbles by a step or two, a pink, and a little noise.
        let mut s = vec![0x86_04_A0; 50];
        s.extend([0x87_05_A1; 20]);
        s.extend([0xCB_49_DE; 25]);
        s.extend([0x10_10_10; 5]);
        let p = group(&s);
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].rgb, p[0].share), (Rgb::from_u32(0x86_04_A0), 0.7));
        assert_eq!(p[1].rgb, Rgb::from_u32(0xCB_49_DE));
        assert_eq!(p[2].rgb, Rgb::from_u32(0x10_10_10));
    }

    #[test]
    fn at_most_twelve_and_nothing_from_nothing() {
        let s: Vec<u32> = (0..40u32).map(|i| i * 0x08_08_08).collect();
        assert_eq!(group(&s).len(), MAX_ENTRIES);
        assert!(group(&[]).is_empty());
    }
}
