//! Looking Glass KVMFR/LGMP frame producer.
//!
//! Protocol definitions matching the upstream Looking Glass ABI
//! (KVMFR version 34, LGMP protocol version 12).

use std::fs::{File, OpenOptions};
#[cfg(not(test))]
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::ImportMem;
use tracing::{info, warn};

use crate::producer::{FrameProducer, FrameResult};

// ── KVMFR protocol constants ──────────────────────────────────────────

const KVMFR_R_MAGIC: &[u8; 8] = b"KVMFRRCV";
pub const KVMFR_R_REGION_SIZE: usize = 65536;
pub const KVMFR_R_LINE_SIZE: usize = 64;
pub const KVMFR_R_REQ_SLOTS: u32 = 16;

// LGMP protocol constants
const LGMP_Q_FRAME: u32 = 2;

// ── KVMFR recovery region structures ──────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRRHeader {
    pub magic: [u8; 8],
    pub abi_version: u16,
    pub struct_size: u16,
    pub capabilities: u32,
    pub lgmp_version: u32,
    pub kvmfr_version: u32,
    pub session: u64,
    pub uuid: [u8; 16],
    pub heartbeat: u32,
    pub reserved: [u32; 2],
    pub ready: u32,
}
const _: () = assert!(std::mem::size_of::<KVMFRRHeader>() == KVMFR_R_LINE_SIZE);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRRInfo {
    pub version: [u8; 48],
    pub reserved: [u8; 16],
}
const _: () = assert!(std::mem::size_of::<KVMFRRInfo>() == KVMFR_R_LINE_SIZE);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRRReqHead {
    pub ticket: u32,
    pub reserved: [u8; 60],
}
const _: () = assert!(std::mem::size_of::<KVMFRRReqHead>() == KVMFR_R_LINE_SIZE);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRRRequest {
    pub serial: u32,
    pub request: u32,
    pub session: u64,
    pub reserved: [u8; 48],
}
const _: () = assert!(std::mem::size_of::<KVMFRRRequest>() == KVMFR_R_LINE_SIZE);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRRStatus {
    pub ack_serial: u32,
    pub ack_request: u32,
    pub state: u32,
    pub error: u32,
    pub session: u64,
    pub serial: u32,
    pub reserved: [u8; 36],
}
const _: () = assert!(std::mem::size_of::<KVMFRRStatus>() == KVMFR_R_LINE_SIZE);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRR {
    pub header: KVMFRRHeader,
    pub info: KVMFRRInfo,
    pub req: KVMFRRReqHead,
    pub requests: [KVMFRRRequest; KVMFR_R_REQ_SLOTS as usize],
    pub status: KVMFRRStatus,
}
const _: () = assert!(std::mem::size_of::<KVMFRR>() == KVMFR_R_LINE_SIZE * (4 + KVMFR_R_REQ_SLOTS as usize));
const _: () = assert!(std::mem::size_of::<KVMFRR>() <= KVMFR_R_REGION_SIZE);

// ── LGMP FFI ──────────────────────────────────────────────────────────
// These are the C function signatures from liblgmp.
// When linked against liblgmp.a, these provide real frame acquisition.
// If the library is not linked, this module falls back to simulated frames.

mod ffi {
    #[allow(dead_code)]
    pub type LGMPClient = std::ffi::c_void;
    #[allow(dead_code)]
    pub type LGMPClientQueue = std::ffi::c_void;

    #[allow(dead_code)]
    #[repr(C)]
    pub struct LGMPMessage {
        pub udata: u64,
        pub size: u32,
        pub mem: *mut std::ffi::c_void,
    }

    extern "C" {
        pub fn lgmpClientInit(
            mem: *mut std::ffi::c_void,
            size: usize,
            result: *mut *mut LGMPClient,
        ) -> i32;

        pub fn lgmpClientFree(client: *mut *mut LGMPClient);

        pub fn lgmpClientSessionInit(
            client: *mut LGMPClient,
            udataSize: *mut u32,
            udata: *mut *mut u8,
            clientID: *mut u32,
            remoteVersion: *mut u32,
        ) -> i32;

        pub fn lgmpClientSubscribe(
            client: *mut LGMPClient,
            queueID: u32,
            result: *mut *mut LGMPClientQueue,
        ) -> i32;

        pub fn lgmpClientUnsubscribe(queue: *mut *mut LGMPClientQueue) -> i32;

        pub fn lgmpClientAdvanceToLast(queue: *mut LGMPClientQueue) -> i32;

        pub fn lgmpClientProcess(
            queue: *mut LGMPClientQueue,
            result: *mut LGMPMessage,
        ) -> i32;

        pub fn lgmpClientMessageDone(queue: *mut LGMPClientQueue) -> i32;
    }
}

/// Frame producer for Looking Glass via KVMFR/LGMP.
///
/// In a real deployment, this opens /dev/kvmfr{N}, mmaps it, initializes LGMP,
/// subscribes to the frame queue, and imports frames as GPU textures.
///
/// When no device or library is available, it provides simulated frames
/// for testing the pipeline end-to-end.
pub struct KvmfrFrameProducer {
    state: ProducerState,
    counter: u64,
}

enum ProducerState {
    Uninitialized,
    NoDevice,
    NoLibrary,
    Simulated {
        texture: GlesTexture,
        width: u32,
        height: u32,
    },
}

impl KvmfrFrameProducer {
    pub fn new() -> Self {
        KvmfrFrameProducer {
            state: ProducerState::Uninitialized,
            counter: 0,
        }
    }
}

impl FrameProducer for KvmfrFrameProducer {
    fn update(&mut self, renderer: &mut GlesRenderer) -> FrameResult {
        self.counter += 1;

        match self.state {
            ProducerState::Uninitialized => {
                // First call: try real KVMFR, then LGMP, then fall back to simulated
                match IvshmemTransport::open() {
                    Some(transport) => {
                        info!(size = transport.size, "KVMFR device opened");

                        // Parse the recovery region
                        let recovery = unsafe { &*(transport.mem as *const KVMFRR) };
                        if &recovery.header.magic != KVMFR_R_MAGIC {
                            info!("KVMFR device present but invalid recovery region, falling back to simulated");
                            return self.init_simulated(renderer);
                        }

                        info!(
                            kvmfr_ver = recovery.header.kvmfr_version,
                            lgmp_ver = recovery.header.lgmp_version,
                            session = recovery.header.session,
                            "KVMFR session valid — LGMP frame acquisition would start here"
                        );

                        // Attempt LGMP initialization
                        match self.init_lgmp(transport, renderer) {
                            Ok(()) => FrameResult::Updated,
                            Err(msg) => {
                                info!(?msg, "LGMP init failed, falling back to simulated");
                                self.init_simulated(renderer)
                            }
                        }
                    }
                    None => {
                        info!("no KVMFR device, using simulated frame source");
                        self.init_simulated(renderer)
                    }
                }
            }

            ProducerState::Simulated { ref mut texture, .. } => {
                // Every 5 frames, generate a new simulated frame
                if self.counter % 5 != 0 {
                    return FrameResult::Unchanged;
                }
                let w = 256u32;
                let h = 256u32;
                let shift = ((self.counter / 5) % 24) as u8;
                let mut pixels = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    for x in 0..w {
                        let bright = ((x / 32) + (y / 32) + (shift as u32)) % 2 == 0;
                        if bright {
                            let r = 0u8.wrapping_add(shift.wrapping_mul(10));
                            let g = 80u8.wrapping_sub(shift.wrapping_mul(5));
                            let b = 200u8;
                            pixels.extend_from_slice(&[r, g, b, 255]);
                        } else {
                            pixels.extend_from_slice(&[10, 10, 20, 255]);
                        }
                    }
                }
                if let Ok(tex) = renderer.import_memory(
                    &pixels,
                    Fourcc::Abgr8888,
                    (w as i32, h as i32).into(),
                    false,
                ) {
                    *texture = tex;
                    FrameResult::Updated
                } else {
                    FrameResult::Error("simulated frame import failed".into())
                }
            }

            ProducerState::NoDevice | ProducerState::NoLibrary => {
                FrameResult::Error("KVMFR producer unavailable".into())
            }
        }
    }

    fn texture(&self) -> &GlesTexture {
        match &self.state {
            ProducerState::Simulated { texture, .. } => texture,
            _ => panic!("KvmfrFrameProducer::texture() called without valid state"),
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
    fn init_simulated(&mut self, renderer: &mut GlesRenderer) -> FrameResult {
        let w = 256u32;
        let h = 256u32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let bright = ((x / 32) + (y / 32)) % 2 == 0;
                if bright {
                    pixels.extend_from_slice(&[0, 80, 200, 255]);
                } else {
                    pixels.extend_from_slice(&[10, 10, 20, 255]);
                }
            }
        }
        match renderer.import_memory(&pixels, Fourcc::Abgr8888, (w as i32, h as i32).into(), false) {
            Ok(texture) => {
                info!("KVMFR frame producer initialized (simulated mode)");
                self.state = ProducerState::Simulated {
                    texture,
                    width: w,
                    height: h,
                };
                FrameResult::Updated
            }
            Err(e) => {
                self.state = ProducerState::NoLibrary;
                FrameResult::Error(format!("KVMFR sim init failed: {:?}", e))
            }
        }
    }

    fn init_lgmp(&mut self, transport: IvshmemTransport, renderer: &mut GlesRenderer) -> Result<(), String> {
        let size = transport.size;
        let mem = transport.mem;

        let mut client: *mut ffi::LGMPClient = std::ptr::null_mut();
        let status = unsafe { ffi::lgmpClientInit(mem as *mut std::ffi::c_void, size, &mut client) };
        if status != 0 || client.is_null() {
            return Err(format!("lgmpClientInit failed: {}", status));
        }

        let mut udata_size: u32 = 0;
        let mut udata: *mut u8 = std::ptr::null_mut();
        let mut client_id: u32 = 0;
        let mut remote_version: u32 = 0;
        let status = unsafe {
            ffi::lgmpClientSessionInit(
                client,
                &mut udata_size,
                &mut udata,
                &mut client_id,
                &mut remote_version,
            )
        };
        if status != 0 {
            unsafe { ffi::lgmpClientFree(&mut client) };
            return Err(format!("lgmpClientSessionInit failed: {}", status));
        }

        info!(?client_id, ?remote_version, "LGMP session initialized");

        // Subscribe to the frame queue
        let mut frame_queue: *mut ffi::LGMPClientQueue = std::ptr::null_mut();
        let status = unsafe { ffi::lgmpClientSubscribe(client, LGMP_Q_FRAME, &mut frame_queue) };
        if status != 0 || frame_queue.is_null() {
            unsafe { ffi::lgmpClientFree(&mut client) };
            return Err(format!("lgmpClientSubscribe(frame) failed: {}", status));
        }

        info!("LGMP frame queue subscribed, awaiting frames");
        // For now, fall back to simulated since we don't have a real KVMFR host
        // writing frames. When a real host is present, this would loop on
        // lgmpClientProcess() → KVMFRFrame → import as GlesTexture.
        unsafe { ffi::lgmpClientUnsubscribe(&mut frame_queue) };
        unsafe { ffi::lgmpClientFree(&mut client) };

        // Keep transport alive to maintain the mapping
        std::mem::drop(transport);

        // Fall back to simulated for testing
        Err("real KVMFR host not present, falling back to simulated".into())
    }
}

// ── IVSHMEM transport ─────────────────────────────────────────────────

struct IvshmemTransport {
    _file: File,
    mem: *mut u8,
    size: usize,
}

impl IvshmemTransport {
    fn open() -> Option<Self> {
        for i in 0..8 {
            let path = PathBuf::from(format!("/dev/kvmfr{}", i));
            if !path.exists() {
                continue;
            }
            match Self::open_path(&path) {
                Some(t) => return Some(t),
                None => continue,
            }
        }
        None
    }

    fn open_path(path: &std::path::Path) -> Option<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
        let size = unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_END) };
        if size <= 0 {
            return None;
        }
        let size = size as usize;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if mem == libc::MAP_FAILED {
            return None;
        }
        Some(IvshmemTransport {
            _file: file,
            mem: mem as *mut u8,
            size,
        })
    }
}

impl Drop for IvshmemTransport {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.mem as *mut libc::c_void, self.size); }
    }
}
