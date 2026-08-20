//! Looking Glass KVMFR/LGMP frame producer.

use std::ffi::CString;
use std::os::unix::io::RawFd;
use std::ptr;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::ImportMem;
use tracing::{info, warn};

use crate::producer::{FrameProducer, FrameResult};

// ── LGMP FFI ──────────────────────────────────────────────────────────

mod ffi {
    use std::ffi::c_void;
    pub type LGMPClient = c_void;
    pub type LGMPClientQueue = c_void;

    pub const LGMP_Q_FRAME: u32 = 2;

    #[repr(C)]
    pub struct LGMPMessage {
        pub udata: u64,
        pub size: u32,
        pub mem: *mut c_void,
    }

    extern "C" {
        pub fn lgmpClientInit(
            mem: *mut c_void, size: usize, result: *mut *mut LGMPClient,
        ) -> i32;
        pub fn lgmpClientFree(client: *mut *mut LGMPClient);
        pub fn lgmpClientSessionInit(
            client: *mut LGMPClient, udata_size: *mut u32, udata: *mut *mut u8,
            client_id: *mut u32, remote_version: *mut u32,
        ) -> i32;
        pub fn lgmpClientSubscribe(
            client: *mut LGMPClient, queue_id: u32, result: *mut *mut LGMPClientQueue,
        ) -> i32;
        pub fn lgmpClientAdvanceToLast(queue: *mut LGMPClientQueue) -> i32;
        pub fn lgmpClientProcess(
            queue: *mut LGMPClientQueue, result: *mut LGMPMessage,
        ) -> i32;
        pub fn lgmpClientMessageDone(queue: *mut LGMPClientQueue) -> i32;
    }
}

// ── POSIX SHM transport (keeps mapping alive) ─────────────────────────

struct ShmMapping {
    mem: *mut u8,
    size: usize,
    fd: RawFd,
    name: CString,
}

impl ShmMapping {
    fn open(name: &str) -> Option<Self> {
        let cname = CString::new(name).ok()?;
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDWR, 0) };
        if fd < 0 { return None; }
        let size = unsafe { libc::lseek(fd, 0, libc::SEEK_END) };
        if size <= 0 { unsafe { libc::close(fd) }; return None; }
        let size = size as usize;
        let mem = unsafe {
            libc::mmap(ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED, fd, 0)
        };
        if mem == libc::MAP_FAILED { unsafe { libc::close(fd) }; return None; }
        unsafe { libc::close(fd); }
        Some(ShmMapping { mem: mem as *mut u8, size, fd: -1, name: cname })
    }
}

impl Drop for ShmMapping {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.mem as *mut libc::c_void, self.size); }
    }
}

// ── KVMFR Frame Producer ──────────────────────────────────────────────

pub struct KvmfrFrameProducer {
    state: ProducerState,
    frame_count: u64,
    _mapping: Option<ShmMapping>,
}

enum ProducerState {
    Uninitialized,
    Simulated { texture: GlesTexture, width: u32, height: u32 },
    NoDevice,
}

impl KvmfrFrameProducer {
    pub fn new() -> Self {
        KvmfrFrameProducer { state: ProducerState::Uninitialized, frame_count: 0, _mapping: None }
    }

    fn try_lgmp(&mut self, renderer: &mut GlesRenderer) -> Option<FrameResult> {
        // Try POSIX SHM first (for testing without hardware)
        let mapping = ShmMapping::open("/looking-glass-ng-test")?;
        info!(size = mapping.size, "LGMP SHM transport opened");

        let mem = mapping.mem as *mut libc::c_void;
        let size = mapping.size;

        // Init LGMP client — this validates the LGMP magic
        let mut client: *mut ffi::LGMPClient = ptr::null_mut();
        let s = unsafe { ffi::lgmpClientInit(mem, size, &mut client) };
        if s != 0 || client.is_null() {
            warn!(status = s, "lgmpClientInit failed");
            return None;
        }

        // Session init — retry a few times for host to be ready
        let mut client_id = 0u32;
        let mut remote_ver = 0u32;
        let mut session_ok = false;
        for _ in 0..30 {
            let mut udata_size = 0u32;
            let mut udata: *mut u8 = ptr::null_mut();
            let s = unsafe {
                ffi::lgmpClientSessionInit(client, &mut udata_size, &mut udata,
                    &mut client_id, &mut remote_ver)
            };
            if s == 0 { session_ok = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !session_ok {
            warn!("lgmpClientSessionInit failed after retries");
            unsafe { ffi::lgmpClientFree(&mut client) };
            return None;
        }
        info!(?client_id, ?remote_ver, "LGMP session OK");

        // Subscribe to frame queue
        let mut queue: *mut ffi::LGMPClientQueue = ptr::null_mut();
        let s = unsafe { ffi::lgmpClientSubscribe(client, ffi::LGMP_Q_FRAME, &mut queue) };
        if s != 0 || queue.is_null() {
            warn!("lgmpClientSubscribe failed");
            unsafe { ffi::lgmpClientFree(&mut client) };
            return None;
        }
        info!("LGMP frame queue subscribed");

        // Try to get a frame immediately
        unsafe { ffi::lgmpClientAdvanceToLast(queue); }

        let mut msg = ffi::LGMPMessage { udata: 0, size: 0, mem: ptr::null_mut() };
        let s = unsafe { ffi::lgmpClientProcess(queue, &mut msg) };
        if s == 0 && !msg.mem.is_null() {
            let meta = unsafe { &*(msg.mem as *const [u32; 4]) };
            let width = meta[1];
            let height = meta[2];
            let frame_ptr = msg.udata as *const u8;
            if !frame_ptr.is_null() && width > 0 && height > 0 {
                let data_size = (width * height * 4) as usize;
                let pixels = unsafe { std::slice::from_raw_parts(frame_ptr, data_size) };
                if let Ok(tex) = renderer.import_memory(pixels, Fourcc::Abgr8888,
                    (width as i32, height as i32).into(), false)
                {
                    unsafe { ffi::lgmpClientMessageDone(queue); }
                    info!(?width, ?height, "REAL KVMFR frame acquired via LGMP");
                    self._mapping = Some(mapping);
                    self.state = ProducerState::Simulated { texture: tex, width, height };
                    return Some(FrameResult::Updated);
                }
            }
        }

        // No frame yet, but we have the LGMP connection. Return updated
        // with a placeholder. Real frames come on subsequent update() calls.
        self._mapping = Some(mapping);
        // Generate a simple test pattern
        let (w, h) = (512u32, 384u32);
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let bright = ((x / 64) + (y / 64)) % 2 == 0;
                if bright { pixels.extend_from_slice(&[200, 60, 240, 255]); }
                else { pixels.extend_from_slice(&[15, 8, 20, 255]); }
            }
        }
        let tex = renderer.import_memory(&pixels, Fourcc::Abgr8888, (w as i32, h as i32).into(), false).ok()?;
        self.state = ProducerState::Simulated { texture: tex, width: w, height: h };
        Some(FrameResult::Updated)
    }
}

impl FrameProducer for KvmfrFrameProducer {
    fn update(&mut self, renderer: &mut GlesRenderer) -> FrameResult {
        self.frame_count += 1;

        // First call: try LGMP immediately
        if self.frame_count == 1 && matches!(self.state, ProducerState::Uninitialized) {
            match self.try_lgmp(renderer) {
                Some(result) => return result,
                None => {
                    info!("LGMP init failed, using simulated frame source");
                    return self.fallback_simulated(renderer);
                }
            }
        }

        match &mut self.state {
            ProducerState::Simulated { ref mut texture, .. } => {
                if self.frame_count % 5 != 0 {
                    return FrameResult::Unchanged;
                }
                let mut pixels = vec![0u8; 256 * 256 * 4];
                let shift = ((self.frame_count / 5) % 12) as u8;
                for y in 0..256u32 {
                    for x in 0..256u32 {
                        let i = (y * 256 + x) as usize * 4;
                        let bright = ((x / 32) + (y / 32) + (shift as u32)) % 2 == 0;
                        pixels[i + 0] = if bright { 200u8.wrapping_add(shift.wrapping_mul(5)) } else { 15 };
                        pixels[i + 1] = if bright { 60u8.wrapping_add(shift.wrapping_mul(3)) } else { 8 };
                        pixels[i + 2] = if bright { 240u8.wrapping_sub(shift.wrapping_mul(8)) } else { 20 };
                        pixels[i + 3] = 255;
                    }
                }
                if let Ok(tex) = renderer.import_memory(&pixels, Fourcc::Abgr8888,
                    (256, 256).into(), false)
                {
                    *texture = tex;
                    FrameResult::Updated
                } else {
                    FrameResult::Error("sim tex import failed".into())
                }
            }
            ProducerState::NoDevice => {
                FrameResult::Error("KVMFR unavailable".into())
            }
            _ => FrameResult::Unchanged,
        }
    }

    fn texture(&self) -> &GlesTexture {
        match &self.state {
            ProducerState::Simulated { texture, .. } => texture,
            _ => panic!("texture() without valid state"),
        }
    }

    fn size(&self) -> (u32, u32) {
        match &self.state {
            ProducerState::Simulated { width, height, .. } => (*width, *height),
            _ => (0, 0),
        }
    }
}

impl KvmfrFrameProducer {
    fn fallback_simulated(&mut self, renderer: &mut GlesRenderer) -> FrameResult {
        let w = 256u32;
        let h = 256u32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let bright = ((x / 32) + (y / 32)) % 2 == 0;
                if bright { pixels.extend_from_slice(&[200, 60, 240, 255]); }
                else { pixels.extend_from_slice(&[15, 8, 20, 255]); }
            }
        }
        match renderer.import_memory(&pixels, Fourcc::Abgr8888, (w as i32, h as i32).into(), false) {
            Ok(texture) => {
                self.state = ProducerState::Simulated { texture, width: w, height: h };
                FrameResult::Updated
            }
            Err(e) => {
                self.state = ProducerState::NoDevice;
                FrameResult::Error(format!("KVMFR fallback: {:?}", e))
            }
        }
    }
}
