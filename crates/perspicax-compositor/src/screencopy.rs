//! `wlr-screencopy-unstable-v1`: a monitor's pixels, for a screenshot or
//! screen-recording tool.
//!
//! What grim, wf-recorder and xdg-desktop-portal-wlr speak. A frame is asked
//! for, told the size and format of buffer to bring, and filled with the
//! monitor as it looks: drawn on request with the same renderer as an
//! agent's picture (`crate::capture`), but with nothing painted over. The
//! person's own tool sees the person's screen; what an agent may see is a
//! different question, governed by consent, and is not asked here.
//!
//! Shared memory only: no dmabuf, so a client that can take nothing else
//! gets nothing. The pointer is not drawn into the picture, whatever the
//! client asks. `copy_with_damage` waits for the next commit and reports the
//! whole frame as damaged: the truth, if not the whole of it.
//!
//! Who may use it is `[protocols] screencopy`. While the session is locked,
//! every copy fails.

use std::sync::Mutex;

use perspicax_policy::Protocol;
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_buffer::WlBuffer, wl_shm},
        },
    },
    utils::{Clock, Monotonic, Physical, Rectangle},
    wayland::shm::{BufferData, with_buffer_contents, with_buffer_contents_mut},
};

use crate::{
    access::{Filtered, Gate},
    state::Compositor,
};

/// The version this compositor speaks: 3 adds `buffer_done`.
const VERSION: u32 = 3;

/// The formats a frame may be copied into: the two every client knows,
/// four bytes a pixel.
const FORMATS: [wl_shm::Format; 2] = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888];

/// What a frame is of, and whether it has been used.
#[derive(Debug)]
pub(crate) struct Frame {
    output: String,
    /// The part of the monitor, in its pixels.
    region: Rectangle<i32, Physical>,
    copied: Mutex<bool>,
}

/// Frames waiting for the next commit before they are copied.
#[derive(Default)]
pub(crate) struct Screencopy {
    pub(crate) waiting: Vec<(ZwlrScreencopyFrameV1, WlBuffer)>,
}

impl Screencopy {
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, ZwlrScreencopyManagerV1, _>(
            VERSION,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::Screencopy,
            },
        );
        Self::default()
    }
}

impl Compositor {
    /// Copy every frame that was waiting for something to change. Called
    /// once something has.
    pub(crate) fn flush_screencopy(&mut self) {
        for (frame, buffer) in std::mem::take(&mut self.screencopy.waiting) {
            if frame.is_alive() {
                self.copy_frame(&frame, &buffer, true);
            }
        }
    }

    /// Fail every waiting frame of a client the rules no longer admit.
    pub(crate) fn revoke_screencopy(&mut self) {
        let gate = self.gate.clone();
        let display = self.display.clone();
        self.screencopy.waiting.retain(|(frame, _)| {
            let admitted = display
                .get_client(frame.id())
                .is_ok_and(|client| gate.admits(Protocol::Screencopy, &client));
            if !admitted {
                frame.failed();
            }
            admitted
        });
    }

    /// Fill `buffer` with the frame's part of its monitor, and say so.
    fn copy_frame(&mut self, frame: &ZwlrScreencopyFrameV1, buffer: &WlBuffer, damage: bool) {
        let Some(data) = frame.data::<Frame>() else {
            return;
        };
        let admitted = self
            .display
            .get_client(frame.id())
            .is_ok_and(|client| self.gate.admits(Protocol::Screencopy, &client));
        if self.lock.is_some() || !admitted {
            frame.failed();
            return;
        }
        let pixels = match crate::capture::draw_output(self, &data.output) {
            Ok(pixels) => pixels,
            Err(error) => {
                tracing::warn!(%error, "screencopy failed");
                frame.failed();
                return;
            }
        };
        if let Err(error) = write(buffer, &pixels, data.region) {
            tracing::warn!(%error, "screencopy could not write the client's buffer");
            frame.failed();
            return;
        }
        frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if damage {
            let size = data.region.size;
            frame.damage(0, 0, size.w.unsigned_abs(), size.h.unsigned_abs());
        }
        let now = std::time::Duration::from(Clock::<Monotonic>::new().now());
        let seconds = now.as_secs();
        frame.ready(
            u32::try_from(seconds >> 32).unwrap_or(0),
            u32::try_from(seconds & u64::from(u32::MAX)).unwrap_or(0),
            now.subsec_nanos(),
        );
    }
}

/// Whether `buffer` is shared memory of the size, stride and a format the
/// frame said to bring.
fn fits(buffer: &WlBuffer, region: Rectangle<i32, Physical>) -> bool {
    with_buffer_contents(buffer, |_, _, data: BufferData| {
        FORMATS.contains(&data.format)
            && data.width == region.size.w
            && data.height == region.size.h
            && data.stride >= region.size.w * 4
    })
    .unwrap_or(false)
}

/// Copy `region` of a picture into a client's shared-memory buffer, RGBA to
/// the little-endian BGRA both formats are.
fn write(
    buffer: &WlBuffer,
    pixels: &crate::capture::Pixels,
    region: Rectangle<i32, Physical>,
) -> Result<(), String> {
    let width = usize::try_from(pixels.width).map_err(|error| error.to_string())?;
    let (x0, y0) = (
        usize::try_from(region.loc.x).map_err(|error| error.to_string())?,
        usize::try_from(region.loc.y).map_err(|error| error.to_string())?,
    );
    let (w, h) = (
        usize::try_from(region.size.w).map_err(|error| error.to_string())?,
        usize::try_from(region.size.h).map_err(|error| error.to_string())?,
    );
    if x0 + w > width || (y0 + h) * width * 4 > pixels.rgba.len() {
        return Err("the region is outside the picture".to_owned());
    }
    let mut rows = Vec::with_capacity(w * h * 4);
    for row in pixels.rgba.chunks_exact(width * 4).skip(y0).take(h) {
        for pixel in row[x0 * 4..(x0 + w) * 4].chunks_exact(4) {
            rows.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    with_buffer_contents_mut(buffer, |base, length, data| {
        let offset = usize::try_from(data.offset).map_err(|error| error.to_string())?;
        let stride = usize::try_from(data.stride).map_err(|error| error.to_string())?;
        if offset + stride * h > length {
            return Err("the buffer is smaller than it says".to_owned());
        }
        for (at, row) in rows.chunks_exact(w * 4).enumerate() {
            let start = offset + at * stride;
            // The client's memory, which it may change under us at any time:
            // so a byte copy through the pointer smithay hands out, within the
            // length it reports, and never a reference into it.
            #[allow(
                unsafe_code,
                reason = "shared memory a client owns; bounds checked against the pool's length above"
            )]
            unsafe {
                std::ptr::copy_nonoverlapping(row.as_ptr(), base.add(start), row.len());
            }
        }
        Ok(())
    })
    .map_err(|error| error.to_string())?
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, Filtered> for Compositor {
    fn bind(
        _state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        let (frame, output, logical) = match request {
            Request::CaptureOutput { frame, output, .. } => (frame, output, None),
            Request::CaptureOutputRegion {
                frame,
                output,
                x,
                y,
                width,
                height,
                ..
            } => (
                frame,
                output,
                Some(Rectangle::<i32, smithay::utils::Logical>::new(
                    (x, y).into(),
                    (width, height).into(),
                )),
            ),
            _ => return,
        };
        let output = Output::from_resource(&output);
        let size = output
            .as_ref()
            .and_then(Output::current_mode)
            .map(|mode| mode.size);
        let (Some(output), Some(size)) = (output, size) else {
            let frame = data_init.init(
                frame,
                Frame {
                    output: String::new(),
                    region: Rectangle::default(),
                    copied: Mutex::new(true),
                },
            );
            frame.failed();
            return;
        };
        let whole = Rectangle::from_size(size);
        let scale = output.current_scale().fractional_scale();
        let region = logical
            .map_or(Some(whole), |region| {
                region.to_physical_precise_round(scale).intersection(whole)
            })
            .filter(|region| !region.is_empty());
        let frame = data_init.init(
            frame,
            Frame {
                output: output.name(),
                region: region.unwrap_or_default(),
                copied: Mutex::new(region.is_none()),
            },
        );
        let Some(region) = region else {
            frame.failed();
            return;
        };
        let (width, height) = (region.size.w.unsigned_abs(), region.size.h.unsigned_abs());
        for format in FORMATS {
            frame.buffer(format, width, height, width * 4);
        }
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, Frame> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &Frame,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_frame_v1::Request;
        let (buffer, damage) = match request {
            Request::Copy { buffer } => (buffer, false),
            Request::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        {
            let mut copied = data
                .copied
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *copied {
                frame.post_error(
                    zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                    "this frame has already been copied",
                );
                return;
            }
            *copied = true;
        }
        if !fits(&buffer, data.region) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "the buffer is not the shared memory, size or format this frame described",
            );
            return;
        }
        if damage {
            state.screencopy.waiting.push((frame.clone(), buffer));
        } else {
            state.copy_frame(frame, &buffer, false);
        }
    }
}
