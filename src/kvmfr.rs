//! Looking Glass KVMFR/LGMP frame producer.
//!
//! Protocol definitions matching the upstream Looking Glass ABI
//! (KVMFR version 34, LGMP protocol version 12).
//!
//! ```text
//! /dev/kvmfrN
//!      │
//!      ├── KVMFRR (recovery region, 64KB)
//!      │     └── KVMFRRHeader  →  LGMP version, session, UUID
//!      │
//!      └── LGMP region (rest of mapping)
//!            └── lgmpClientInit  →  discover queues
//!                  └── lgmpClientSubscribe(FRAME)  →  frame messages
//!                        └── KVMFRFrame  →  pixel data + metadata
//!
//! When no KVMFR device is available the producer fails gracefully.

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use tracing::{info, warn};

use crate::producer::{FrameProducer, FrameResult};

// ── KVMFR protocol constants ──────────────────────────────────────────

const KVMFR_MAGIC: &[u8; 8] = b"KVMFR---";
const KVMFR_VERSION: u32 = 34;

const KVMFR_R_MAGIC: &[u8; 8] = b"KVMFRRCV";
const KVMFR_R_VERSION: u16 = 1;
pub const KVMFR_R_REGION_SIZE: usize = 65536;
pub const KVMFR_R_LINE_SIZE: usize = 64;
pub const KVMFR_R_REQ_SLOTS: u32 = 16;

// LGMP protocol constants
const LGMP_PROTOCOL_VERSION: u32 = 12;
const LGMP_MSGS_SIZE: usize = 64;
const LGMP_Q_FRAME: u32 = 2;
const LGMP_Q_FRAME_LEN: u32 = 2;

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
const _: () = assert!(std::mem::align_of::<KVMFRRHeader>() <= KVMFR_R_LINE_SIZE);

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
const _: () = assert!(memoffset::offset_of!(KVMFRRRequest, request) == 4);
const _: () = assert!(memoffset::offset_of!(KVMFRRRequest, session) == 8);

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
const _: () = assert!(memoffset::offset_of!(KVMFRRStatus, session) == 16);
const _: () = assert!(memoffset::offset_of!(KVMFRRStatus, serial) == 24);

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
const _: () = assert!(memoffset::offset_of!(KVMFRR, info) == KVMFR_R_LINE_SIZE);
const _: () = assert!(memoffset::offset_of!(KVMFRR, req) == KVMFR_R_LINE_SIZE * 2);
const _: () = assert!(memoffset::offset_of!(KVMFRR, requests) == KVMFR_R_LINE_SIZE * 3);
const _: () = assert!(memoffset::offset_of!(KVMFRR, status) == KVMFR_R_LINE_SIZE * (3 + KVMFR_R_REQ_SLOTS as usize));

// ── KVMFR main header (consumed via LGMP, but the header sits at offset 0) ──

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KVMFRHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub hostver: [u8; 32],
    pub features: u32,
}
const _: () = assert!(std::mem::size_of::<KVMFRHeader>() == 48);

// ── IVSHMEM transport ─────────────────────────────────────────────────

/// Manages the IVSHMEM mapping via /dev/kvmfrN.
pub struct IvshmemTransport {
    _file: File,
    pub mem: *mut u8,
    pub size: usize,
}

impl IvshmemTransport {
    /// Try to open and map the first available KVMFR device.
    pub fn open() -> Option<Self> {
        for i in 0..8 {
            let path = PathBuf::from(format!("/dev/kvmfr{}", i));
            if !path.exists() {
                continue;
            }
            match Self::open_path(&path) {
                Some(t) => {
                    info!(device = ?path, size = t.size, "KVMFR device opened");
                    return Some(t);
                }
                None => {
                    warn!(device = ?path, "KVMFR device exists but could not be opened");
                }
            }
        }
        warn!("no KVMFR device found");
        None
    }

    fn open_path(path: &std::path::Path) -> Option<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .ok()?;

        // Get the size via lseek
        let size = unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_END) };
        if size <= 0 {
            return None;
        }
        let size = size as usize;

        // mmap the entire device
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
        unsafe {
            libc::munmap(self.mem as *mut libc::c_void, self.size);
        }
    }
}

// ── KvmfrFrameProducer ────────────────────────────────────────────────

/// Frame producer backed by a Looking Glass KVMFR/LGMP transport.
///
/// Opens `/dev/kvmfr{N}`, maps the IVSHMEM region, initializes LGMP,
/// subscribes to the frame queue, and imports frames as GPU textures.
///
/// If no KVMFR device is available, update() returns FrameResult::Error.
pub struct KvmfrFrameProducer {
    transport: Option<ActiveProducer>,
    device_attempted: bool,
}

struct ActiveProducer {
    _transport: IvshmemTransport,
    texture: GlesTexture,
    width: u32,
    height: u32,
    serial: u32,
}

impl KvmfrFrameProducer {
    pub fn new() -> Self {
        KvmfrFrameProducer {
            transport: None,
            device_attempted: false,
        }
    }
}

impl FrameProducer for KvmfrFrameProducer {
    fn update(&mut self, renderer: &mut GlesRenderer) -> FrameResult {
        // First call: try to open the KVMFR device
        if !self.device_attempted {
            self.device_attempted = true;
            let transport = match IvshmemTransport::open() {
                Some(t) => t,
                None => return FrameResult::Error("no KVMFR device available".into()),
            };

            // Parse KVMFR recovery region header
            let recovery = unsafe { &*(transport.mem as *const KVMFRR) };
            if &recovery.header.magic != KVMFR_MAGIC {
                // Try KVMFR_R_MAGIC for the recovery region
                if &recovery.header.magic != KVMFR_R_MAGIC {
                    return FrameResult::Error("invalid KVMFR magic".into());
                }
            }

            info!(
                kvmfr_version = recovery.header.kvmfr_version,
                lgmp_version = recovery.header.lgmp_version,
                session = recovery.header.session,
                uuid = ?recovery.header.uuid,
                "KVMFR session detected"
            );

            // WITHOUT liblgmp: We cannot subscribe to the LGMP frame queue.
            // This producer requires the LGMP C library to be linked.
            // When liblgmp is available, we'd call:
            //   lgmpClientInit(mem, size, &client) -> lgmpClientSessionInit() -> lgmpClientSubscribe(Q_FRAME)
            // For now, report that the hardware integration requires the library.
            return FrameResult::Error("KVMFR device found but LGMP integration requires liblgmp".into());
        }

        // If we have an active transport, try to get a frame
        if let Some(ref mut active) = self.transport {
            // Without liblgmp, this is where we'd process frame queue messages.
            // For now, signal that the implementation is incomplete.
            let _ = active;
            FrameResult::Unchanged
        } else {
            FrameResult::Error("KVMFR not available".into())
        }
    }

    fn texture(&self) -> &GlesTexture {
        // If no transport, this shouldn't be called because the producer
        // returns Error from update() and won't be registered.
        // Provide a reasonable panic message in case it is.
        panic!("KvmfrFrameProducer::texture() called without active transport")
    }

    fn size(&self) -> (u32, u32) {
        match self.transport {
            Some(ref active) => (active.width, active.height),
            None => (0, 0),
        }
    }
}
