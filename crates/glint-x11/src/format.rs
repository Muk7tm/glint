// SPDX-License-Identifier: MIT
//! Pixel formats (PLAN §5.2, Appendix A.3): never assume BGRX. The drawable's visual masks, the
//! bits per pixel and scanline padding of its depth, and the server's image byte order decide how
//! a captured pixel becomes `0xRRGGBB`.

use x11rb_protocol::protocol::xproto::{ImageOrder, Setup, VisualClass};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("visual {0:#x} isn't on this screen")]
    NoVisual(u32),
    #[error("visual {0:#x} isn't TrueColor or DirectColor")]
    NotTrueColor(u32),
    #[error("color mask {0:#x} isn't one run of bits")]
    Mask(u32),
    #[error("{0} bits per color channel is too few; Glint needs at least 4")]
    TooFewBits(u32),
    #[error("{0} bits per pixel isn't supported (16, 24 or 32)")]
    BitsPerPixel(u8),
    #[error("the server lists no pixmap format for depth {0}")]
    NoPixmapFormat(u8),
    #[error("scanline padding of {0} bits isn't supported")]
    Pad(u8),
    #[error("the image has {got} bytes, {want} expected")]
    Short { got: usize, want: usize },
    #[error("{0} pixels don't fit the output")]
    Output(usize),
}

/// Where one color channel sits in a pixel value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Channel {
    shift: u32,
    bits: u32,
}

impl Channel {
    fn from_mask(mask: u32) -> Result<Self, FormatError> {
        let shift = mask.trailing_zeros();
        let bits = mask.count_ones();
        if mask == 0 || (mask >> shift).checked_add(1).is_none_or(|v| !v.is_power_of_two()) {
            return Err(FormatError::Mask(mask));
        }
        if bits < 4 {
            return Err(FormatError::TooFewBits(bits));
        }
        Ok(Self { shift, bits })
    }

    /// The channel scaled to 8 bits: extra bits are dropped (30-bit color keeps the top 8), and
    /// fewer bits are expanded by bit replication (5 bits `abcde` become `abcdeabc`).
    #[inline]
    fn to8(self, pixel: u32) -> u32 {
        let v = (pixel >> self.shift) & (u32::MAX >> (32 - self.bits));
        if self.bits >= 8 {
            v >> (self.bits - 8)
        } else {
            (v << (8 - self.bits)) | (v >> (2 * self.bits - 8))
        }
    }
}

/// How to turn a drawable's image bytes into `0xRRGGBB` pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelFormat {
    bpp: u8,
    pad: u8,
    msb_first: bool,
    r: Channel,
    g: Channel,
    b: Channel,
}

impl PixelFormat {
    pub fn new(
        red_mask: u32,
        green_mask: u32,
        blue_mask: u32,
        bits_per_pixel: u8,
        scanline_pad: u8,
        msb_first: bool,
    ) -> Result<Self, FormatError> {
        if !matches!(bits_per_pixel, 16 | 24 | 32) {
            return Err(FormatError::BitsPerPixel(bits_per_pixel));
        }
        if !matches!(scanline_pad, 8 | 16 | 32) {
            return Err(FormatError::Pad(scanline_pad));
        }
        Ok(Self {
            bpp: bits_per_pixel,
            pad: scanline_pad,
            msb_first,
            r: Channel::from_mask(red_mask)?,
            g: Channel::from_mask(green_mask)?,
            b: Channel::from_mask(blue_mask)?,
        })
    }

    /// The format of images from a drawable with `visual` and `depth`.
    pub fn for_visual(setup: &Setup, visual: u32, depth: u8) -> Result<Self, FormatError> {
        let v = setup
            .roots
            .iter()
            .flat_map(|s| &s.allowed_depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == visual)
            .ok_or(FormatError::NoVisual(visual))?;
        if !matches!(v.class, VisualClass::TRUE_COLOR | VisualClass::DIRECT_COLOR) {
            return Err(FormatError::NotTrueColor(visual));
        }
        let format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .ok_or(FormatError::NoPixmapFormat(depth))?;
        Self::new(
            v.red_mask,
            v.green_mask,
            v.blue_mask,
            format.bits_per_pixel,
            format.scanline_pad,
            setup.image_byte_order == ImageOrder::MSB_FIRST,
        )
    }

    /// The format of the root window of `screen` (screen mode, §5.2).
    pub fn root(setup: &Setup, screen: usize) -> Result<Self, FormatError> {
        let s = setup.roots.get(screen).ok_or(FormatError::NoVisual(0))?;
        Self::for_visual(setup, s.root_visual, s.root_depth)
    }

    /// Bytes per image row: `round_up(width × bpp, scanline_pad) / 8`.
    pub fn stride(&self, width: u16) -> usize {
        let pad = usize::from(self.pad);
        (usize::from(width) * usize::from(self.bpp)).div_ceil(pad) * pad / 8
    }

    /// Bytes in a `width × height` image.
    pub fn image_len(&self, width: u16, height: u16) -> usize {
        self.stride(width) * usize::from(height)
    }

    /// Decodes a `width × height` Z-pixmap image into `out`, row-major, as `0xRRGGBB`. Never
    /// allocates, so the hot loop can call it.
    pub fn decode(&self, data: &[u8], width: u16, height: u16, out: &mut [u32]) -> Result<(), FormatError> {
        let (w, h) = (usize::from(width), usize::from(height));
        let stride = self.stride(width);
        if data.len() < stride * h {
            return Err(FormatError::Short {
                got: data.len(),
                want: stride * h,
            });
        }
        if out.len() < w * h {
            return Err(FormatError::Output(w * h));
        }
        let bytes = usize::from(self.bpp / 8);
        for (y, row_out) in out.chunks_exact_mut(w).take(h).enumerate() {
            let row = &data[y * stride..y * stride + w * bytes];
            for (px, out) in row.chunks_exact(bytes).zip(row_out) {
                let p = self.read(px);
                *out = (self.r.to8(p) << 16) | (self.g.to8(p) << 8) | self.b.to8(p);
            }
        }
        Ok(())
    }

    #[inline]
    fn read(&self, px: &[u8]) -> u32 {
        match *px {
            [a, b, c, d] if self.msb_first => u32::from_be_bytes([a, b, c, d]),
            [a, b, c, d] => u32::from_le_bytes([a, b, c, d]),
            [a, b, c] if self.msb_first => u32::from_be_bytes([0, a, b, c]),
            [a, b, c] => u32::from_le_bytes([a, b, c, 0]),
            [a, b] if self.msb_first => u32::from(u16::from_be_bytes([a, b])),
            [a, b] => u32::from(u16::from_le_bytes([a, b])),
            _ => 0, // the constructor allows only 16, 24 and 32 bits per pixel
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB888: (u32, u32, u32) = (0xff_0000, 0x00_ff00, 0x00_00ff);

    fn fmt(masks: (u32, u32, u32), bpp: u8, msb: bool) -> PixelFormat {
        PixelFormat::new(masks.0, masks.1, masks.2, bpp, 32, msb).unwrap()
    }

    fn decode(f: PixelFormat, data: &[u8], w: u16, h: u16) -> Vec<u32> {
        let mut out = vec![0; usize::from(w) * usize::from(h)];
        f.decode(data, w, h, &mut out).unwrap();
        out
    }

    #[test]
    fn thirty_two_bits_in_both_byte_orders() {
        let lsb = fmt(RGB888, 32, false); // BGRX in memory, the usual case
        assert_eq!(
            decode(lsb, &[0x6F, 0xD1, 0x3B, 0x00, 0xff, 0, 0, 0], 2, 1),
            [0x3BD16F, 0x0000FF]
        );
        let msb = fmt(RGB888, 32, true);
        assert_eq!(decode(msb, &[0x00, 0x3B, 0xD1, 0x6F], 1, 1), [0x3BD16F]);
        let bgr = fmt((0x00_00ff, 0x00_ff00, 0xff_0000), 32, false); // red in the low byte
        assert_eq!(decode(bgr, &[0x3B, 0xD1, 0x6F, 0x00], 1, 1), [0x3BD16F]);
    }

    #[test]
    fn rows_are_padded_to_the_scanline() {
        // 24 bits per pixel, 3 pixels per row: 9 bytes of pixels padded to 12.
        let f = fmt(RGB888, 24, false);
        assert_eq!(f.stride(3), 12);
        let mut data = vec![0u8; 24];
        data[0..3].copy_from_slice(&[0x6F, 0xD1, 0x3B]);
        data[12 + 6..12 + 9].copy_from_slice(&[0x01, 0x02, 0x03]); // row 1, pixel 2
        let out = decode(f, &data, 3, 2);
        assert_eq!((out[0], out[5]), (0x3BD16F, 0x030201));
        assert_eq!(fmt(RGB888, 16, false).stride(3), 8);
        assert_eq!(
            PixelFormat::new(0xf800, 0x7e0, 0x1f, 16, 16, false).unwrap().stride(3),
            6
        );
    }

    #[test]
    fn sixteen_bit_565_expands_by_bit_replication() {
        let f = PixelFormat::new(0xf800, 0x07e0, 0x001f, 16, 16, false).unwrap();
        let px = |v: u16| decode(f, &v.to_le_bytes(), 1, 1)[0];
        assert_eq!(px(0xf800), 0xFF0000);
        assert_eq!(px(0x07e0), 0x00FF00);
        assert_eq!(px(0x001f), 0x0000FF);
        assert_eq!(px(0x0000), 0x000000);
        // red 0b10000 → 0b10000100, green 0b100000 → 0b10000010
        assert_eq!(px(0x8400), 0x848200); // red 0b10000, green 0b100000, blue 0
    }

    #[test]
    fn thirty_bit_color_keeps_the_top_eight_bits() {
        let f = fmt((0x3ff0_0000, 0x000f_fc00, 0x0000_03ff), 32, false);
        let p: u32 = (0x3ff << 20) | (0x200 << 10) | 0x003;
        assert_eq!(decode(f, &p.to_le_bytes(), 1, 1), [0xFF8000]);
    }

    #[test]
    fn unsupported_formats() {
        assert_eq!(
            PixelFormat::new(0xe0, 0x1c, 0x03, 8, 32, false),
            Err(FormatError::BitsPerPixel(8))
        );
        assert_eq!(
            PixelFormat::new(0xe0, 0x1c, 0x03, 16, 32, false),
            Err(FormatError::TooFewBits(3))
        );
        assert_eq!(
            PixelFormat::new(0xf0f0, 0xf0f, 0xf_0000, 32, 32, false),
            Err(FormatError::Mask(0xf0f0))
        );
        assert_eq!(
            PixelFormat::new(0, 0xff00, 0xff, 32, 32, false),
            Err(FormatError::Mask(0))
        );
        assert_eq!(
            PixelFormat::new(0xff_0000, 0xff00, 0xff, 32, 12, false),
            Err(FormatError::Pad(12))
        );
        let f = fmt(RGB888, 32, false);
        let mut out = [0u32; 4];
        assert_eq!(
            f.decode(&[0; 15], 2, 2, &mut out),
            Err(FormatError::Short { got: 15, want: 16 })
        );
        assert_eq!(f.decode(&[0; 16], 2, 2, &mut out[..3]), Err(FormatError::Output(4)));
    }
}
