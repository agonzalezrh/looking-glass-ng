use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::ImportMem;

/// A producer of GPU textures for the scene.
///
/// Each frame, the compositor calls `update()` on every registered producer.
/// If the producer has new pixel data, it imports it into the renderer and
/// replaces its internal texture. The compositor then picks up the new texture
/// via `texture()` and updates the corresponding Visual.
///
/// This abstraction is the bridge between external frame sources (Looking Glass
/// KVMFR, DMA-BUF, video decodes, etc.) and the existing Scene/Visual pipeline.
pub trait FrameProducer {
    /// Update the texture. Returns true if the frame changed.
    fn update(&mut self, renderer: &mut GlesRenderer) -> bool;
    /// Get the current texture
    fn texture(&self) -> &GlesTexture;
    /// Get the dimensions of the produced frames
    fn size(&self) -> (u32, u32);
}

/// An animated checkerboard that continuously cycles colors.
///
/// This demonstrates a producer that updates its texture every frame.
/// A real Looking Glass KVMFR producer would replace this.
pub struct AnimatedCheckerboard {
    texture: GlesTexture,
    width: u32,
    height: u32,
    frame_count: u64,
}

impl AnimatedCheckerboard {
    pub fn new(renderer: &mut GlesRenderer) -> Option<Self> {
        let w = 256u32;
        let h = 256u32;
        let pixels = Self::generate(w, h, 0);
        let tex = renderer.import_memory(&pixels, Fourcc::Abgr8888, (w as i32, h as i32).into(), false).ok()?;
        Some(AnimatedCheckerboard {
            texture: tex,
            width: w,
            height: h,
            frame_count: 0,
        })
    }

    fn generate(w: u32, h: u32, phase: u64) -> Vec<u8> {
        let shift = (phase % 24) as u8;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let bright = ((x / 32) + (y / 32) + (shift as u32)) % 2 == 0;
                if bright {
                    let r = 128u8.wrapping_add(shift.wrapping_mul(10));
                    let g = 200u8.wrapping_sub(shift.wrapping_mul(8));
                    let b = 255u8.wrapping_sub(shift.wrapping_mul(6));
                    pixels.extend_from_slice(&[r, g, b, 255]);
                } else {
                    pixels.extend_from_slice(&[20, 20, 40, 255]);
                }
            }
        }
        pixels
    }
}

impl FrameProducer for AnimatedCheckerboard {
    fn update(&mut self, renderer: &mut GlesRenderer) -> bool {
        self.frame_count += 1;
        // Only regenerate every 3 frames to show animation
        if self.frame_count % 3 != 0 {
            return false;
        }
        let pixels = Self::generate(self.width, self.height, self.frame_count / 3);
        if let Ok(tex) = renderer.import_memory(
            &pixels,
            Fourcc::Abgr8888,
            (self.width as i32, self.height as i32).into(),
            false,
        ) {
            self.texture = tex;
            true
        } else {
            false
        }
    }

    fn texture(&self) -> &GlesTexture {
        &self.texture
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
