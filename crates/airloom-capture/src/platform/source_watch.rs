//! Windows source discovery driven only by notifications and explicit refresh.

use super::applications::{sources_from_sessions, ApplicationProcess};
use super::{allocated_string, device_id, enumerator, failure, friendly_name, ComApartment};
use crate::source_changes::signal::{ChangeSignal, APPLICATIONS, DEVICES, REFRESH};
use crate::{CaptureError, CaptureSource, Result, SourceSnapshot};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::watch;
use windows::core::{implement, Interface, Ref, BOOL, GUID, PCWSTR};
use windows::Win32::Foundation::{HMODULE, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    eAll, eMultimedia, eRender, AudioSessionDisconnectReason, AudioSessionState,
    AudioSessionStateExpired, DisconnectReasonDeviceRemoval, DisconnectReasonFormatChanged,
    DisconnectReasonServerShutdown, EDataFlow, ERole, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionEvents, IAudioSessionEvents_Impl, IAudioSessionManager2,
    IAudioSessionNotification, IAudioSessionNotification_Impl, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, DEVICE_STATE, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
};
use windows::Win32::System::Threading::{
    CloseThreadpoolWait, CreateThreadpoolWait, SetThreadpoolWait, WaitForThreadpoolWaitCallbacks,
    PTP_CALLBACK_INSTANCE, PTP_WAIT,
};

pub(crate) struct SourceWatch {
    signal: Arc<ChangeSignal>,
    finished: Receiver<()>,
    worker: Option<JoinHandle<()>>,
}

impl SourceWatch {
    pub(crate) fn start() -> Result<(Self, watch::Receiver<SourceSnapshot>)> {
        let (signal, wake) = ChangeSignal::channel();
        let worker_signal = signal.clone();
        let (created, created_sessions) = unbounded();
        let (updates, snapshots) = watch::channel(SourceSnapshot::default());
        let (ready, started) = bounded(1);
        let (done, finished) = bounded(1);
        let worker = thread::Builder::new()
            .name("airloom-source-events".into())
            .spawn(move || {
                let result = run(
                    worker_signal.clone(),
                    wake,
                    created,
                    created_sessions,
                    updates,
                    &ready,
                );
                if let Err(error) = result {
                    let _ = ready.try_send(Err(error));
                }
                worker_signal.close();
                let _ = done.try_send(());
            })
            .map_err(CaptureError::WorkerStart)?;
        let control = Self {
            signal,
            finished,
            worker: Some(worker),
        };
        match started.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok((control, snapshots)),
            Ok(Err(error)) => Err(error),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                Err(CaptureError::Timeout("audio source notification startup"))
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                Err(CaptureError::WorkerStopped)
            }
        }
    }

    pub(crate) fn refresh(&self) -> Result<()> {
        if self.signal.is_closed() {
            return Err(CaptureError::WorkerStopped);
        }
        self.signal.notify(DEVICES | APPLICATIONS | REFRESH);
        Ok(())
    }
}

impl Drop for SourceWatch {
    fn drop(&mut self) {
        self.signal.close();
        if let Some(worker) = self.worker.take() {
            // COM unregistration belongs to the worker. A stalled Windows RPC
            // must not block the caller indefinitely. If the deadline expires,
            // that worker retains all its objects until cleanup can complete;
            // the callback module is pinned for its entire process lifetime.
            if self.finished.recv_timeout(Duration::from_secs(1)).is_ok() || worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

fn run(
    signal: Arc<ChangeSignal>,
    wake: Receiver<()>,
    created: Sender<CreatedSession>,
    created_sessions: Receiver<CreatedSession>,
    updates: watch::Sender<SourceSnapshot>,
    ready: &Sender<Result<()>>,
) -> Result<()> {
    let _apartment = ComApartment::initialize()?;
    let mut worker = WatchWorker::new(signal, wake, created, created_sessions)?;
    worker.refresh_devices(true);
    worker.drain_created();
    worker.publish(&updates, true);
    let _ = ready.try_send(Ok(()));
    // An indefinite receive is intentional: no timer, periodic enumeration,
    // or inactivity-driven retry runs in this worker.
    while !worker.signal.is_closed() {
        if worker.wake.recv().is_err() || worker.signal.is_closed() {
            break;
        }
        let changes = worker.signal.take_changes();
        if changes == 0 {
            continue;
        }
        if changes & (DEVICES | REFRESH) != 0 {
            worker.refresh_devices(changes & REFRESH != 0);
        }
        worker.drain_created();
        worker.publish(&updates, changes & REFRESH != 0);
    }
    Ok(())
}

struct WatchWorker {
    signal: Arc<ChangeSignal>,
    wake: Receiver<()>,
    created: Sender<CreatedSession>,
    created_sessions: Receiver<CreatedSession>,
    enumerator: IMMDeviceEnumerator,
    notification: IMMNotificationClient,
    registered: bool,
    endpoints: BTreeMap<String, EndpointWatch>,
    endpoint_sources: Vec<CaptureSource>,
    endpoint_error: Option<String>,
    application_errors: BTreeMap<String, String>,
    processes: BTreeMap<String, ProcessExitWait>,
}

impl WatchWorker {
    fn new(
        signal: Arc<ChangeSignal>,
        wake: Receiver<()>,
        created: Sender<CreatedSession>,
        created_sessions: Receiver<CreatedSession>,
    ) -> Result<Self> {
        let enumerator = enumerator()?;
        let notification: IMMNotificationClient = DeviceObserver {
            signal: signal.clone(),
        }
        .into();
        pin_notification_module(&notification)?;
        let mut worker = Self {
            signal,
            wake,
            created,
            created_sessions,
            enumerator,
            notification,
            registered: false,
            endpoints: BTreeMap::new(),
            endpoint_sources: Vec::new(),
            endpoint_error: None,
            application_errors: BTreeMap::new(),
            processes: BTreeMap::new(),
        };
        unsafe {
            worker
                .enumerator
                .RegisterEndpointNotificationCallback(&worker.notification)
        }
        .map_err(|error| failure("Watch playback device changes", error))?;
        worker.registered = true;
        Ok(worker)
    }

    fn refresh_devices(&mut self, manual: bool) {
        let result = self.read_devices();
        let devices = match result {
            Ok(devices) => devices,
            Err(error) => {
                self.endpoint_error = Some(error.to_string());
                return;
            }
        };
        self.endpoint_error = None;
        let default_id = unsafe {
            self.enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
        }
        .ok()
        .and_then(|device| device_id(&device).ok());
        let present: BTreeSet<_> = devices.iter().map(|(id, _)| id.clone()).collect();
        self.endpoints.retain(|id, endpoint| {
            present.contains(id) && !endpoint.invalidated.load(Ordering::Acquire)
        });
        self.application_errors.retain(|id, _| present.contains(id));
        self.endpoint_sources.clear();
        for (id, device) in devices {
            self.endpoint_sources.push(CaptureSource {
                name: friendly_name(&device).unwrap_or_else(|_| id.clone()),
                is_default: default_id.as_ref() == Some(&id),
                id: id.clone(),
            });
            let is_new = !self.endpoints.contains_key(&id);
            if is_new {
                match EndpointWatch::new(
                    Arc::from(id.as_str()),
                    &device,
                    self.signal.clone(),
                    self.created.clone(),
                ) {
                    Ok(endpoint) => {
                        self.endpoints.insert(id.clone(), endpoint);
                    }
                    Err(error) => {
                        self.application_errors.insert(id, error.to_string());
                        continue;
                    }
                }
            }
            if is_new || manual {
                let endpoint = self.endpoints.get_mut(&id).expect("endpoint was inserted");
                match endpoint.read_existing_sessions() {
                    Ok(()) => {
                        self.application_errors.remove(&id);
                    }
                    Err(error) => {
                        self.application_errors.insert(id, error.to_string());
                    }
                }
            }
        }
        self.endpoint_sources.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
    }

    fn read_devices(&self) -> Result<Vec<(String, IMMDevice)>> {
        let collection = unsafe {
            self.enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
        }
        .map_err(|error| failure("Read watched playback endpoints", error))?;
        let count = unsafe { collection.GetCount() }
            .map_err(|error| failure("Count watched playback endpoints", error))?;
        let mut devices = Vec::new();
        for index in 0..count.min(128) {
            let device = unsafe { collection.Item(index) }
                .map_err(|error| failure("Read watched playback endpoint", error))?;
            devices.push((device_id(&device)?, device));
        }
        Ok(devices)
    }

    fn drain_created(&mut self) {
        // Hold every callback-provided reference until it can be processed on
        // this MTA thread. The enumerator is not a replacement for this queue.
        for _ in 0..4096 {
            let Ok(created) = self.created_sessions.try_recv() else {
                return;
            };
            if let Some(endpoint) = self.endpoints.get_mut(created.endpoint.as_ref()) {
                if let Err(error) = endpoint.add_session(created.control.0) {
                    self.application_errors
                        .insert(created.endpoint.to_string(), error.to_string());
                }
            }
        }
        if !self.created_sessions.is_empty() {
            self.signal.notify(APPLICATIONS);
        }
    }

    fn publish(&mut self, updates: &watch::Sender<SourceSnapshot>, force: bool) {
        for endpoint in self.endpoints.values_mut() {
            endpoint.sessions.retain(|_, session| {
                !session.disconnected.load(Ordering::Acquire)
                    && matches!(unsafe { session.control.GetState() }, Ok(state) if state != AudioSessionStateExpired)
            });
        }
        let mut errors: Vec<_> = self.application_errors.values().cloned().collect();
        if let Some(error) = &self.endpoint_error {
            errors.push(error.clone());
        }
        let mut applications = match sources_from_sessions(
            self.endpoints
                .values()
                .flat_map(|endpoint| endpoint.sessions.values().map(|session| &session.control)),
        ) {
            Ok(sources) => sources,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        };
        let present: BTreeSet<_> = applications
            .iter()
            .map(|source| source.id.clone())
            .collect();
        self.processes.retain(|id, _| present.contains(id));
        applications.retain(|source| {
            if self.processes.contains_key(&source.id) {
                return true;
            }
            match ProcessExitWait::new(&source.id, self.signal.clone()) {
                Ok(wait) => {
                    self.processes.insert(source.id.clone(), wait);
                    true
                }
                Err(CaptureError::ApplicationExited) => false,
                Err(error) => {
                    errors.push(error.to_string());
                    true
                }
            }
        });
        let snapshot = SourceSnapshot {
            endpoints: self.endpoint_sources.clone(),
            applications,
            endpoint_error: self.endpoint_error.clone(),
            application_error: (!errors.is_empty()).then(|| errors.join("; ")),
        };
        if force {
            // A manual refresh must finish even when the lists are unchanged.
            updates.send_replace(snapshot);
        } else {
            updates.send_if_modified(|current| {
                if *current == snapshot {
                    false
                } else {
                    *current = snapshot;
                    true
                }
            });
        }
    }
}

impl Drop for WatchWorker {
    fn drop(&mut self) {
        // Close the queue's writer gate first. Every accepted callback finishes
        // enqueueing before the receiver or its COM references can be dropped.
        self.signal.close();
        if self.registered {
            let _ = unsafe {
                self.enumerator
                    .UnregisterEndpointNotificationCallback(&self.notification)
            };
            self.registered = false;
        }
        self.endpoints.clear();
        self.processes.clear();
        while self.signal.has_callbacks() {
            let _ = self.wake.recv();
        }
        while self.created_sessions.try_recv().is_ok() {}
    }
}

struct EndpointWatch {
    manager: IAudioSessionManager2,
    notification: IAudioSessionNotification,
    sessions: BTreeMap<String, WatchedSession>,
    signal: Arc<ChangeSignal>,
    invalidated: Arc<AtomicBool>,
}

impl EndpointWatch {
    fn new(
        endpoint: Arc<str>,
        device: &IMMDevice,
        signal: Arc<ChangeSignal>,
        created: Sender<CreatedSession>,
    ) -> Result<Self> {
        let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|error| failure("Watch application audio sessions", error))?;
        let notification: IAudioSessionNotification = SessionCreatedObserver {
            endpoint,
            signal: signal.clone(),
            created,
        }
        .into();
        // Register before enumerating. GetCount below enables notifications;
        // all later callbacks retain their own session-control references.
        unsafe { manager.RegisterSessionNotification(&notification) }
            .map_err(|error| failure("Register application session notifications", error))?;
        Ok(Self {
            manager,
            notification,
            sessions: BTreeMap::new(),
            signal,
            invalidated: Arc::new(AtomicBool::new(false)),
        })
    }

    fn read_existing_sessions(&mut self) -> Result<()> {
        let sessions = unsafe { self.manager.GetSessionEnumerator() }.map_err(|error| {
            self.invalidated.store(true, Ordering::Release);
            failure("Read watched application sessions", error)
        })?;
        let count = unsafe { sessions.GetCount() }.map_err(|error| {
            self.invalidated.store(true, Ordering::Release);
            failure("Enable application session notifications", error)
        })?;
        let mut first_error = None;
        for session in self.sessions.values_mut() {
            let result = session.register();
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        for index in 0..count.clamp(0, 1024) {
            let result = unsafe { sessions.GetSession(index) }
                .map_err(|error| failure("Read watched application session", error))
                .and_then(|control| self.add_session(control));
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        // Never discard retained callback sessions absent from this enumerator.
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn add_session(&mut self, control: IAudioSessionControl) -> Result<()> {
        if unsafe { control.GetState() }.ok() == Some(AudioSessionStateExpired) {
            return Ok(());
        }
        let control = control
            .cast::<IAudioSessionControl2>()
            .map_err(|error| failure("Read watched application session identity", error))?;
        let id = allocated_string(
            unsafe { control.GetSessionInstanceIdentifier() }
                .map_err(|error| failure("Identify watched application session", error))?,
        )?;
        let session = self.sessions.entry(id).or_insert_with(|| {
            WatchedSession::new(control, self.signal.clone(), self.invalidated.clone())
        });
        // Keep the control even if subscribing to its state fails. A manual
        // refresh can retry registration without losing callback-only sessions.
        session.register()
    }
}

impl Drop for EndpointWatch {
    fn drop(&mut self) {
        let _ = unsafe {
            self.manager
                .UnregisterSessionNotification(&self.notification)
        };
        self.sessions.clear();
    }
}

struct WatchedSession {
    control: IAudioSessionControl2,
    notification: IAudioSessionEvents,
    disconnected: Arc<AtomicBool>,
    registered: bool,
}

impl WatchedSession {
    fn new(
        control: IAudioSessionControl2,
        signal: Arc<ChangeSignal>,
        endpoint_invalidated: Arc<AtomicBool>,
    ) -> Self {
        let disconnected = Arc::new(AtomicBool::new(false));
        let notification: IAudioSessionEvents = SessionObserver {
            signal,
            disconnected: disconnected.clone(),
            endpoint_invalidated,
        }
        .into();
        Self {
            control,
            notification,
            disconnected,
            registered: false,
        }
    }

    fn register(&mut self) -> Result<()> {
        if !self.registered {
            unsafe {
                self.control
                    .RegisterAudioSessionNotification(&self.notification)
            }
            .map_err(|error| failure("Watch application session state", error))?;
            self.registered = true;
        }
        Ok(())
    }
}

impl Drop for WatchedSession {
    fn drop(&mut self) {
        if self.registered {
            let _ = unsafe {
                self.control
                    .UnregisterAudioSessionNotification(&self.notification)
            };
        }
    }
}

struct CreatedSession {
    endpoint: Arc<str>,
    control: MtaSession,
}

struct MtaSession(IAudioSessionControl);

// These controls come from the session manager's MTA callback, not from an
// IAudioClient::GetService call with thread-specific release requirements.
// They move only into our MTA worker in the same COM apartment. The callback
// adds a reference; the worker alone uses and ultimately releases that reference.
unsafe impl Send for MtaSession {}

#[implement(IAudioSessionNotification)]
struct SessionCreatedObserver {
    endpoint: Arc<str>,
    signal: Arc<ChangeSignal>,
    created: Sender<CreatedSession>,
}

impl IAudioSessionNotification_Impl for SessionCreatedObserver_Impl {
    fn OnSessionCreated(&self, session: Ref<IAudioSessionControl>) -> windows::core::Result<()> {
        let Some(_entry) = self.signal.enter() else {
            return Ok(());
        };
        if let Some(control) = session.cloned() {
            // Unbounded send never waits for a receiver. Shutdown keeps the
            // receiver alive until this entry leaves, including during unwinding.
            let _ = self.created.send(CreatedSession {
                endpoint: self.endpoint.clone(),
                control: MtaSession(control),
            });
            self.signal.notify(APPLICATIONS);
        }
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct SessionObserver {
    signal: Arc<ChangeSignal>,
    disconnected: Arc<AtomicBool>,
    endpoint_invalidated: Arc<AtomicBool>,
}

impl IAudioSessionEvents_Impl for SessionObserver_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        self.signal.notify(APPLICATIONS);
        Ok(())
    }

    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _: u32,
        _: *const f32,
        _: u32,
        _: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> windows::core::Result<()> {
        self.signal.notify(APPLICATIONS);
        Ok(())
    }

    fn OnStateChanged(&self, _: AudioSessionState) -> windows::core::Result<()> {
        self.signal.notify(APPLICATIONS);
        Ok(())
    }

    fn OnSessionDisconnected(
        &self,
        reason: AudioSessionDisconnectReason,
    ) -> windows::core::Result<()> {
        self.disconnected.store(true, Ordering::Release);
        if reason == DisconnectReasonDeviceRemoval
            || reason == DisconnectReasonFormatChanged
            || reason == DisconnectReasonServerShutdown
        {
            self.endpoint_invalidated.store(true, Ordering::Release);
            self.signal.notify(DEVICES | APPLICATIONS);
        } else {
            self.signal.notify(APPLICATIONS);
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceObserver {
    signal: Arc<ChangeSignal>,
}

impl IMMNotificationClient_Impl for DeviceObserver_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        self.signal.notify(DEVICES);
        Ok(())
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.signal.notify(DEVICES);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.signal.notify(DEVICES);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        _: ERole,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        if flow == eRender || flow == eAll {
            self.signal.notify(DEVICES);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        self.signal.notify(DEVICES);
        Ok(())
    }
}

struct ProcessExitWait {
    wait: PTP_WAIT,
    // The context and process handle outlive disarming and draining callbacks.
    _signal: Arc<ChangeSignal>,
    _process: ApplicationProcess,
}

impl ProcessExitWait {
    fn new(source_id: &str, signal: Arc<ChangeSignal>) -> Result<Self> {
        let process = ApplicationProcess::open(source_id)?;
        let wait = unsafe {
            CreateThreadpoolWait(
                Some(process_exited),
                Some(Arc::as_ptr(&signal).cast_mut().cast()),
                None,
            )
        }
        .map_err(|error| failure("Watch application process exit", error))?;
        // NULL timeout waits only for the process handle; it is not a timer.
        unsafe { SetThreadpoolWait(wait, Some(process.handle()), None) };
        Ok(Self {
            wait,
            _signal: signal,
            _process: process,
        })
    }
}

unsafe extern "system" fn process_exited(
    _: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
    _: PTP_WAIT,
    _: u32,
) {
    let signal = unsafe { &*context.cast::<ChangeSignal>() };
    signal.notify(APPLICATIONS);
}

impl Drop for ProcessExitWait {
    fn drop(&mut self) {
        unsafe {
            SetThreadpoolWait(self.wait, None, None);
            WaitForThreadpoolWaitCallbacks(self.wait, true);
            CloseThreadpoolWait(self.wait);
        }
    }
}

fn pin_notification_module(callback: &IMMNotificationClient) -> Result<()> {
    static PINNED: OnceLock<std::result::Result<(), i32>> = OnceLock::new();
    let address = callback.vtable().OnDeviceAdded as *const () as *const u16;
    let result = PINNED.get_or_init(|| {
        let mut module = HMODULE::default();
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                PCWSTR(address),
                &mut module,
            )
        }
        .map_err(|error| error.code().0)
    });
    result.map_err(|code| CaptureError::Windows {
        operation: "Keep source notification callback module loaded",
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ApplicationId;
    use std::os::windows::io::AsRawHandle;
    use std::sync::Mutex;
    use std::time::Instant;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{E_NOTIMPL, FILETIME, HANDLE};
    use windows::Win32::Media::Audio::{
        eCapture, AudioSessionStateActive, AudioSessionStateInactive,
        DisconnectReasonSessionDisconnected, IAudioSessionControl_Impl,
    };
    use windows::Win32::System::Threading::GetProcessTimes;

    #[implement(IAudioSessionControl)]
    struct TestSession {
        released_on: Arc<Mutex<Option<thread::ThreadId>>>,
    }

    impl Drop for TestSession {
        fn drop(&mut self) {
            *self.released_on.lock().unwrap() = Some(thread::current().id());
        }
    }

    impl IAudioSessionControl_Impl for TestSession_Impl {
        fn GetState(&self) -> windows::core::Result<AudioSessionState> {
            Ok(AudioSessionStateInactive)
        }

        fn GetDisplayName(&self) -> windows::core::Result<PWSTR> {
            Err(windows::core::Error::from_hresult(E_NOTIMPL))
        }

        fn SetDisplayName(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
            Ok(())
        }

        fn GetIconPath(&self) -> windows::core::Result<PWSTR> {
            Err(windows::core::Error::from_hresult(E_NOTIMPL))
        }

        fn SetIconPath(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
            Ok(())
        }

        fn GetGroupingParam(&self) -> windows::core::Result<GUID> {
            Ok(GUID::zeroed())
        }

        fn SetGroupingParam(&self, _: *const GUID, _: *const GUID) -> windows::core::Result<()> {
            Ok(())
        }

        fn RegisterAudioSessionNotification(
            &self,
            _: Ref<IAudioSessionEvents>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn UnregisterAudioSessionNotification(
            &self,
            _: Ref<IAudioSessionEvents>,
        ) -> windows::core::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn session_callback_retains_its_session_until_worker_release() {
        let (signal, wake) = ChangeSignal::channel();
        let (created, receiver) = unbounded();
        let observer: IAudioSessionNotification = SessionCreatedObserver {
            endpoint: Arc::from("test-endpoint"),
            signal: signal.clone(),
            created,
        }
        .into();
        let released_on = Arc::new(Mutex::new(None));
        let session: IAudioSessionControl = TestSession {
            released_on: released_on.clone(),
        }
        .into();
        unsafe { observer.OnSessionCreated(&session) }.unwrap();
        drop(session);
        assert!(released_on.lock().unwrap().is_none());
        wake.recv().unwrap();
        assert_eq!(signal.take_changes(), APPLICATIONS);
        let worker_id = thread::spawn(move || {
            let _apartment = ComApartment::initialize().unwrap();
            let created = receiver.recv().unwrap();
            assert_eq!(created.endpoint.as_ref(), "test-endpoint");
            assert_eq!(
                unsafe { created.control.0.GetState() }.unwrap(),
                AudioSessionStateInactive
            );
            drop(created);
            thread::current().id()
        })
        .join()
        .unwrap();
        assert_eq!(*released_on.lock().unwrap(), Some(worker_id));
    }

    #[test]
    fn notification_callbacks_signal_only_relevant_list_changes() {
        let (signal, wake) = ChangeSignal::channel();
        let disconnected = Arc::new(AtomicBool::new(false));
        let endpoint_invalidated = Arc::new(AtomicBool::new(false));
        let session: IAudioSessionEvents = SessionObserver {
            signal: signal.clone(),
            disconnected: disconnected.clone(),
            endpoint_invalidated: endpoint_invalidated.clone(),
        }
        .into();
        let devices: IMMNotificationClient = DeviceObserver {
            signal: signal.clone(),
        }
        .into();
        unsafe {
            session.OnStateChanged(AudioSessionStateActive).unwrap();
            session
                .OnSessionDisconnected(DisconnectReasonSessionDisconnected)
                .unwrap();
            devices.OnDeviceAdded(PCWSTR::null()).unwrap();
            devices.OnDeviceRemoved(PCWSTR::null()).unwrap();
            devices
                .OnDeviceStateChanged(PCWSTR::null(), DEVICE_STATE_ACTIVE)
                .unwrap();
            devices
                .OnDefaultDeviceChanged(eRender, eMultimedia, PCWSTR::null())
                .unwrap();
        }
        assert_eq!(wake.len(), 1);
        wake.recv().unwrap();
        assert_eq!(signal.take_changes(), APPLICATIONS | DEVICES);
        assert!(disconnected.load(Ordering::Acquire));
        assert!(!endpoint_invalidated.load(Ordering::Acquire));
        unsafe {
            session
                .OnSimpleVolumeChanged(0.5, false, std::ptr::null())
                .unwrap();
            devices
                .OnDefaultDeviceChanged(eCapture, eMultimedia, PCWSTR::null())
                .unwrap();
        }
        assert!(
            wake.is_empty(),
            "volume changes must not re-enumerate the list"
        );
        unsafe {
            session
                .OnSessionDisconnected(DisconnectReasonServerShutdown)
                .unwrap();
        }
        wake.recv().unwrap();
        assert_eq!(signal.take_changes(), APPLICATIONS | DEVICES);
        assert!(endpoint_invalidated.load(Ordering::Acquire));
        signal.close();
        wake.recv().unwrap();
        unsafe { devices.OnDeviceAdded(PCWSTR::null()) }.unwrap();
        assert!(wake.is_empty());
    }

    struct Child(std::process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn process_exit_notification_is_cancellable_and_follows_the_owned_lifetime() {
        let mut child = Child(
            std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[Threading.Thread]::Sleep(60000)",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetProcessTimes(
                HANDLE(child.0.as_raw_handle()),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        }
        .unwrap();
        let id = ApplicationId {
            process_id: child.0.id(),
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        }
        .source_id();
        let (signal, wake) = ChangeSignal::channel();
        let pending = ProcessExitWait::new(&id, signal.clone()).unwrap();
        assert!(wake.is_empty());
        let stopped = Instant::now();
        drop(pending);
        assert!(stopped.elapsed() < Duration::from_secs(1));
        assert!(child.0.try_wait().unwrap().is_none());
        let wait = ProcessExitWait::new(&id, signal.clone()).unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        wake.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(signal.take_changes(), APPLICATIONS);
        drop(wait);
        assert!(matches!(
            ProcessExitWait::new(&id, signal),
            Err(CaptureError::ApplicationExited)
        ));
    }

    /// This verifies real device-notification registration even on a runner
    /// without a physical endpoint. It does not simulate actual app playback.
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires Windows with the Windows Audio service"]
    async fn source_notifications_start_refresh_and_stop() {
        println!("Source notifications: starting real Windows subscription");
        let mut sources = tokio::task::spawn_blocking(crate::watch_sources)
            .await
            .unwrap()
            .expect("register real Windows source notifications");
        let initial = sources.recv().await.expect("initial source snapshot");
        println!(
            "Source notifications: initial snapshot has {} endpoints and {} applications",
            initial.endpoints.len(),
            initial.applications.len()
        );
        // Two explicit refreshes exercise the unchanged-snapshot completion
        // case on an empty runner as well as callers using populated desktops.
        for _ in 0..2 {
            sources.refresh().expect("request a manual refresh");
            tokio::time::timeout(Duration::from_secs(3), sources.recv())
                .await
                .expect("manual refresh did not publish a completion")
                .expect("manual refresh stopped the watcher");
        }
        let stopped = Instant::now();
        drop(sources);
        assert!(stopped.elapsed() < Duration::from_millis(1500));
        let restarted = tokio::task::spawn_blocking(crate::watch_sources)
            .await
            .unwrap()
            .expect("restart a source subscription after dropping it");
        drop(restarted);
        println!("Real source notifications passed: registration, initial snapshot, explicit refresh and bounded teardown. No hardware session-change claim.");
    }
}
