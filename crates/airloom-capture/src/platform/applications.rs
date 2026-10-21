//! Windows process loopback; activation and capture never widen to system audio.

use super::{allocated_string, enumerator, failure, ComApartment, Event};
use crate::application::{same_application_parent, ApplicationId};
use crate::{CaptureError, CaptureSource, Result};
use crossbeam_channel::{bounded, Sender};
use std::collections::{BTreeMap, BTreeSet};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use windows::core::{implement, Interface, Ref, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    E_OUTOFMEMORY, FILETIME, HMODULE, S_OK, WAIT_FAILED, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::{
    eRender, ActivateAudioInterfaceAsync, AudioSessionStateExpired,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioClient, IAudioSessionControl2,
    IAudioSessionManager2, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    DEVICE_STATE_ACTIVE, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{CoTaskMemAlloc, BLOB, CLSCTX_ALL};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use windows::Win32::System::Variant::VT_BLOB;

#[derive(Clone)]
struct ProcessMetadata {
    id: ApplicationId,
    image_path: String,
}

fn creation_time(handle: &Event) -> Result<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|error| failure("Read application identity", error))?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn process_is_alive(handle: &Event) -> Result<()> {
    match unsafe { WaitForSingleObject(handle.0, 0) } {
        WAIT_TIMEOUT => Ok(()),
        WAIT_FAILED => Err(failure(
            "Check application lifetime",
            windows::core::Error::from_thread(),
        )),
        _ => Err(CaptureError::ApplicationExited),
    }
}

fn open_process(process_id: u32) -> Result<Event> {
    if process_id == 0 {
        return Err(CaptureError::ApplicationExited);
    }
    unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            process_id,
        )
    }
    .map(Event)
    .map_err(|error| {
        // ERROR_INVALID_PARAMETER means the selected process no longer exists.
        if error.code().0 == 0x80070057u32 as i32 {
            CaptureError::ApplicationExited
        } else {
            failure("Open selected application", error)
        }
    })
}

fn process_metadata(process_id: u32, handle: &Event) -> Result<ProcessMetadata> {
    process_is_alive(handle)?;
    let created = creation_time(handle)?;
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(|error| failure("Read application name", error))?;
    process_is_alive(handle)?;
    Ok(ProcessMetadata {
        id: ApplicationId {
            process_id,
            created,
        },
        image_path: String::from_utf16_lossy(&path[..length as usize]),
    })
}

fn executable_name(image_path: &str) -> &str {
    image_path.rsplit(['\\', '/']).next().unwrap_or(image_path)
}

fn is_shared_host(image_path: &str) -> bool {
    matches!(
        executable_name(image_path).to_ascii_lowercase().as_str(),
        "explorer.exe"
            | "svchost.exe"
            | "dllhost.exe"
            | "rundll32.exe"
            | "applicationframehost.exe"
            | "runtimebroker.exe"
            | "wwahost.exe"
    )
}

fn parent_snapshot() -> Result<BTreeMap<u32, u32>> {
    let snapshot = Event(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|error| failure("List application process trees", error))?,
    );
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut parents = BTreeMap::new();
    if unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_err() {
        return Ok(parents);
    }
    loop {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        if parents.len() >= 32_768 || unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(parents)
}

fn read_metadata(
    process_id: u32,
    cache: &mut BTreeMap<u32, Option<ProcessMetadata>>,
) -> Option<ProcessMetadata> {
    cache
        .entry(process_id)
        .or_insert_with(|| {
            let handle = open_process(process_id).ok()?;
            process_metadata(process_id, &handle).ok()
        })
        .clone()
}

fn application_root(
    process_id: u32,
    parents: &BTreeMap<u32, u32>,
    cache: &mut BTreeMap<u32, Option<ProcessMetadata>>,
) -> Option<ProcessMetadata> {
    let mut current = read_metadata(process_id, cache)?;
    if is_shared_host(&current.image_path) || process_id == std::process::id() {
        return None;
    }
    let mut visited = BTreeSet::from([process_id]);
    for _ in 0..32 {
        let Some(parent_id) = parents.get(&current.id.process_id) else {
            break;
        };
        if *parent_id == std::process::id() || !visited.insert(*parent_id) {
            break;
        }
        let Some(parent) = read_metadata(*parent_id, cache) else {
            break;
        };
        if is_shared_host(&parent.image_path)
            || !same_application_parent(
                current.id,
                &current.image_path,
                parent.id,
                &parent.image_path,
            )
        {
            break;
        }
        current = parent;
    }
    Some(current)
}

pub(crate) fn enumerate_applications() -> Result<Vec<CaptureSource>> {
    thread::Builder::new()
        .name("airloom-applications".into())
        .spawn(|| {
            let _apartment = ComApartment::initialize()?;
            let parents = parent_snapshot()?;
            let mut metadata = BTreeMap::new();
            let mut applications: BTreeMap<ApplicationId, String> = BTreeMap::new();
            let devices = unsafe { enumerator()?.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
                .map_err(|error| failure("Find application audio endpoints", error))?;
            let count = unsafe { devices.GetCount() }
                .map_err(|error| failure("Count application audio endpoints", error))?;
            for index in 0..count.min(128) {
                let Ok(device) = (unsafe { devices.Item(index) }) else {
                    continue;
                };
                let Ok(manager): windows::core::Result<IAudioSessionManager2> =
                    (unsafe { device.Activate(CLSCTX_ALL, None) })
                else {
                    continue;
                };
                let Ok(sessions) = (unsafe { manager.GetSessionEnumerator() }) else {
                    continue;
                };
                let Ok(count) = (unsafe { sessions.GetCount() }) else {
                    continue;
                };
                for index in 0..count.clamp(0, 1024) {
                    let Ok(session) = (unsafe { sessions.GetSession(index) }) else {
                        continue;
                    };
                    // Keep paused/inactive sessions selectable; only expired ones
                    // disappear. The app may need to play once to create a session.
                    let Ok(state) = (unsafe { session.GetState() }) else {
                        continue;
                    };
                    if state == AudioSessionStateExpired {
                        continue;
                    }
                    let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    if unsafe { control.IsSystemSoundsSession() } == S_OK {
                        continue;
                    }
                    let Ok(pid) = (unsafe { control.GetProcessId() }) else {
                        continue;
                    };
                    let Some(root) = application_root(pid, &parents, &mut metadata) else {
                        continue;
                    };
                    // Recheck the root before publishing a choice; selection will
                    // check the same lifetime again and retain its process handle.
                    if ApplicationProcess::open(&root.id.source_id()).is_err() {
                        continue;
                    }
                    let display_name = unsafe { session.GetDisplayName() }
                        .ok()
                        .and_then(|value| allocated_string(value).ok())
                        .filter(|name| !name.trim().is_empty() && !name.starts_with('@'));
                    applications.entry(root.id).or_insert_with(|| {
                        display_name.unwrap_or_else(|| executable_name(&root.image_path).to_owned())
                    });
                }
            }
            let mut sources: Vec<_> = applications
                .into_iter()
                .map(|(id, name)| CaptureSource {
                    id: id.source_id(),
                    name: format!("{name} · {}", id.process_id),
                    is_default: false,
                })
                .collect();
            sources.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then(a.id.cmp(&b.id))
            });
            Ok(sources)
        })
        .map_err(CaptureError::WorkerStart)?
        .join()
        .map_err(|_| CaptureError::WorkerPanicked)?
}

pub(super) struct ApplicationProcess {
    handle: Event,
    metadata: ProcessMetadata,
}

impl ApplicationProcess {
    pub(super) fn open(source_id: &str) -> Result<Self> {
        let expected = ApplicationId::parse(source_id)?;
        let handle = open_process(expected.process_id)?;
        let metadata = process_metadata(expected.process_id, &handle)?;
        if metadata.id != expected {
            return Err(CaptureError::ApplicationExited);
        }
        Ok(Self { handle, metadata })
    }

    pub(super) fn ensure_alive(&self) -> Result<()> {
        process_is_alive(&self.handle)
    }

    pub(super) fn name(&self) -> String {
        format!(
            "{} · {}",
            executable_name(&self.metadata.image_path),
            self.metadata.id.process_id
        )
    }

    pub(super) fn activate(&self, cancelled: &AtomicBool) -> Result<IAudioClient> {
        self.ensure_alive()?;
        let payload = Arc::new(ActivationPayload::new(self.metadata.id.process_id)?);
        let (sender, ready) = bounded(1);
        // windows-rs' implement macro supplies IAgileObject and a free-threaded
        // marshaler. The callback only signals; the audio client remains owned
        // and used by the MTA capture worker.
        let callback: IActivateAudioInterfaceCompletionHandler = ActivationCompletion {
            sender,
            _payload: payload.clone(),
        }
        .into();
        keep_activation_module_loaded(&callback)?;
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(payload.variant.as_ref()),
                &callback,
            )
        }
        .map_err(|error| failure("Activate application audio capture", error))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.ensure_alive()?;
            if cancelled.load(Ordering::Acquire) {
                return Err(CaptureError::WorkerStopped);
            }
            if ready.recv_timeout(Duration::from_millis(20)).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                // Windows retains the callback until completion, including its
                // immutable parameter allocations. Late completion never
                // references this stack or an already-dropped worker channel.
                return Err(CaptureError::Timeout("application audio activation"));
            }
        }
        let mut activation_result = HRESULT(0x80004005u32 as i32);
        let mut unknown = None;
        unsafe { operation.GetActivateResult(&mut activation_result, &mut unknown) }
            .map_err(|error| failure("Read application activation result", error))?;
        activation_result
            .ok()
            .map_err(|error| failure("Activate selected application", error))?;
        unknown
            .ok_or_else(|| {
                CaptureError::Conversion("Windows returned no application audio client".into())
            })?
            .cast()
            .map_err(|error| failure("Get application audio client", error))
    }
}

fn keep_activation_module_loaded(
    callback: &IActivateAudioInterfaceCompletionHandler,
) -> Result<()> {
    static PINNED: OnceLock<std::result::Result<(), i32>> = OnceLock::new();
    // Activation has no cancellation API. After our bounded wait returns,
    // Windows may still hold a callback into airloom_core.dll. Owning its Rust
    // data is insufficient if the native host then destroys EngineBridge and
    // calls FreeLibrary. Pin the module containing the actual callback thunk
    // before registering it, so late completion cannot execute unloaded code.
    //
    // This deliberately lasts until process exit. It neither keeps the process
    // running nor extends capture: workers and clients still stop normally.
    // Airloom's updater already waits for the complete process to exit before
    // replacing files, at which point Windows releases this module as well.
    let result = PINNED.get_or_init(|| {
        let mut module = HMODULE::default();
        let address = callback.vtable().ActivateCompleted as *const () as *const u16;
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
        operation: "Keep application capture callback module loaded",
        code,
    })
}

struct ActivationPayload {
    // windows-rs implements Drop for PROPVARIANT in its extension module; Drop
    // calls PropVariantClear, which owns/frees this VT_BLOB with CoTaskMemFree.
    // Its data therefore uses CoTaskMemAlloc, never a borrowed Rust Box. The
    // variant and its blob remain immutable and alive with the COM callback,
    // including after the worker's activation deadline has expired.
    variant: Box<PROPVARIANT>,
}

// The only raw pointer is an immutable reference to our stable COM parameter
// allocation. Neither structure contains apartment-bound interfaces or handles.
unsafe impl Send for ActivationPayload {}
unsafe impl Sync for ActivationPayload {}

impl ActivationPayload {
    fn new(process_id: u32) -> Result<Self> {
        let parameters = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: process_id,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let allocation =
            unsafe { CoTaskMemAlloc(std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>()) }
                .cast::<AUDIOCLIENT_ACTIVATION_PARAMS>();
        if allocation.is_null() {
            return Err(failure(
                "Allocate application activation parameters",
                windows::core::Error::from_hresult(E_OUTOFMEMORY),
            ));
        }
        unsafe { allocation.write(parameters) };
        let variant = Box::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: allocation.cast(),
                        },
                    },
                    ..Default::default()
                }),
            },
        });
        Ok(Self { variant })
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationCompletion {
    sender: Sender<()>,
    _payload: Arc<ActivationPayload>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationCompletion_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.sender.try_send(());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CaptureConfig, CaptureStream};

    #[test]
    fn activation_payload_retains_then_releases_its_owned_com_blob() {
        // This runs in the ordinary Windows tests, without an audio service.
        // A borrowed Rust allocation behind an owning VT_BLOB corrupts the
        // heap when the last payload reference is released.
        for process_id in 1..=64 {
            let payload = Arc::new(ActivationPayload::new(process_id).unwrap());
            let retained = payload.clone();
            drop(payload);
            let header = unsafe { &retained.variant.Anonymous.Anonymous };
            assert_eq!(header.vt, VT_BLOB);
            let blob = unsafe { header.Anonymous.blob };
            assert_eq!(
                blob.cbSize as usize,
                std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>()
            );
            let parameters = unsafe {
                blob.pBlobData
                    .cast::<AUDIOCLIENT_ACTIVATION_PARAMS>()
                    .read()
            };
            assert_eq!(
                parameters.ActivationType,
                AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
            );
            let process = unsafe { parameters.Anonymous.ProcessLoopbackParams };
            assert_eq!(process.TargetProcessId, process_id);
            assert_eq!(
                process.ProcessLoopbackMode,
                PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
            );
            drop(retained);
        }
    }

    struct Child(std::process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Exercises the real Windows API. The child has no audio stream, so this
    /// does not establish audible capture or exclusion of another playing app.
    #[test]
    #[ignore = "requires Windows build 20348+ with the Windows Audio service"]
    fn process_loopback_activation_and_lifetime() {
        println!("Process capture: spawning owned lifetime-test process");
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
                .expect("spawn an owned lifetime-test process"),
        );
        let handle = open_process(child.0.id()).expect("open owned test process");
        let metadata = process_metadata(child.0.id(), &handle).expect("read test process identity");
        println!("Process capture: process handle and creation identity read");
        let config = CaptureConfig {
            source_id: metadata.id.source_id(),
            ..Default::default()
        };
        let stale = ApplicationId {
            created: metadata.id.created + 1,
            ..metadata.id
        };
        assert!(matches!(
            ApplicationProcess::open(&stale.source_id()),
            Err(CaptureError::ApplicationExited)
        ));

        println!("Process capture: activating first real process-loopback client");
        let mut stream =
            CaptureStream::start(config.clone()).expect("real process loopback activation");
        println!("Process capture: first activation and client start completed");
        assert_eq!(stream.info().unwrap().source_id, config.source_id);
        assert!(matches!(
            stream.set_local_mute(true),
            Err(CaptureError::ApplicationMuteUnsupported)
        ));
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if let Some(chunk) = stream.read_timeout(Duration::from_millis(50)).unwrap() {
                assert!(
                    chunk
                        .samples
                        .iter()
                        .all(|sample| sample.is_finite() && *sample == 0.0),
                    "silent process {} produced nonzero audio: sequence={}, frames={}, peak={:?}, rms={:?}, first_nonzero={:?}",
                    config.source_id,
                    chunk.sequence,
                    chunk.frames(),
                    chunk.peak,
                    chunk.rms,
                    chunk
                        .samples
                        .iter()
                        .copied()
                        .filter(|sample| !sample.is_finite() || *sample != 0.0)
                        .take(16)
                        .collect::<Vec<_>>()
                );
            }
        }
        println!("Process capture: silent input and mute refusal verified; stopping");
        let stopped = Instant::now();
        stream.stop().expect("stop a live process capture");
        assert!(
            stopped.elapsed() < Duration::from_secs(1),
            "capture cancellation did not finish promptly"
        );

        println!("Process capture: bounded stop passed; activating second client");
        let mut stream =
            CaptureStream::start(config.clone()).expect("restart the same live process capture");
        println!("Process capture: second client started; ending owned process");
        child.0.kill().expect("end owned test process");
        child.0.wait().expect("wait for owned process exit");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match stream.read_timeout(Duration::from_millis(50)) {
                Err(CaptureError::ApplicationExited) => break,
                Err(error) => panic!("wrong terminal error after application exit: {error}"),
                Ok(_) if Instant::now() < deadline => {}
                Ok(_) => panic!("capture did not report the selected application's exit"),
            }
        }
        let _ = stream.stop();
        println!("Process capture: application exit observed; checking stale selection rejection");
        assert!(matches!(
            CaptureStream::start(config),
            Err(CaptureError::ApplicationExited)
        ));
        println!("Real process loopback passed: activation, identity rejection, silent input, mute refusal, bounded stop and process-exit handling. No endpoint/audio-isolation claim.");
    }
}
