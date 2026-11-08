//! Same-thread WASAPI FFI. Only an independently Job-gated worker may construct a producer.
//! No unsafe Send/Sync: Rc markers keep all COM objects and the permit on their owner thread.
use crate::{
    format::{
        Encoding, Format
    }, packet::Track, Result
};
use std::{
    marker::PhantomData, rc::Rc, os::windows::io::{
        OwnedHandle, FromRawHandle, AsRawHandle
    }, ptr
};
use windows::{
    core::{
        Interface, PCWSTR, PWSTR
    }, Win32::{
        Foundation::*, Media::{
            Audio::*, KernelStreaming::KSDATAFORMAT_SUBTYPE_PCM, Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        }, System::{
            Com::*, Com::StructuredStorage::{
                PROPVARIANT, PropVariantClear, PropVariantToStringAlloc
            }, Threading::*
        }, Devices::FunctionDiscovery::PKEY_Device_FriendlyName
    }
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(crate) struct Handle(OwnedHandle);
impl Handle {
    // SAFETY: every caller transfers exactly one newly created non-null kernel handle.
    pub(crate) unsafe fn take(h: HANDLE) -> Self {
        Self(unsafe {
            OwnedHandle::from_raw_handle(h.0)
        })
    }
    pub(crate) fn raw(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }
}
pub(crate) struct Event(pub(crate) Handle);
impl Event {
    pub(crate) fn create(manual: bool, name: Option<&str>) -> Result<Self> {
        let wide: Vec<u16>=name.unwrap_or("").encode_utf16().chain([0]).collect();
        // SAFETY: no inheritable attributes; optional UTF-16 name remains live through call.
        let handle = unsafe {
            SetLastError(ERROR_SUCCESS);
            CreateEventW(None, manual, false, if name.is_some() {
                PCWSTR(wide.as_ptr())
            } else {
                PCWSTR::null()
            }).map_err(err)?
        };
        // SAFETY: immediately inspect creation status before any other Windows call.
        let exists = unsafe {
            GetLastError()
        } == ERROR_ALREADY_EXISTS;
        let event = Self(unsafe {
            Handle::take(handle)
        });
        if exists {
            return Err("Owned stop event name already exists".into());
        }
        Ok(event)
    }
    pub(crate) fn signal(&self) -> Result<()> {
        // SAFETY: this owned Event remains open through the call.
        unsafe {
            SetEvent(self.0.raw()).map_err(err)
        }
    }
    pub(crate) fn open(name: &str) -> Result<Self> {
        let w: Vec<_>=name.encode_utf16().chain([0]).collect();
        // SAFETY: UTF-16 name lives through call; returned handle is newly owned, not inherited.
        unsafe {
            OpenEventW(SYNCHRONIZATION_SYNCHRONIZE|EVENT_MODIFY_STATE, false, PCWSTR(w.as_ptr())).map(|h|Self(Handle::take(h))).map_err(err)
        }
    }
    pub(crate) fn set(&self) -> Result<bool> {
        // SAFETY: owned valid Event, nonblocking observation.
        match unsafe {
            WaitForSingleObject(self.0.raw(), 0)
        }
        {
            WAIT_OBJECT_0 => Ok(true), WAIT_TIMEOUT => Ok(false), _ => Err("Stop event observation failed".into())
        }
    }
}
pub(crate) fn duplicate_stdin() -> Result<std::fs::File> {
    let input = unsafe {
        // SAFETY: duplicate the borrowed stdin handle; File exclusively owns only the duplicate.
        let mut duplicate = HANDLE::default();
        let process = GetCurrentProcess();
        DuplicateHandle(
            process,
            HANDLE(std::io::stdin().as_raw_handle()),
            process,
            &mut duplicate,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
        .map_err(err)?;
        std::fs::File::from_raw_handle(duplicate.0)
    };
    Ok(input)
}
pub(crate) fn pipe_control(input: &std::fs::File, stop: bool) -> Result<crate::worker::Control> {
    let mut available = 0;
    // SAFETY: exact duplicate of the unbuffered gate pipe, query only, no competing reader.
    let peek = unsafe {
        windows::Win32::System::Pipes::PeekNamedPipe(
            HANDLE(input.as_raw_handle()),
            None,
            0,
            None,
            Some(&mut available),
            None,
        )
    };
    let eof = match peek {
        Ok(()) => false,
        Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_BROKEN_PIPE.0) => true,
        Err(e) => return Err(err(e)),
    };
    crate::worker::pipe_control(stop, available, eof)
}
/// Exact pipe and named Job ownership shared by audio and the read-only helper.
pub(crate) struct OwnedGate {
    stop: Event,
    input: std::fs::File,
    _thread: PhantomData<Rc<()>>,
}
impl OwnedGate {
    pub(crate) fn receive<T>(
        job_name: &str,
        stop_name: &str,
        read: impl FnOnce(&mut std::fs::File) -> Result<T>,
    ) -> Result<(Self, T)> {
        if [job_name, stop_name]
            .iter()
            .any(|n| n.is_empty() || n.contains('\0') || n.len() > 512)
        {
            return Err("Invalid owned object name".into());
        }
        let mut input = duplicate_stdin()?;
        let init = read(&mut input)?;
        let w: Vec<u16> = job_name.encode_utf16().chain([0]).collect();
        // SAFETY: query the exact named Job against this process; pseudo-handle is never closed.
        unsafe {
            let job = Handle::take(
                windows::Win32::System::JobObjects::OpenJobObjectW(
                    0x0004,
                    false,
                    PCWSTR(w.as_ptr()),
                )
                .map_err(err)?,
            );
            let mut inside = windows::core::BOOL(0);
            windows::Win32::System::JobObjects::IsProcessInJob(
                GetCurrentProcess(),
                Some(job.raw()),
                &mut inside,
            )
            .map_err(err)?;
            if !inside.as_bool() {
                return Err("Worker is not in its parent's independent audio Job".into());
            }
        }
        let gate = Self {
            stop: Event::open(stop_name)?,
            input,
            _thread: PhantomData,
        };
        if gate.control()? != crate::worker::Control::Live {
            return Err("Owned child gate cancelled".into());
        }
        Ok((gate, init))
    }
    pub(crate) fn control(&self) -> Result<crate::worker::Control> {
        pipe_control(&self.input, self.stop.set()?)
    }
}
pub struct WorkerPermit {
    gate: OwnedGate,
    frequency: u64,
}
impl WorkerPermit {
    pub fn receive(
        job_name: &str,
        stop_name: &str,
        nonce: &str,
    ) -> Result<(Self, crate::protocol::ValidatedInit)> {
        let (gate, init) = OwnedGate::receive(job_name, stop_name, |input| {
            let init = crate::protocol::read_init(input, nonce).map_err(err)?;
            crate::wav::reject_links(&init.request().output_dir)?;
            if init.request().output_dir.canonicalize().map_err(err)? != init.request().output_dir {
                return Err("Worker output directory must be canonical".into());
            }
            Ok(init)
        })?;
        let mut frequency = 0;
        // SAFETY: writable QPC storage; positive frequency precedes any native audio access.
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
                .map_err(err)?;
        }
        if frequency <= 0 {
            return Err("QPC frequency is not positive".into());
        }
        let permit = Self {
            gate,
            frequency: frequency as u64,
        };
        if permit.control()? != crate::worker::Control::Live {
            return Err("Worker ownership gate cancelled".into());
        }
        Ok((permit, init))
    }
    pub(crate) fn control(&self) -> Result<crate::worker::Control> {
        self.gate.control()
    }
    pub(crate) fn stamp(&self) -> Result<crate::protocol::ClockStamp> {
        let mut qpc = 0;
        // SAFETY: output is initialized by successful QPC; never reinterpret negative ticks.
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceCounter(&mut qpc).map_err(err)?;
        }
        if qpc < 0 {
            return Err("QPC returned negative ticks".into());
        }
        Ok(crate::protocol::ClockStamp {
            qpc: qpc as u64,
            frequency: self.frequency,
        })
    }
}
pub(crate) struct Apartment(PhantomData<Rc<()>>);
impl Apartment {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: this non-Send guard balances every successful HRESULT, including S_FALSE, on this thread.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(err)?;
        }
        Ok(Self(PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: guard is constructed only after successful initialization.
        unsafe {
            CoUninitialize();
        }
    }
}
pub(crate) struct Allocation<T>(pub(crate) *mut T);
impl<T> Drop for Allocation<T> {
    fn drop(&mut self) {
        // SAFETY: pointers originate from GetMixFormat/GetId/PropVariantToStringAlloc, all CoTaskMem allocations.
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
struct Property(PROPVARIANT);
impl Drop for Property {
    fn drop(&mut self) {
        // SAFETY: initialized GetValue output; no borrowed value survives clear.
        unsafe {
            let _ = PropVariantClear(&mut self.0);
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    System, Microphone
}
impl Kind {
    fn flow(self) -> EDataFlow {
        match self {
            Self::System => eRender, Self::Microphone => eCapture
        }
    }
}
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub kind: Kind, pub id: String, pub display_name: String
}
pub(crate) fn identity(device: &IMMDevice) -> Result<String> {
    // SAFETY: GetId allocates a NUL-terminated ID; copy before freeing.
    unsafe {
        let id = Allocation(device.GetId().map_err(err)?.0);
        PWSTR(id.0).to_string().map_err(err)
    }
}
/// Enumeration is library-only in this preparation and requires the same owned worker gate.
pub fn enumerate(_permit: &WorkerPermit, kind: Kind) -> Result<Vec<Endpoint>> {
    let _com = Apartment::new()?;
    // SAFETY: all interfaces/property outputs stay on this apartment thread and drop before it.
    unsafe {
        let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
        let devices = e.EnumAudioEndpoints(kind.flow(), DEVICE_STATE_ACTIVE).map_err(err)?;
        let mut out = Vec::new();
        for i in 0..devices.GetCount().map_err(err)? {
            let d = devices.Item(i).map_err(err)?;
            let id = identity(&d)?;
            let store = d.OpenPropertyStore(STGM_READ).map_err(err)?;
            let prop = Property(store.GetValue(&PKEY_Device_FriendlyName).map_err(err)?);
            let label = Allocation(PropVariantToStringAlloc(&prop.0).map_err(err)?.0);
            out.push(Endpoint {
                kind, id, display_name: PWSTR(label.0).to_string().map_err(err)?
            });
        }
        Ok(out)
    }
}
pub(crate) fn mix_format(p: *const WAVEFORMATEX) -> Result<Format> {
    if p.is_null() {
        return Err("Endpoint returned no mix format".into());
    }
    // SAFETY: WASAPI GetMixFormat owns at least WAVEFORMATEX and cbSize extension bytes until freed.
    // Packed fields are copied by value; read_unaligned never creates an unaligned reference.
    unsafe {
        let f = ptr::read_unaligned(p);
        let tag = f.wFormatTag;
        let mut valid = f.wBitsPerSample;
        let mut mask = 0;
        let encoding = match tag {
            1 => Encoding::Pcm, 3 => Encoding::Float, 0xfffe => {
                if f.cbSize<22 {
                    return Err("Truncated extensible mix format".into());
                }
                let x = ptr::read_unaligned(p.cast::<WAVEFORMATEXTENSIBLE>());
                valid = x.Samples.wValidBitsPerSample;
                mask = x.dwChannelMask;
                if x.dwChannelMask != 0 && x.dwChannelMask.count_ones() != u32::from(f.nChannels) {
                    return Err("Mix format channel mask mismatch".into());
                }
                let subtype = x.SubFormat;
                if subtype == KSDATAFORMAT_SUBTYPE_PCM {
                    Encoding::Pcm
                }
                else if subtype == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                    Encoding::Float
                } else {
                    return Err("Unsupported mix subtype".into());
                }
            }, _ => return Err("Unsupported mix format tag".into())
        };
        let format = Format {
            rate: f.nSamplesPerSec, channels: f.nChannels, bits: f.wBitsPerSample, valid_bits: valid, align: f.nBlockAlign, encoding
        }
        .validate()?;
        format.validate_mask(mask)?;
        if f.nAvgBytesPerSec != format.rate.checked_mul(u32::from(format.align)).ok_or("Mix byte rate overflow")? {
            return Err("Mix byte rate mismatch".into());
        }
        Ok(format)
    }
}
pub use crate::worker::{Wake, PacketPoll, NativeFault};
pub struct Producer<'a> {
    capture: IAudioCaptureClient,
    client: IAudioClient,
    _device: IMMDevice,
    _enumerator: IMMDeviceEnumerator,
    ready: Event,
    permit: &'a WorkerPermit,
    format: Format,
    id: String,
    kind: Kind,
    started: bool,
    _com: Apartment,
}
impl<'a> Producer<'a> {
    pub fn open(permit: &'a WorkerPermit, id: &str, kind: Kind) -> Result<Self> {
        if id.is_empty() || id.contains('\0') {
            return Err("Select an exact endpoint ID".into());
        }
        ensure_live(permit)?;
        let com = Apartment::new()?;
        ensure_live(permit)?;
        let w: Vec<_>=id.encode_utf16().chain([0]).collect();
        // SAFETY: COM interfaces/format/event live through initialization on this non-Send owner thread.
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
            ensure_live(permit)?;
            let device = enumerator.GetDevice(PCWSTR(w.as_ptr())).map_err(err)?;
            ensure_live(permit)?;
            if identity(&device)? != id || device.GetState().map_err(err)? != DEVICE_STATE_ACTIVE || device.cast::<IMMEndpoint>().map_err(err)?.GetDataFlow().map_err(err)? != kind.flow() {
                return Err("Selected endpoint ID, flow or active state changed".into());
            }
            ensure_live(permit)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(err)?;
            ensure_live(permit)?;
            let mix = Allocation(client.GetMixFormat().map_err(err)?);
            let format = mix_format(mix.0)?;
            ensure_live(permit)?;
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK|if kind == Kind::System {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            };
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, mix.0, None).map_err(err)?;
            ensure_live(permit)?;
            let ready = Event::create(false, None)?;
            client.SetEventHandle(ready.0.raw()).map_err(err)?;
            ensure_live(permit)?;
            let capture = client.GetService::<IAudioCaptureClient>().map_err(err)?;
            ensure_live(permit)?;
            Ok(Self {
                capture, client, _device: device, _enumerator: enumerator, ready, permit, format, id:id.into(), kind, started: false, _com: com
            })
        }
    }
    pub fn format(&self) -> Format { self.format }

    pub fn start(&mut self) -> std::result::Result<(), NativeFault> {
        if self.started {
            return Err(NativeFault::fatal("Audio client already started"));
        }
        if self.permit.gate.stop.set().map_err(NativeFault::fatal)? {
            return Err(NativeFault::fatal("Stop requested before audio startup"));
        }
        // SAFETY: exact endpoint and mix format remain borrowed on the invocation apartment thread.
        unsafe {
            if identity(&self._device).map_err(NativeFault::fatal)? != self.id
                || self._device.GetState().map_err(native_fault)? != DEVICE_STATE_ACTIVE
                || self._device.cast::<IMMEndpoint>().map_err(native_fault)?.GetDataFlow().map_err(native_fault)? != self.kind.flow() {
                return Err(NativeFault::fatal("Selected endpoint ID, flow or active state changed before Start"));
            }
            ensure_live(self.permit).map_err(NativeFault::fatal)?;
            let mix=Allocation(self.client.GetMixFormat().map_err(native_fault)?);
            if mix_format(mix.0).map_err(NativeFault::fatal)? != self.format {
                return Err(NativeFault::fatal("Selected endpoint mix format changed before Start"));
            }
            ensure_live(self.permit).map_err(NativeFault::fatal)?;
            self.client.Start().map_err(native_fault)?;
        }
        self.started = true;
        Ok(())
    }
    pub fn wait(&self, timeout_ms: u32) -> Result<Wake> {
        if timeout_ms>100 {
            return Err("Audio wait exceeds finite poll budget".into());
        }
        let handles = [self.permit.gate.stop.0.raw(), self.ready.0.raw()];
        // SAFETY: both handles remain open; stop is first, so simultaneous signals prefer stop.
        let r = unsafe {
            WaitForMultipleObjects(&handles, false, timeout_ms)
        };
        if r == WAIT_OBJECT_0 {
            Ok(Wake::Stop)
        }
        else if r.0 == WAIT_OBJECT_0.0+1 {
            Ok(Wake::Audio)
        }
        else if r == WAIT_TIMEOUT {
            Ok(Wake::Timeout)
        } else {
            Err("Audio event wait failed".into())
        }
    }
    pub fn packet(&mut self, track: &mut Track) -> std::result::Result<PacketPoll, NativeFault> {
        if self.permit.gate.stop.set().map_err(NativeFault::fatal)? {
            return Ok(PacketPoll::Stopped);
        }
        if track.format != self.format {
            return Err(NativeFault::fatal("Track/native format mismatch"));
        }
        let mut frames = 0;
        let mut data = std::mem::MaybeUninit::<*mut u8>::uninit();
        let mut flags = std::mem::MaybeUninit::<u32>::uninit();
        let mut device = std::mem::MaybeUninit::<u64>::uninit();
        let mut qpc = std::mem::MaybeUninit::<u64>::uninit();
        // SAFETY: output storage is writable; successful-empty leaves every output except frame count unread.
        unsafe {
            self.capture.GetBuffer(data.as_mut_ptr(), &mut frames, flags.as_mut_ptr(), Some(device.as_mut_ptr()), Some(qpc.as_mut_ptr())).map_err(native_fault)?;
        }
        if frames == 0 {
            return Ok(PacketPoll::Empty);
        }
        // SAFETY: a nonempty successful acquisition initializes flags/positions; data is unread for SILENT.
        let result = ( || unsafe {
            let flags = flags.assume_init();
            let d = device.assume_init();
            let t = qpc.assume_init();
            let len = self.format.byte_len(frames)?;
            if flags&!7 != 0 {
                return Err("Unknown native packet flags".into());
            }
            let bytes = if flags&crate::packet::SILENT != 0 {
                None
            } else {
                let p = data.assume_init();
                if p.is_null() {
                    return Err("Non-silent native packet pointer is null".into());
                }
                Some(std::slice::from_raw_parts(p, len))
            };
            track.admit(frames, flags, d, t, bytes)
        })();
        // SAFETY: exactly one release for this nonempty acquisition, same thread, before any subsequent call/wait.
        let release = unsafe {
            self.capture.ReleaseBuffer(frames).map_err(err)
        };
        if let Err(e) = release {
            track.ended = Some(e.clone());
            return Err(NativeFault::fatal(e));
        }
        if let Err(e) = result {
            track.ended = Some(e.clone());
            return Err(NativeFault::fatal(e));
        }
        Ok(PacketPoll::Accepted)
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.started {
            // Consume the sole Stop attempt even on HRESULT failure; Drop must not retry it.
            self.started = false;
            // SAFETY: same initialized client owner thread; hard hangs require parent Job containment.
            unsafe {
                self.client.Stop().map_err(err)?;
            }
        }
        Ok(())
    }
}
impl Drop for Producer<'_> {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn native_fault(e: windows::core::Error) -> NativeFault { NativeFault::classify(e.code().0, e.to_string()) }
/// Invocation-local adapter: all native ownership borrows the thread-bound permit, never a thread.
pub struct NativeOps<'a> { permit: &'a WorkerPermit, producer: Option<Producer<'a>> }
impl crate::worker::WorkerOps for NativeOps<'_> {
    fn check_control(&mut self) -> Result<crate::worker::Control> { self.permit.control() }
    fn open_exact(&mut self, request: &crate::protocol::TrackRequest) -> std::result::Result<Format, NativeFault> {
        if self.producer.is_some() { return Err(NativeFault::fatal("Native producer already owned")); }
        request.validate().map_err(|e|NativeFault::fatal(e.to_string()))?;
        let kind = match request.kind { crate::protocol::TrackKind::Microphone=>Kind::Microphone, crate::protocol::TrackKind::System=>Kind::System };
        let producer = Producer::open(self.permit, &request.endpoint_id, kind).map_err(NativeFault::fatal)?;
        let format = producer.format(); self.producer=Some(producer); Ok(format)
    }
    fn start(&mut self) -> std::result::Result<(), NativeFault> {
        self.producer.as_mut().ok_or_else(||NativeFault::fatal("Native producer absent"))?.start()
    }
    fn wait(&mut self, ms: u32) -> std::result::Result<Wake, NativeFault> {
        self.producer.as_ref().ok_or_else(||NativeFault::fatal("Native producer absent"))?.wait(ms).map_err(NativeFault::fatal)
    }
    fn packet(&mut self, track: &mut Track) -> std::result::Result<PacketPoll, NativeFault> {
        self.producer.as_mut().ok_or_else(||NativeFault::fatal("Native producer absent"))?.packet(track)
    }
    fn stop_and_drop(&mut self) -> Result<()> {
        if let Some(mut producer)=self.producer.take() {
            let stopped=producer.stop();
            // Same-thread field drops release capture/client/device/enumerator before Apartment::drop.
            drop(producer);
            stopped
        } else { Ok(()) }
    }
    fn stamp(&mut self) -> Result<crate::protocol::ClockStamp> { self.permit.stamp() }
    fn emit(&mut self, frame: &crate::protocol::EventV1) -> Result<()> {
        use std::io::Write;
        let bytes=crate::protocol::encode_event(frame).map_err(err)?;
        let mut out=std::io::stdout().lock();out.write_all(&bytes).map_err(err)?;out.flush().map_err(err)
    }
}
pub(crate) fn dispatch_worker(job: &str, stop: &str, nonce: &str) -> i32 {
    match WorkerPermit::receive(job,stop,nonce) {
        Ok((permit,init)) => crate::worker::run_worker(&mut NativeOps { permit:&permit, producer:None },init).code,
        Err(_) => 64,
    }
}

pub(crate) fn ensure_live(permit: &WorkerPermit) -> Result<()> {
    if permit.control()? == crate::worker::Control::Live { Ok(()) }
    else { Err("Parent control ended native admission".into()) }
}

/// Only a delegated, exact-owned discovery child may enter this apartment.
pub(crate) fn discover_render(
    permit: &crate::proof::guard::helper::AdmittedDiscovery,
) -> Result<Vec<crate::proof::guard::helper::RenderEndpoint>> {
    use crate::proof::guard::helper::{collect_inventory, RenderEndpoint, RenderInventoryOps};
    struct Inventory<'a> {
        devices: IMMDeviceCollection,
        permit: &'a crate::proof::guard::helper::AdmittedDiscovery,
    }
    impl RenderInventoryOps for Inventory<'_> {
        fn count(&mut self) -> Result<u32> {
            self.permit.check()?;
            // SAFETY: collection is owned on its initialized apartment thread.
            unsafe { self.devices.GetCount().map_err(err) }
        }
        fn item(&mut self, index: u32) -> Result<RenderEndpoint> {
            self.permit.check()?;
            // SAFETY: in-range collection item and COM-owned strings/variants are released on this thread.
            unsafe {
                let device = self.devices.Item(index).map_err(err)?;
                let id = bounded_string(Allocation(device.GetId().map_err(err)?.0), 2048)?;
                let state = device.GetState().map_err(err)?.0;
                let endpoint: IMMEndpoint = device.cast().map_err(err)?;
                let flow = endpoint.GetDataFlow().map_err(err)?.0;
                let store = device.OpenPropertyStore(STGM_READ).map_err(err)?;
                let property = Property(store.GetValue(&PKEY_Device_FriendlyName).map_err(err)?);
                let label = bounded_string(
                    Allocation(PropVariantToStringAlloc(&property.0).map_err(err)?.0),
                    512,
                )?;
                Ok(RenderEndpoint {
                    id,
                    label,
                    state,
                    flow,
                })
            }
        }
    }
    permit.check()?;
    let _apartment = Apartment::new()?;
    // SAFETY: only the enumerator and read-only active render collection are created; no client activation.
    let items = unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(err)?;
        collect_inventory(&mut Inventory { devices, permit })?
    };
    permit.check()?;
    Ok(items)
}
fn bounded_string(value: Allocation<u16>, cap: usize) -> Result<String> {
    if value.0.is_null() {
        return Err("Endpoint returned a null string".into());
    }
    let mut units = Vec::new();
    for i in 0..=cap {
        // SAFETY: GetId/PropVariantToStringAlloc guarantee a NUL-terminated owned allocation;
        // stop at its NUL and never scan more than the maximum possible accepted UTF-16 units.
        let unit = unsafe { *value.0.add(i) };
        if unit == 0 {
            let text = String::from_utf16(&units).map_err(err)?;
            if text.len() > cap {
                return Err("Endpoint UTF-8 string cap exceeded".into());
            }
            return Ok(text);
        }
        units.push(unit);
    }
    Err("Endpoint string cap exceeded".into())
}
