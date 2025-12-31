use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::state::ProjectWC;
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};
use smithay::utils::{Physical, Size};
use smithay::wayland::shm;
const SCREENCOPY_VERSION: u32 = 3;

#[derive(Default)]
pub struct ScreencopyManagerState;

pub struct ScreencopyManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl ScreencopyManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>,
        D: Dispatch<ZwlrScreencopyManagerV1, ()>,
        D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
        D: ScreencopyHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = ScreencopyManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ZwlrScreencopyManagerV1, _>(SCREENCOPY_VERSION, global_data);

        Self
    }
}

impl<D> GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData, D>
    for ScreencopyManagerState
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>,
    D: Dispatch<ZwlrScreencopyManagerV1, ()>,
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
    D: ScreencopyHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<ZwlrScreencopyManagerV1>,
        _manager_state: &ScreencopyManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }

    fn can_view(client: Client, global_data: &ScreencopyManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwlrScreencopyManagerV1, (), D> for ScreencopyManagerState
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>,
    D: Dispatch<ZwlrScreencopyManagerV1, ()>,
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
    D: ScreencopyHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (frame, output) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, output, .. } => {
                let Some(output) = Output::from_resource(&output) else {
                    tracing::trace!("screencopy: client requested non-existent output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };
                (frame, output)
            }
            // TODO: implement region capture (currently captures full output)
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion { frame, output, .. } => {
                let Some(output) = Output::from_resource(&output) else {
                    tracing::trace!("screencopy: client requested non-existent output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };
                (frame, output)
            }
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => unreachable!(),
        };

        let Some(mode) = output.current_mode() else {
            tracing::trace!("screencopy: output has no current mode");
            let frame = data_init.init(frame, ScreencopyFrameState::Failed);
            frame.failed();
            return;
        };

        let buffer_size = mode.size;
        let info = ScreencopyFrameInfo {
            output,
            buffer_size,
        };
        let frame = data_init.init(
            frame,
            ScreencopyFrameState::Pending {
                info,
                copied: Arc::new(AtomicBool::new(false)),
            },
        );

        frame.buffer(
            Format::Xrgb8888,
            buffer_size.w as u32,
            buffer_size.h as u32,
            buffer_size.w as u32 * 4,
        );

        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

#[derive(Clone)]
pub struct ScreencopyFrameInfo {
    output: Output,
    buffer_size: Size<i32, Physical>,
}

pub enum ScreencopyFrameState {
    Failed,
    Pending {
        info: ScreencopyFrameInfo,
        copied: Arc<AtomicBool>,
    },
}

impl<D> Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState, D> for ScreencopyManagerState
where
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>,
    D: ScreencopyHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameState,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        if matches!(request, zwlr_screencopy_frame_v1::Request::Destroy) {
            return;
        }

        let ScreencopyFrameState::Pending { info, copied } = data else {
            return;
        };

        if copied.load(Ordering::SeqCst) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "copy was already requested",
            );
            return;
        }

        let buffer = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => buffer,
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => buffer,
            _ => unreachable!(),
        };

        let size = info.buffer_size;

        let valid = shm::with_buffer_contents(&buffer, |_, shm_len, buffer_data| {
            buffer_data.format == Format::Xrgb8888
                && buffer_data.width == size.w
                && buffer_data.height == size.h
                && buffer_data.stride == size.w * 4
                && shm_len == buffer_data.stride as usize * buffer_data.height as usize
        })
        .unwrap_or(false);

        if !valid {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "invalid buffer",
            );
            return;
        }

        copied.store(true, Ordering::SeqCst);

        state.frame(Screencopy {
            buffer,
            frame: frame.clone(),
            info: info.clone(),
            submitted: false,
        });
    }
}

pub struct Screencopy {
    pub buffer: WlBuffer,
    frame: ZwlrScreencopyFrameV1,
    info: ScreencopyFrameInfo,
    submitted: bool,
}

impl Drop for Screencopy {
    fn drop(&mut self) {
        if !self.submitted {
            self.frame.failed();
        }
    }
}

impl Screencopy {
    pub fn output(&self) -> &Output {
        &self.info.output
    }

    pub fn buffer_size(&self) -> Size<i32, Physical> {
        self.info.buffer_size
    }

    pub fn submit(mut self, timestamp: Duration) {
        self.frame.flags(Flags::empty());

        let tv_sec_hi = (timestamp.as_secs() >> 32) as u32;
        let tv_sec_lo = (timestamp.as_secs() & 0xFFFFFFFF) as u32;
        let tv_nsec = timestamp.subsec_nanos();
        self.frame.ready(tv_sec_hi, tv_sec_lo, tv_nsec);

        self.submitted = true;
    }
}

pub trait ScreencopyHandler {
    fn screencopy_state(&mut self) -> &mut ScreencopyManagerState;
    fn frame(&mut self, screencopy: Screencopy);
}

impl ScreencopyHandler for ProjectWC {
    fn screencopy_state(&mut self) -> &mut ScreencopyManagerState {
        &mut self.screencopy_state
    }

    fn frame(&mut self, screencopy: Screencopy) {
        self.pending_screencopy = Some(screencopy);
    }
}

#[macro_export]
macro_rules! delegate_screencopy {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: ScreencopyManagerGlobalData
        ] => ScreencopyManagerState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: ()
        ] => ScreencopyManagerState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1: ScreencopyFrameState
        ] => ScreencopyManagerState);
    };
}

delegate_screencopy!(ProjectWC);
