use std::ffi::{CStr, CString, c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows::core::{PCSTR, PCWSTR, s};

use crate::install::{self, CLIENT_LIBRARY};
use crate::{Address, Error};

const PEER_SIZE: usize = 0x20;

#[repr(C)]
struct RawGeneric {
    kind: u32,
    size: u64,
    data: *const u8,
}

#[repr(C)]
struct RawMessage {
    generic: *const RawGeneric,
    status: *const c_void,
    peer_count: u64,
    peers: *const u8,
    unique_id: *const c_char,
    reference: *const c_char,
    source_system: *const c_char,
    source_module: *const c_char,
    source_session: *const c_char,
    source_user: *const c_char,
    source_process: u32,
    joined: u32,
    target_system: *const c_char,
    target_module: *const c_char,
    target_session: *const c_char,
    target_user: *const c_char,
    domain: *const c_char,
}

type Callback = unsafe extern "C" fn(*const RawMessage, *mut c_void);
type New = unsafe extern "C" fn() -> *mut c_void;
type Delete = unsafe extern "C" fn(*mut c_void);
type InterfaceWithConfig = unsafe extern "C" fn(u32, *const u16) -> *mut c_void;
type ReleaseInterface = unsafe extern "C" fn(*mut c_void);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    #[default]
    Default,
    Driver,
}
const INTERFACE_ROLE: u32 = 2;
const INTERFACE_SLOT: usize = 8;
type AddObserver =
    unsafe extern "C" fn(*mut c_void, Callback, *mut c_void, *const c_char, *const c_char);
type RemoveObserver = unsafe extern "C" fn(*mut c_void, Callback, *mut c_void);
type Post = unsafe extern "C" fn(*mut c_void, *const RawMessage) -> u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub source: Address,
    pub process: u32,
    pub kind: Option<u32>,
    pub data: Vec<u8>,
    pub peers: Vec<Address>,
}

type Handler = Box<dyn Fn(Message) + Send + Sync>;

pub struct Bus {
    library: HMODULE,
    handle: *mut c_void,
    delete: Delete,
    remove: RemoveObserver,
    post: Post,
    handler: *mut Handler,
    address: Address,
    system: CString,
    module: CString,
    settings: Option<&'static str>,
    sent: AtomicU64,
}

unsafe impl Send for Bus {}
unsafe impl Sync for Bus {}

unsafe fn text(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

unsafe fn peers(message: &RawMessage) -> Vec<Address> {
    if message.peers.is_null() {
        return Vec::new();
    }
    (0..message.peer_count as usize)
        .map(|index| unsafe {
            let peer = message.peers.add(index * PEER_SIZE).cast::<*const c_char>();
            Address {
                system: text(peer.read_unaligned()),
                module: text(peer.add(1).read_unaligned()),
            }
        })
        .collect()
}

unsafe extern "C" fn receive(message: *const RawMessage, context: *mut c_void) {
    if message.is_null() || context.is_null() {
        return;
    }
    let (message, handler) = unsafe { (&*message, &*context.cast::<Handler>()) };
    let (kind, data) = match unsafe { message.generic.as_ref() } {
        Some(generic) if !generic.data.is_null() => (
            Some(generic.kind),
            unsafe { std::slice::from_raw_parts(generic.data, generic.size as usize) }.to_vec(),
        ),
        Some(generic) => (Some(generic.kind), Vec::new()),
        None => (None, Vec::new()),
    };
    let received = unsafe {
        Message {
            id: text(message.unique_id),
            source: Address {
                system: text(message.source_system),
                module: text(message.source_module),
            },
            process: message.source_process,
            kind,
            data,
            peers: peers(message),
        }
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(received)));
}

unsafe fn symbol<T: Copy>(library: HMODULE, name: PCSTR) -> Result<T, Error> {
    let address =
        unsafe { GetProcAddress(library, name) }.ok_or(Error::Library("нет нужной функции"))?;
    Ok(unsafe { std::mem::transmute_copy(&address) })
}
unsafe fn attach_driver(
    handle: *mut c_void,
    with_config: InterfaceWithConfig,
    release: ReleaseInterface,
    configs: &[(&'static str, String)],
) -> Result<&'static str, Error> {
    let accepted = configs.iter().find_map(|(label, config)| {
        if config.contains('\0') {
            return None;
        }
        let config: Vec<u16> = config.encode_utf16().chain(Some(0)).collect();
        let driver = unsafe { with_config(INTERFACE_ROLE, config.as_ptr()) };
        (!driver.is_null()).then_some((*label, driver))
    });
    let Some((label, driver)) = accepted else {
        return Err(Error::Library(
            "канал драйвера не принял ни один вариант настроек",
        ));
    };
    let slot = unsafe { handle.byte_add(INTERFACE_SLOT).cast::<*mut c_void>() };
    let previous = unsafe { slot.read() };
    unsafe { slot.write(driver) };
    if !previous.is_null() {
        unsafe { release(previous) };
    }
    Ok(label)
}

impl Bus {
    pub fn join(
        address: Address,
        handler: impl Fn(Message) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        Self::join_on(Transport::Default, address, handler)
    }

    pub fn join_on(
        transport: Transport,
        address: Address,
        handler: impl Fn(Message) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let system = CString::new(address.system.as_str()).map_err(|_| Error::Name)?;
        let module = CString::new(address.module.as_str()).map_err(|_| Error::Name)?;
        let driver_config = match transport {
            Transport::Default => None,
            Transport::Driver => Some(install::driver_configs()?),
        };
        let path: Vec<u16> = install::bus_directory()?
            .join(CLIENT_LIBRARY)
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let library =
            unsafe { LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH) }
                .map_err(|_| Error::Library("не загрузилась"))?;
        let functions = unsafe {
            (|| {
                Ok::<_, Error>((
                    symbol::<New>(library, s!("messageBusNew"))?,
                    symbol::<Delete>(library, s!("messageBusDelete"))?,
                    symbol::<AddObserver>(library, s!("messageBusAddObserver"))?,
                    symbol::<RemoveObserver>(library, s!("messageBusRemoveObserver"))?,
                    symbol::<Post>(library, s!("messageBusPostMessage"))?,
                    symbol::<InterfaceWithConfig>(library, s!("getMessageBusInterfaceWithConfig"))?,
                    symbol::<ReleaseInterface>(library, s!("releaseMessageBusInterface"))?,
                ))
            })()
        };
        let (new, delete, add, remove, post, with_config, release) = match functions {
            Ok(functions) => functions,
            Err(error) => {
                let _ = unsafe { FreeLibrary(library) };
                return Err(error);
            }
        };
        let handle = unsafe { new() };
        if handle.is_null() {
            let _ = unsafe { FreeLibrary(library) };
            return Err(Error::Library("не создала подключение"));
        }
        let settings = match driver_config.as_deref() {
            None => None,
            Some(configs) => {
                match unsafe { attach_driver(handle, with_config, release, configs) } {
                    Ok(label) => Some(label),
                    Err(error) => {
                        unsafe { delete(handle) };
                        let _ = unsafe { FreeLibrary(library) };
                        return Err(error);
                    }
                }
            }
        };
        let handler: *mut Handler = Box::into_raw(Box::new(Box::new(handler)));
        unsafe {
            add(
                handle,
                receive,
                handler.cast(),
                system.as_ptr(),
                module.as_ptr(),
            )
        };
        Ok(Self {
            library,
            handle,
            delete,
            remove,
            post,
            handler,
            address,
            system,
            module,
            settings,
            sent: AtomicU64::new(0),
        })
    }

    pub fn settings(&self) -> Option<&'static str> {
        self.settings
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    pub fn post(&self, target: &Address, kind: u32, data: &[u8]) -> Result<(), Error> {
        self.send(target, kind, data, None)
    }

    pub fn reply(&self, request: &Message, kind: u32, data: &[u8]) -> Result<(), Error> {
        self.send(&request.source, kind, data, Some(&request.id))
    }

    fn send(
        &self,
        target: &Address,
        kind: u32,
        data: &[u8],
        reference: Option<&str>,
    ) -> Result<(), Error> {
        let reference = reference
            .map(CString::new)
            .transpose()
            .map_err(|_| Error::Name)?;
        let target_system = CString::new(target.system.as_str()).map_err(|_| Error::Name)?;
        let target_module = CString::new(target.module.as_str()).map_err(|_| Error::Name)?;
        let number = self.sent.fetch_add(1, Ordering::Relaxed);
        let unique = CString::new(format!(
            "rewind-{}-{}-{number}",
            std::process::id(),
            self.address.module
        ))
        .map_err(|_| Error::Name)?;
        let generic = RawGeneric {
            kind,
            size: data.len() as u64,
            data: data.as_ptr(),
        };
        let message = RawMessage {
            generic: &generic,
            status: ptr::null(),
            peer_count: 0,
            peers: ptr::null(),
            unique_id: unique.as_ptr(),
            reference: reference.as_ref().map_or(ptr::null(), |text| text.as_ptr()),
            source_system: self.system.as_ptr(),
            source_module: self.module.as_ptr(),
            source_session: ptr::null(),
            source_user: ptr::null(),
            source_process: 0,
            joined: 0,
            target_system: target_system.as_ptr(),
            target_module: target_module.as_ptr(),
            target_session: ptr::null(),
            target_user: ptr::null(),
            domain: ptr::null(),
        };
        if unsafe { (self.post)(self.handle, &message) } & 0xFF == 0 {
            Err(Error::Rejected)
        } else {
            Ok(())
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        unsafe {
            (self.remove)(self.handle, receive, self.handler.cast());
            (self.delete)(self.handle);
            drop(Box::from_raw(self.handler));
            let _ = FreeLibrary(self.library);
        }
    }
}
