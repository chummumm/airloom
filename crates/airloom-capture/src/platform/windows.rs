//! All Core Audio objects are created, used and released on one COM worker.

#[path = "applications.rs"]
mod applications;

pub(crate) use applications::enumerate_applications;

use crate::mute_policy::MuteOwnership;
use crate::pipeline::{levels, Encoding, NativeFormat, StereoResampler};
use crate::{
    is_application_source, AudioChunk, CaptureConfig, CaptureError, CaptureInfo, CaptureSource,
    Result, DEFAULT_SOURCE_ID,
};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::core::{implement, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateGuid, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

fn failure(operation: &'static str, error: windows::core::Error) -> CaptureError {
    CaptureError::Windows {
        operation,
        code: error.code().0,
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| failure("CoInitializeEx", e))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct TaskAllocation(*mut c_void);

impl Drop for TaskAllocation {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast_const())) };
    }
}

fn allocated_string(value: PWSTR) -> Result<String> {
    let _allocation = TaskAllocation(value.0.cast());
    if value.is_null() {
        return Err(CaptureError::Conversion(
            "Windows returned a null device name".into(),
        ));
    }
    unsafe { value.to_string() }
        .map_err(|_| CaptureError::Conversion("Windows returned an invalid device name".into()))
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| failure("Create device enumerator", e))
}

fn device_id(device: &IMMDevice) -> Result<String> {
    allocated_string(unsafe { device.GetId() }.map_err(|e| failure("Get endpoint ID", e))?)
}

fn friendly_name(device: &IMMDevice) -> Result<String> {
    let properties = unsafe { device.OpenPropertyStore(STGM_READ) }
        .map_err(|e| failure("Open endpoint properties", e))?;
    let value = unsafe { properties.GetValue(&PKEY_Device_FriendlyName) }
        .map_err(|e| failure("Read endpoint name", e))?;
    let result = unsafe { PropVariantToStringAlloc(&value) };
    // windows-rs' PROPVARIANT Drop clears its original allocation; the string
    // produced by PropVariantToStringAlloc has separate ownership.
    allocated_string(result.map_err(|e| failure("Convert endpoint name", e))?)
}

pub(crate) fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    thread::Builder::new()
        .name("airloom-enumerate".into())
        .spawn(|| {
            let _apartment = ComApartment::initialize()?;
            let enumerator = enumerator()?;
            let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
                .ok()
                .and_then(|device| device_id(&device).ok());
            let devices = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
                .map_err(|e| failure("Enumerate playback endpoints", e))?;
            let count = unsafe { devices.GetCount() }
                .map_err(|e| failure("Count playback endpoints", e))?;
            let mut sources = Vec::with_capacity(count as usize);
            for index in 0..count {
                let device = unsafe { devices.Item(index) }
                    .map_err(|e| failure("Read playback endpoint", e))?;
                let id = device_id(&device)?;
                let name = friendly_name(&device).unwrap_or_else(|_| id.clone());
                let is_default = default_id.as_ref() == Some(&id);
                sources.push(CaptureSource {
                    id,
                    name,
                    is_default,
                });
            }
            Ok(sources)
        })
        .map_err(CaptureError::WorkerStart)?
        .join()
        .map_err(|_| CaptureError::WorkerPanicked)?
}

enum Command {
    SetMute(bool, Sender<Result<()>>),
}

pub(crate) struct Stream {
    pub(crate) info: CaptureInfo,
    audio: Receiver<AudioChunk>,
    commands: Sender<Command>,
    cancelled: Arc<AtomicBool>,
    terminal_error: Arc<Mutex<Option<CaptureError>>>,
    worker: Option<JoinHandle<()>>,
}

impl Stream {
    pub(crate) fn start(config: CaptureConfig) -> Result<Self> {
        let (audio_sender, audio) = bounded(config.queue_capacity);
        let discard = audio.clone();
        let (commands, command_receiver) = bounded(8);
        let (ready_sender, ready) = bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let terminal_error = Arc::new(Mutex::new(None));
        let worker_error = terminal_error.clone();
        let worker = thread::Builder::new()
            .name("airloom-wasapi".into())
            .spawn(move || {
                let result = run_worker(
                    config,
                    &audio_sender,
                    &discard,
                    command_receiver,
                    &worker_cancelled,
                    ready_sender,
                );
                // Store before dropping the last sender so readers see the
                // actual failure when the channel becomes disconnected.
                if let Err(error) = result {
                    if let Ok(mut slot) = worker_error.lock() {
                        *slot = Some(error);
                    }
                }
            })
            .map_err(CaptureError::WorkerStart)?;
        let mut stream = Self {
            info: CaptureInfo::default(),
            audio,
            commands,
            cancelled,
            terminal_error,
            worker: Some(worker),
        };
        match ready.recv() {
            Ok(Ok(info)) => {
                stream.info = info;
                Ok(stream)
            }
            Ok(Err(error)) => {
                let _ = stream.stop();
                Err(error)
            }
            Err(_) => {
                stream.stop()?;
                Err(CaptureError::WorkerStopped)
            }
        }
    }

    pub(crate) fn read_timeout(&self, timeout: Duration) -> Result<Option<AudioChunk>> {
        if let Some(error) = self.take_error() {
            while self.audio.try_recv().is_ok() {}
            return Err(error);
        }
        match self.audio.recv_timeout(timeout) {
            Ok(chunk) => {
                // Do not deliver queued samples after the worker has reported
                // a source failure, especially a selected application exiting.
                if let Some(error) = self.take_error() {
                    while self.audio.try_recv().is_ok() {}
                    Err(error)
                } else {
                    Ok(Some(chunk))
                }
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(self.take_error().unwrap_or(CaptureError::WorkerStopped))
            }
        }
    }

    pub(crate) fn set_local_mute(&self, enabled: bool) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(CaptureError::WorkerStopped);
        }
        let (reply, response) = bounded(1);
        self.commands
            .send_timeout(Command::SetMute(enabled, reply), Duration::from_secs(2))
            .map_err(|error| match error {
                crossbeam_channel::SendTimeoutError::Timeout(_) => {
                    CaptureError::Timeout("capture command queue")
                }
                crossbeam_channel::SendTimeoutError::Disconnected(_) => CaptureError::WorkerStopped,
            })?;
        response.recv().map_err(|_| CaptureError::WorkerStopped)?
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| CaptureError::WorkerPanicked)?;
        }
        while self.audio.try_recv().is_ok() {}
        match self.take_error() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn take_error(&self) -> Option<CaptureError> {
        self.terminal_error
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }
}

fn run_worker(
    config: CaptureConfig,
    audio: &Sender<AudioChunk>,
    discard: &Receiver<AudioChunk>,
    commands: Receiver<Command>,
    cancelled: &AtomicBool,
    ready: Sender<Result<CaptureInfo>>,
) -> Result<()> {
    // Declared before Session so every COM interface is released before COM.
    let _apartment = match ComApartment::initialize() {
        Ok(apartment) => apartment,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let mut session = match Session::start(&config, cancelled) {
        Ok(session) => session,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    if ready.send(Ok(session.info.clone())).is_err() {
        return session.stop();
    }
    let mut sequence = 0u64;
    let mut dropped_frames = 0u64;
    let result = (|| {
        while !cancelled.load(Ordering::Acquire) {
            if let Some(process) = &session.process {
                process.ensure_alive()?;
            }
            while let Ok(command) = commands.try_recv() {
                match command {
                    Command::SetMute(enabled, reply) => {
                        let _ = reply.send(session.set_mute(enabled));
                    }
                }
            }
            if unsafe { WaitForSingleObject(session.event.0, 20) } == WAIT_FAILED {
                return Err(failure(
                    "Wait for capture event",
                    windows::core::Error::from_thread(),
                ));
            }
            // Bound work between cancellation/command checks even if the
            // endpoint continuously adds packets while this thread drains it.
            for _ in 0..64 {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let Some(chunks) = session.read_packet()? else {
                    break;
                };
                for samples in chunks {
                    if samples.is_empty() {
                        continue;
                    }
                    let (rms, peak) = levels(&samples);
                    let chunk = AudioChunk {
                        samples,
                        sample_rate: config.target_sample_rate,
                        sequence,
                        rms,
                        peak,
                        dropped_frames,
                    };
                    sequence = sequence.wrapping_add(1);
                    enqueue_latest(audio, discard, chunk, &mut dropped_frames)?;
                }
            }
        }
        Ok(())
    })();
    let cleanup = session.stop();
    result.and(cleanup)
}

fn enqueue_latest(
    sender: &Sender<AudioChunk>,
    discard: &Receiver<AudioChunk>,
    mut chunk: AudioChunk,
    dropped_frames: &mut u64,
) -> Result<()> {
    loop {
        chunk.dropped_frames = *dropped_frames;
        match sender.try_send(chunk) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(CaptureError::WorkerStopped),
            Err(TrySendError::Full(returned)) => {
                chunk = returned;
                if let Ok(oldest) = discard.try_recv() {
                    *dropped_frames = dropped_frames.saturating_add(oldest.frames() as u64);
                }
            }
        }
    }
}

struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Session {
    // Drop order keeps the event alive until the audio client is released.
    client: IAudioClient,
    capture: IAudioCaptureClient,
    device: Option<IMMDevice>,
    process: Option<applications::ApplicationProcess>,
    event: Event,
    format: NativeFormat,
    resampler: StereoResampler,
    buffer_frames: u32,
    local_mute: Option<LocalMute>,
    started: bool,
    info: CaptureInfo,
}

impl Session {
    fn start(config: &CaptureConfig, cancelled: &AtomicBool) -> Result<Self> {
        if is_application_source(&config.source_id) {
            return Self::start_application(config, cancelled);
        }
        let enumerator = enumerator()?;
        let device = if config.source_id == DEFAULT_SOURCE_ID {
            unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
        } else {
            let source: Vec<u16> = config.source_id.encode_utf16().chain(Some(0)).collect();
            unsafe { enumerator.GetDevice(PCWSTR(source.as_ptr())) }
        }
        .map_err(|e| failure("Select playback endpoint", e))?;
        let source_id = device_id(&device)?;
        let info = CaptureInfo {
            source_name: friendly_name(&device).unwrap_or_else(|_| source_id.clone()),
            source_id,
        };
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|e| failure("Activate WASAPI client", e))?;
        let mix = unsafe { client.GetMixFormat() }.map_err(|e| failure("Get mix format", e))?;
        let _mix_allocation = TaskAllocation(mix.cast());
        let format = unsafe { parse_mix_format(mix) }?;
        let resampler = StereoResampler::new(format.sample_rate, config.target_sample_rate)?;
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0,
                0,
                mix,
                None,
            )
        }
        .map_err(|e| failure("Initialize shared loopback", e))?;
        let event = Event(
            unsafe { CreateEventW(None, false, false, None) }
                .map_err(|e| failure("Create capture event", e))?,
        );
        unsafe { client.SetEventHandle(event.0) }
            .map_err(|e| failure("Attach capture event", e))?;
        let capture: IAudioCaptureClient =
            unsafe { client.GetService() }.map_err(|e| failure("Get WASAPI capture service", e))?;
        let buffer_frames =
            unsafe { client.GetBufferSize() }.map_err(|e| failure("Get WASAPI buffer size", e))?;
        let mut session = Self {
            client,
            capture,
            device: Some(device),
            process: None,
            event,
            format,
            resampler,
            buffer_frames,
            local_mute: None,
            started: false,
            info,
        };
        unsafe { session.client.Start() }.map_err(|e| failure("Start WASAPI capture", e))?;
        session.started = true;
        if config.mute_local {
            session.set_mute(true)?;
        }
        Ok(session)
    }

    fn start_application(config: &CaptureConfig, cancelled: &AtomicBool) -> Result<Self> {
        let process = applications::ApplicationProcess::open(&config.source_id)?;
        let info = CaptureInfo {
            source_id: config.source_id.clone(),
            source_name: process.name(),
        };
        let client = process.activate(cancelled)?;
        process.ensure_alive()?;
        // The virtual process device has no physical endpoint mix format.
        // Request float samples, as used by our pipeline and Windows' shared
        // audio engine. Capturing PCM16 here would introduce an unnecessary
        // float -> integer -> float conversion before final network encoding.
        let mix = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: 2,
            nSamplesPerSec: config.target_sample_rate,
            nAvgBytesPerSec: config.target_sample_rate * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let format = NativeFormat::new(config.target_sample_rate, 2, 8, 0, Encoding::Float32)?;
        let resampler = StereoResampler::new(config.target_sample_rate, config.target_sample_rate)?;
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                0,
                0,
                &mix,
                None,
            )
        }
        .map_err(|error| failure("Initialize application audio capture", error))?;
        let event = Event(
            unsafe { CreateEventW(None, false, false, None) }
                .map_err(|error| failure("Create application capture event", error))?,
        );
        unsafe { client.SetEventHandle(event.0) }
            .map_err(|error| failure("Attach application capture event", error))?;
        let capture = unsafe { client.GetService() }
            .map_err(|error| failure("Get application capture service", error))?;
        let buffer_frames = unsafe { client.GetBufferSize() }
            .map_err(|error| failure("Get application capture buffer size", error))?;
        let mut session = Self {
            client,
            capture,
            device: None,
            process: Some(process),
            event,
            format,
            resampler,
            buffer_frames,
            local_mute: None,
            started: false,
            info,
        };
        unsafe { session.client.Start() }
            .map_err(|error| failure("Start application audio capture", error))?;
        session.started = true;
        Ok(session)
    }

    fn read_packet(&mut self) -> Result<Option<Vec<Vec<f32>>>> {
        if unsafe { self.capture.GetNextPacketSize() }
            .map_err(|e| failure("Get capture packet size", e))?
            == 0
        {
            return Ok(None);
        }
        let mut data = ptr::null_mut();
        let mut frames = 0;
        let mut flags = 0;
        unsafe {
            self.capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
        }
        .map_err(|e| failure("Get capture buffer", e))?;
        let decoded = (|| {
            if frames > self.buffer_frames {
                return Err(CaptureError::Conversion(
                    "WASAPI packet exceeds its buffer".into(),
                ));
            }
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            let bytes = if silent || frames == 0 {
                &[][..]
            } else {
                if data.is_null() {
                    return Err(CaptureError::Conversion(
                        "WASAPI returned a null audio buffer".into(),
                    ));
                }
                let length = (frames as usize)
                    .checked_mul(self.format.block_align)
                    .ok_or_else(|| {
                        CaptureError::Conversion("WASAPI packet size overflow".into())
                    })?;
                unsafe { std::slice::from_raw_parts(data, length) }
            };
            self.format.decode(bytes, frames as usize, silent)
        })();
        // Always release the Windows buffer, including validation failures,
        // before resampling or trying to deliver a chunk to another thread.
        let released = unsafe { self.capture.ReleaseBuffer(frames) }
            .map_err(|e| failure("Release capture buffer", e));
        let samples = decoded?;
        released?;
        if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
            self.resampler.reset();
        }
        Ok(Some(self.resampler.push(samples)?))
    }

    fn set_mute(&mut self, enabled: bool) -> Result<()> {
        if enabled && self.local_mute.is_none() {
            let device = self
                .device
                .as_ref()
                .ok_or(CaptureError::ApplicationMuteUnsupported)?;
            self.local_mute = Some(LocalMute::new(device)?);
        }
        if let Some(mute) = &mut self.local_mute {
            mute.set_enabled(enabled)?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        let stop_result = if self.started {
            self.started = false;
            unsafe { self.client.Stop() }.map_err(|e| failure("Stop WASAPI capture", e))
        } else {
            Ok(())
        };
        let mute_result = self.set_mute(false);
        stop_result.and(mute_result)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// The allocation comes from IAudioClient::GetMixFormat. Its cbSize describes
/// the valid trailing bytes; packed WAVEFORMATEX fields are read unaligned.
unsafe fn parse_mix_format(pointer: *const WAVEFORMATEX) -> Result<NativeFormat> {
    if pointer.is_null() {
        return Err(CaptureError::UnsupportedFormat("null mix format".into()));
    }
    let base = unsafe { pointer.read_unaligned() };
    let bits = base.wBitsPerSample;
    let mut tag = base.wFormatTag;
    let mut mask = 0u32;
    if tag == 0xfffe {
        if base.cbSize < 22 {
            return Err(CaptureError::UnsupportedFormat(
                "short extensible format".into(),
            ));
        }
        let bytes = pointer.cast::<u8>();
        let valid_bits = unsafe { bytes.add(18).cast::<u16>().read_unaligned() };
        mask = unsafe { bytes.add(20).cast::<u32>().read_unaligned() };
        let subformat = unsafe { bytes.add(24).cast::<GUID>().read_unaligned() };
        tag = if subformat == GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71) {
            1
        } else if subformat == GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71) {
            3
        } else {
            return Err(CaptureError::UnsupportedFormat(
                "unsupported extensible subformat".into(),
            ));
        };
        if valid_bits > bits {
            return Err(CaptureError::UnsupportedFormat(
                "valid bits exceed container width".into(),
            ));
        }
    }
    let encoding = match (tag, bits) {
        (1, 8) => Encoding::Unsigned8,
        (1, 16) => Encoding::Signed16,
        (1, 24) => Encoding::Signed24,
        (1, 32) => Encoding::Signed32,
        (3, 32) => Encoding::Float32,
        (3, 64) => Encoding::Float64,
        _ => {
            return Err(CaptureError::UnsupportedFormat(format!(
                "format tag {tag}, {bits}-bit samples"
            )));
        }
    };
    NativeFormat::new(
        base.nSamplesPerSec,
        base.nChannels as usize,
        base.nBlockAlign as usize,
        mask,
        encoding,
    )
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeObserver {
    context: GUID,
    changed_externally: Arc<AtomicBool>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeObserver_Impl {
    fn OnNotify(
        &self,
        notification: *mut AUDIO_VOLUME_NOTIFICATION_DATA,
    ) -> windows::core::Result<()> {
        if !notification.is_null() && unsafe { (*notification).guidEventContext } != self.context {
            // The callback only records an atomic flag; it never calls Core
            // Audio, waits for the worker, or touches UI state.
            self.changed_externally.store(true, Ordering::Release);
        }
        Ok(())
    }
}

struct LocalMute {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
    context: GUID,
    changed_externally: Arc<AtomicBool>,
    ownership: MuteOwnership,
}

impl LocalMute {
    fn new(device: &IMMDevice) -> Result<Self> {
        let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|e| failure("Activate endpoint volume", e))?;
        let context = unsafe { CoCreateGuid() }.map_err(|e| failure("Create mute context", e))?;
        let changed_externally = Arc::new(AtomicBool::new(false));
        let callback: IAudioEndpointVolumeCallback = VolumeObserver {
            context,
            changed_externally: changed_externally.clone(),
        }
        .into();
        unsafe { volume.RegisterControlChangeNotify(&callback) }
            .map_err(|e| failure("Observe endpoint volume", e))?;
        Ok(Self {
            volume,
            callback,
            context,
            changed_externally,
            ownership: MuteOwnership::default(),
        })
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        if enabled {
            if self.ownership.is_active() {
                return Ok(());
            }
            // Reset before reading the baseline. An external notification from
            // this point onward conservatively relinquishes restoration rights.
            self.changed_externally.store(false, Ordering::Release);
            let original = unsafe { self.volume.GetMute() }
                .map_err(|e| failure("Read endpoint mute", e))?
                .as_bool();
            if !original {
                unsafe { self.volume.SetMute(true, &self.context) }
                    .map_err(|e| failure("Mute local endpoint", e))?;
            }
            self.ownership.acquire(original);
            return Ok(());
        }
        if !self.ownership.is_active() {
            return Ok(());
        }
        let current = unsafe { self.volume.GetMute() }
            .map_err(|e| failure("Read endpoint mute", e))?
            .as_bool();
        let external = self.changed_externally.load(Ordering::Acquire);
        if let Some(restore) = self.ownership.release(external, current) {
            // A last check narrows the unavoidable race with an independent
            // system volume control; there is no atomic conditional SetMute API.
            if !self.changed_externally.load(Ordering::Acquire) {
                if let Err(error) = unsafe { self.volume.SetMute(restore, &self.context) } {
                    self.ownership.acquire(false); // allow cleanup to retry
                    return Err(failure("Restore endpoint mute", error));
                }
            }
        }
        Ok(())
    }
}

impl Drop for LocalMute {
    fn drop(&mut self) {
        let _ = self.set_enabled(false);
        let _ = unsafe { self.volume.UnregisterControlChangeNotify(&self.callback) };
    }
}
