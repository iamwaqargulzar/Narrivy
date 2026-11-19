//! WASAPI shared-mode capture of the default (or a chosen) input device.
//!
//! The stream is opened on the capture producer thread (COM objects stay on
//! the thread that made them) and asked for the contract format directly,
//! mono f32 at 16 kHz, with the audio engine's converter
//! (`AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`). If the endpoint refuses, it is
//! opened in the engine's mix format and [`crate::audio_format`] converts.
//!
//! Event-driven: the producer waits on the stream's event with a short
//! timeout, so Stop and Cancel are observed within one poll. A device that
//! is unplugged, disabled or reconfigured reports
//! `AUDCLNT_E_DEVICE_INVALIDATED`; that ends the source (`SourceEnded`) and
//! the audio captured so far is kept.
//!
//! A data discontinuity (the engine dropped samples because the thread did
//! not read in time, or the driver glitched) is filled with silence of the
//! measured length (the device position says how many frames are missing),
//! so the dictation keeps its timing and goes on, as on Linux. A gap over
//! [`crate::capture::MAX_FILLED_GAP_SECONDS`] fails the session as
//! `capture_overrun`. The flag on the very first packet is the stream
//! starting, not a loss, and is ignored.
//!
//! The same source records the computer's sound ([`Endpoint::Loopback`]):
//! a loopback of an output device, opened without an event (loopback
//! streams are polled) and with a silent render stream on the same device
//! beside it, because the engine sends a loopback nothing while nothing
//! plays, and the recording would lose its time.

use std::time::Duration;

use narrivy_platform_api::{
    AudioCapturePort, AudioCaptureSession, PlatformError, StreamingAudioCapturePort,
    StreamingAudioCaptureSession,
};
use narrivy_types::SessionControl;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_RESOURCES_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, EDataFlow, IAudioCaptureClient,
    IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};
use windows::core::{GUID, HSTRING, PWSTR, w};

use super::com::ComApartment;
use crate::audio_format::{
    InputFormat, MonoConverter, SampleFormat, WAVE_FORMAT_EXTENSIBLE, WAVE_FORMAT_IEEE_FLOAT,
    WaveFormatFields, WaveSubFormat,
};
use crate::capture::{
    BatchCaptureSession, CAPTURE_RATE_HZ, CaptureConfig, CaptureSource, PositionTracker,
    SourceOpener, SourceRead, StreamingCaptureSession,
};
use crate::error;

/// `KSDATAFORMAT_SUBTYPE_PCM` and `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`.
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
/// Engine buffer requested: 100 ms (in 100 ns units). Event-driven reads
/// drain it every few milliseconds.
const BUFFER_DURATION_HNS: i64 = 1_000_000;
/// How long a loopback read sleeps before draining.
const LOOPBACK_POLL: Duration = Duration::from_millis(10);

/// Which input device to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelection {
    /// The user's default recording device (the console role).
    Default,
    /// An endpoint id from [`list_input_devices`].
    Id(String),
}

/// What a [`WasapiSource`] records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// An input device (a microphone).
    Capture(DeviceSelection),
    /// What an output device plays (the computer's sound); `Id` is an
    /// endpoint id from [`list_output_devices`].
    Loopback(DeviceSelection),
}

impl Endpoint {
    fn flow(&self) -> EDataFlow {
        match self {
            Self::Capture(_) => eCapture,
            Self::Loopback(_) => eRender,
        }
    }

    fn selection(&self) -> &DeviceSelection {
        match self {
            Self::Capture(selection) | Self::Loopback(selection) => selection,
        }
    }
}

impl DeviceSelection {
    fn selection_label(&self) -> String {
        match self {
            Self::Default => "default".to_owned(),
            Self::Id(id) => id.clone(),
        }
    }
}

/// [`AudioCapturePort`] and [`StreamingAudioCapturePort`] over WASAPI.
#[derive(Debug, Clone)]
pub struct WasapiCapture {
    seconds: f32,
    device: DeviceSelection,
}

impl WasapiCapture {
    /// `seconds` is the session's bound, as for the Linux capture.
    ///
    /// # Errors
    /// `capture_duration_invalid` outside `[1, MAX_CAPTURE_SECONDS]`.
    pub fn new(seconds: f32) -> Result<Self, PlatformError> {
        CaptureConfig::new("default", seconds)?;
        Ok(Self {
            seconds,
            device: DeviceSelection::Default,
        })
    }

    #[must_use]
    pub fn with_device(mut self, device: DeviceSelection) -> Self {
        self.device = device;
        self
    }

    fn config(&self) -> Result<CaptureConfig, PlatformError> {
        CaptureConfig::new(self.device.selection_label(), self.seconds)
    }

    fn opener(&self) -> SourceOpener {
        let endpoint = Endpoint::Capture(self.device.clone());
        Box::new(move || open_source(&endpoint))
    }
}

impl AudioCapturePort for WasapiCapture {
    fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
        let session = BatchCaptureSession::start(&self.config()?, self.opener())?;
        Ok(Box::new(session))
    }
}

impl StreamingAudioCapturePort for WasapiCapture {
    fn start_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
        let session = StreamingCaptureSession::start(&self.config()?, self.opener(), control)?;
        Ok(Box::new(session))
    }
}

/// Open a source for `endpoint` on the calling thread (which then owns its
/// COM objects): the producer of a capture or meeting recording.
///
/// # Errors
/// `capture_device_unavailable` and the other open failures.
pub fn open_source(endpoint: &Endpoint) -> Result<Box<dyn CaptureSource>, PlatformError> {
    WasapiSource::open(endpoint).map(|source| Box::new(source) as Box<dyn CaptureSource>)
}

/// One input or output device, for a device picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// The active input devices.
///
/// # Errors
/// `capture_device_enumeration_failed`.
pub fn list_input_devices() -> Result<Vec<InputDevice>, PlatformError> {
    list_devices(eCapture)
}

/// The active output devices (speakers, headphones), for the computer's
/// sound in meetings.
///
/// # Errors
/// `capture_device_enumeration_failed`.
pub fn list_output_devices() -> Result<Vec<InputDevice>, PlatformError> {
    list_devices(eRender)
}

/// The endpoint id of the default input (`output == false`) or output
/// device, when there is one.
#[must_use]
pub fn default_device_id(output: bool) -> Option<String> {
    let _com = ComApartment::enter().ok()?;
    // SAFETY: COM is initialised on this thread for the scope of `_com`,
    // which outlives the interfaces below (declared later, dropped first).
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ok()?;
    let flow = if output { eRender } else { eCapture };
    // SAFETY: valid enumerator; plain enum arguments.
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }.ok()?;
    device_id(&device)
}

fn list_devices(flow: EDataFlow) -> Result<Vec<InputDevice>, PlatformError> {
    let failed = |_| error("capture_device_enumeration_failed");
    let _com = ComApartment::enter()?;
    // SAFETY: COM is initialised on this thread for the scope of `_com`,
    // which outlives every interface below (declared later, dropped first).
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(failed)?;
    // SAFETY: valid enumerator; plain enum arguments.
    let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }
        .ok()
        .and_then(|device| device_id(&device));
    // SAFETY: as above.
    let collection =
        unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }.map_err(failed)?;
    // SAFETY: valid collection.
    let count = unsafe { collection.GetCount() }.map_err(failed)?;
    let mut devices = Vec::new();
    for index in 0..count {
        // SAFETY: index < count.
        let Ok(device) = (unsafe { collection.Item(index) }) else {
            continue;
        };
        let Some(id) = device_id(&device) else {
            continue;
        };
        let name = friendly_name(&device).unwrap_or_else(|| id.clone());
        devices.push(InputDevice {
            is_default: default_id.as_deref() == Some(id.as_str()),
            id,
            name,
        });
    }
    Ok(devices)
}

/// Take ownership of a COM-allocated wide string.
fn take_co_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: a non-null, NUL-terminated string allocated by COM.
    let text = unsafe { value.to_string() }.ok();
    // SAFETY: allocated with CoTaskMemAlloc by the callee; freed once here.
    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
    text
}

fn device_id(device: &IMMDevice) -> Option<String> {
    // SAFETY: valid device; the returned string is owned by us.
    take_co_string(unsafe { device.GetId() }.ok()?)
}

fn friendly_name(device: &IMMDevice) -> Option<String> {
    // SAFETY: valid device; read-only store.
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    // SAFETY: valid store and a static property key.
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    // SAFETY: `value` is a live PROPVARIANT; the result is COM-allocated.
    take_co_string(unsafe { PropVariantToStringAlloc(&value) }.ok()?)
}

/// Owns a Win32 event handle.
struct EventHandle(HANDLE);

impl Drop for EventHandle {
    fn drop(&mut self) {
        // SAFETY: a handle we created and close exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Reverts MMCSS scheduling for the capture thread.
struct MmcssTask(HANDLE);

impl Drop for MmcssTask {
    fn drop(&mut self) {
        // SAFETY: the handle AvSetMmThreadCharacteristicsW returned on this
        // thread.
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

/// A started WASAPI capture stream. Field order is drop order: the stream
/// stops (in `Drop`) and the interfaces are released before the apartment.
struct WasapiSource {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// The stream's event; none for a loopback, which is polled.
    event: Option<EventHandle>,
    /// Keeps a loopback's engine running while nothing plays.
    _keep_alive: Option<SilentRender>,
    converter: MonoConverter,
    block_align: usize,
    /// Device positions of the packets, for the frames a gap left out.
    positions: PositionTracker,
    _mmcss: Option<MmcssTask>,
    _com: ComApartment,
}

fn is_device_gone(code: windows::core::HRESULT) -> bool {
    code == AUDCLNT_E_DEVICE_INVALIDATED
        || code == AUDCLNT_E_RESOURCES_INVALIDATED
        || code == AUDCLNT_E_SERVICE_NOT_RUNNING
}

impl WasapiSource {
    fn open(endpoint: &Endpoint) -> Result<Self, PlatformError> {
        let com = ComApartment::enter()?;
        let loopback = matches!(endpoint, Endpoint::Loopback(_));
        let unavailable = |_| error("capture_device_unavailable");
        // SAFETY: COM initialised on this thread by `com`.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|_| error("capture_device_enumeration_failed"))?;
        let device = match endpoint.selection() {
            // SAFETY: valid enumerator; enum arguments.
            DeviceSelection::Default => unsafe {
                enumerator.GetDefaultAudioEndpoint(endpoint.flow(), eConsole)
            },
            // SAFETY: valid enumerator; `id` outlives the call.
            DeviceSelection::Id(id) => unsafe { enumerator.GetDevice(&HSTRING::from(id.as_str())) },
        }
        .map_err(unavailable)?;

        let (client, converter, block_align, units_per_frame) = open_client(&device, loopback)?;
        let event = if loopback {
            None
        } else {
            // SAFETY: auto-reset event, no name, default security.
            let event = EventHandle(
                unsafe { CreateEventW(None, false, false, None) }
                    .map_err(|_| error("capture_event_failed"))?,
            );
            // SAFETY: valid initialised client; the event outlives the
            // stream (the source owns both and stops the stream before
            // closing it).
            unsafe { client.SetEventHandle(event.0) }.map_err(|_| error("capture_event_failed"))?;
            Some(event)
        };
        // Optional: without it the recorder fills the quiet time itself.
        let keep_alive = loopback
            .then(|| SilentRender::start(&device).ok())
            .flatten();
        // SAFETY: valid initialised client.
        let capture: IAudioCaptureClient =
            unsafe { client.GetService() }.map_err(|_| error("capture_stream_failed"))?;
        // Pro-audio scheduling keeps packets flowing on a busy machine.
        // Optional: without it capture still works, with more risk of
        // overruns.
        let mut task_index = 0_u32;
        // SAFETY: static task name; out-pointer to a live local.
        let mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Audio"), &mut task_index) }
            .ok()
            .map(MmcssTask);
        // SAFETY: valid initialised client (with an event set, unless it is
        // a polled loopback).
        unsafe { client.Start() }.map_err(|failure| {
            if is_device_gone(failure.code()) {
                error("capture_device_unavailable")
            } else {
                error("capture_stream_failed")
            }
        })?;
        Ok(Self {
            client,
            capture,
            event,
            _keep_alive: keep_alive,
            converter,
            block_align,
            positions: PositionTracker::new(units_per_frame),
            _mmcss: mmcss,
            _com: com,
        })
    }

    /// Drain every packet the engine holds.
    fn drain(&mut self, out: &mut Vec<f32>) -> Result<SourceRead, PlatformError> {
        let mut read = SourceRead::Idle;
        loop {
            // SAFETY: valid started capture client.
            let packet = match unsafe { self.capture.GetNextPacketSize() } {
                Ok(packet) => packet,
                Err(failure) if is_device_gone(failure.code()) => return Ok(SourceRead::Ended),
                Err(_) => return Err(error("capture_output_unreadable")),
            };
            if packet == 0 {
                return Ok(read);
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0_u32;
            let mut flags = 0_u32;
            let mut position = 0_u64;
            // SAFETY: out-pointers to live locals; the buffer is valid until
            // the matching ReleaseBuffer below.
            match unsafe {
                self.capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    None,
                )
            } {
                Ok(()) => {}
                Err(failure) if is_device_gone(failure.code()) => return Ok(SourceRead::Ended),
                Err(_) => return Err(error("capture_output_unreadable")),
            }
            let frames_usize = frames as usize;
            let discontinuity = flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;
            let gap = self.positions.packet(
                position,
                frames,
                discontinuity,
                self.converter.input().sample_rate_hz,
            );
            let converted = match gap {
                None => Err(error("capture_overrun")),
                Some(missing) => {
                    self.converter.push_silence(missing, out);
                    if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                        self.converter.push_silence(frames_usize, out);
                        Ok(())
                    } else {
                        // SAFETY: WASAPI guarantees `frames * block_align`
                        // readable bytes at `data` until ReleaseBuffer.
                        let bytes = unsafe {
                            std::slice::from_raw_parts(data, frames_usize * self.block_align)
                        };
                        self.converter
                            .push(bytes, frames_usize, out)
                            .map_err(|failure| error(failure.code()))
                    }
                }
            };
            // SAFETY: releases exactly the frames GetBuffer returned.
            let released = unsafe { self.capture.ReleaseBuffer(frames) };
            converted?;
            if let Err(failure) = released {
                return if is_device_gone(failure.code()) {
                    Ok(SourceRead::Ended)
                } else {
                    Err(error("capture_output_unreadable"))
                };
            }
            if frames > 0 {
                read = SourceRead::Data;
            }
        }
    }
}

impl CaptureSource for WasapiSource {
    fn read(&mut self, out: &mut Vec<f32>, wait: Duration) -> Result<SourceRead, PlatformError> {
        match &self.event {
            Some(event) => {
                let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
                // Signalled or timed out, the packet calls below tell: a
                // device that went away stops signalling and reports itself
                // through them.
                // SAFETY: a live event handle owned by this source.
                let _ = unsafe { WaitForSingleObject(event.0, millis) };
            }
            // A loopback is polled: the engine sends a packet every 10 ms.
            None => std::thread::sleep(wait.min(LOOPBACK_POLL)),
        }
        self.drain(out)
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        // Whatever the engine still holds was captured before Stop.
        let _ = self.drain(out);
        self.converter.flush(out);
    }
}

impl Drop for WasapiSource {
    fn drop(&mut self) {
        // SAFETY: valid client; stopping an already stopped or invalidated
        // stream is harmless.
        let _ = unsafe { self.client.Stop() };
    }
}

/// A render stream on an output device that plays nothing: the engine
/// processes the device while a stream is running, so its loopback keeps
/// sending (silent) packets and the recording keeps its time. The buffer is
/// filled with silence once; running dry later just plays silence.
struct SilentRender {
    client: IAudioClient,
}

impl SilentRender {
    fn start(device: &IMMDevice) -> Result<Self, PlatformError> {
        let failed = |_| error("capture_stream_failed");
        // SAFETY: valid device; COM is initialised on this thread (the
        // source's apartment, which outlives this stream).
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(failed)?;
        // SAFETY: valid client; freed below after Initialize has copied it.
        let mix = unsafe { client.GetMixFormat() }.map_err(failed)?;
        // SAFETY: valid client; `mix` is alive for the call.
        let initialised = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                BUFFER_DURATION_HNS,
                0,
                mix,
                None,
            )
        };
        // SAFETY: allocated by GetMixFormat; freed exactly once.
        unsafe { CoTaskMemFree(Some(mix as *const _)) };
        initialised.map_err(failed)?;
        // SAFETY: valid initialised client.
        let frames = unsafe { client.GetBufferSize() }.map_err(failed)?;
        // SAFETY: valid initialised client.
        let render: IAudioRenderClient = unsafe { client.GetService() }.map_err(failed)?;
        // SAFETY: the whole buffer is requested before the stream starts;
        // released at once as silent, so its bytes are never read.
        unsafe { render.GetBuffer(frames) }.map_err(failed)?;
        // SAFETY: releases exactly the frames GetBuffer returned.
        unsafe { render.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) }
            .map_err(failed)?;
        // SAFETY: valid initialised client with its buffer filled.
        unsafe { client.Start() }.map_err(failed)?;
        Ok(Self { client })
    }
}

impl Drop for SilentRender {
    fn drop(&mut self) {
        // SAFETY: valid client; stopping a stopped stream is harmless.
        let _ = unsafe { self.client.Stop() };
    }
}

/// Initialise a shared-mode client: the contract format through the engine's
/// converter, else the mix format with our own conversion. A loopback is
/// opened on an output device, without an event. The last value is the
/// first guess of device position units per stream frame: the engine's rate
/// over the contract rate when the engine converts, else one.
fn open_client(
    device: &IMMDevice,
    loopback: bool,
) -> Result<(IAudioClient, MonoConverter, usize, f64), PlatformError> {
    let mode = if loopback {
        AUDCLNT_STREAMFLAGS_LOOPBACK
    } else {
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    };
    let activate = || -> Result<IAudioClient, PlatformError> {
        // SAFETY: valid device; COM is initialised on this thread.
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|failure| {
            if is_device_gone(failure.code()) {
                error("capture_device_unavailable")
            } else {
                error("capture_stream_failed")
            }
        })
    };

    let contract = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: 1,
        nSamplesPerSec: CAPTURE_RATE_HZ,
        nAvgBytesPerSec: CAPTURE_RATE_HZ * 4,
        nBlockAlign: 4,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let client = activate()?;
    let flags = mode | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    // SAFETY: valid client; `contract` lives across the call.
    let direct = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_DURATION_HNS,
            0,
            &contract,
            None,
        )
    };
    if direct.is_ok() {
        let converter = MonoConverter::new(InputFormat {
            sample_rate_hz: CAPTURE_RATE_HZ,
            channels: 1,
            sample: SampleFormat::F32,
            block_align: 4,
        })
        .map_err(|failure| error(failure.code()))?;
        // SAFETY: valid client; the returned format is CoTaskMemAlloc'd and
        // freed right after its rate is read.
        let units_per_frame = match unsafe { client.GetMixFormat() } {
            Ok(mix) => {
                // SAFETY: `mix` is a valid WAVEFORMATEX until freed.
                let rate = unsafe { wave_fields(mix) }.sample_rate_hz;
                // SAFETY: allocated by GetMixFormat; freed exactly once.
                unsafe { CoTaskMemFree(Some(mix as *const _)) };
                f64::from(rate) / f64::from(CAPTURE_RATE_HZ)
            }
            Err(_) => 1.0,
        };
        return Ok((client, converter, 4, units_per_frame));
    }

    // A client whose Initialize failed is not reused.
    drop(client);
    let client = activate()?;
    // SAFETY: valid client; the returned format is CoTaskMemAlloc'd and
    // freed below after it has been copied.
    let mix = unsafe { client.GetMixFormat() }.map_err(|_| error("capture_format_unsupported"))?;
    // SAFETY: `mix` is a valid WAVEFORMATEX (plus extension when cbSize says
    // so) until freed.
    let fields = unsafe { wave_fields(mix) };
    // SAFETY: valid client; `mix` is alive for the call.
    let initialised = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            mode,
            BUFFER_DURATION_HNS,
            0,
            mix,
            None,
        )
    };
    // SAFETY: allocated by GetMixFormat; freed exactly once.
    unsafe { CoTaskMemFree(Some(mix as *const _)) };
    initialised.map_err(|failure| {
        if is_device_gone(failure.code()) {
            error("capture_device_unavailable")
        } else {
            error("capture_stream_failed")
        }
    })?;
    let format = InputFormat::from_wave(fields).map_err(|failure| error(failure.code()))?;
    let converter = MonoConverter::new(format).map_err(|failure| error(failure.code()))?;
    Ok((client, converter, format.block_align, 1.0))
}

/// Copy the layout fields out of a (possibly extensible) `WAVEFORMATEX`.
///
/// # Safety
/// `format` must point to a valid `WAVEFORMATEX` followed by `cbSize`
/// extension bytes.
unsafe fn wave_fields(format: *const WAVEFORMATEX) -> WaveFormatFields {
    // SAFETY: caller guarantees validity; packed struct read unaligned.
    let base = unsafe { format.read_unaligned() };
    let extension_bytes =
        std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>();
    let extensible = (base.wFormatTag == WAVE_FORMAT_EXTENSIBLE
        && usize::from(base.cbSize) >= extension_bytes)
        .then(|| {
            // SAFETY: cbSize covers the extension, so the whole
            // WAVEFORMATEXTENSIBLE is readable.
            let ext = unsafe { format.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() };
            let sub = ext.SubFormat;
            let kind = if sub == SUBTYPE_IEEE_FLOAT {
                WaveSubFormat::Float
            } else if sub == SUBTYPE_PCM {
                WaveSubFormat::Pcm
            } else {
                WaveSubFormat::Other
            };
            // SAFETY: every member of this union is a u16.
            let valid_bits = unsafe { ext.Samples.wValidBitsPerSample };
            (kind, valid_bits)
        });
    WaveFormatFields {
        tag: base.wFormatTag,
        channels: base.nChannels,
        sample_rate_hz: base.nSamplesPerSec,
        block_align: base.nBlockAlign,
        bits_per_sample: base.wBitsPerSample,
        extensible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_platform_api::CaptureEnd;

    /// Needs a working input device: run on the Windows test machine with
    /// `--ignored`.
    #[test]
    #[ignore = "needs a Windows input device"]
    fn default_device_captures_contract_audio() {
        let mut capture = match WasapiCapture::new(5.0) {
            Ok(capture) => capture,
            Err(failure) => unreachable!("{failure:?}"),
        };
        let mut session = match capture.start() {
            Ok(session) => session,
            Err(failure) => unreachable!("{failure:?}"),
        };
        let control = SessionControl::new();
        assert!(session.wait_ready(&control).is_ok());
        std::thread::sleep(Duration::from_millis(500));
        control.request_finalization();
        match session.collect(&control) {
            Ok(captured) => {
                assert_eq!(captured.end, CaptureEnd::FinalizationRequested);
                assert_eq!(captured.audio.sample_rate_hz, 16_000);
                assert!(captured.audio.samples.len() >= 4_000);
            }
            Err(failure) => unreachable!("{failure:?}"),
        }
    }

    #[test]
    #[ignore = "needs a Windows audio service"]
    fn devices_are_listed() {
        let devices = list_input_devices();
        assert!(devices.is_ok());
    }
}
