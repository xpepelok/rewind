use std::ffi::c_void;

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    WinHttpWriteData,
};
use windows::core::{HSTRING, PCWSTR};

const CHUNK: usize = 64 * 1024;
const RESOLVE_TIMEOUT_MS: i32 = 15_000;
const CONNECT_TIMEOUT_MS: i32 = 15_000;
const TRANSFER_TIMEOUT_MS: i32 = 120_000;
pub const BROWSER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

pub struct Response {
    pub status: u32,
    pub body: String,
    pub bytes: Vec<u8>,
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn opened(handle: *mut c_void, what: &str) -> Result<Handle, String> {
    if handle.is_null() {
        Err(format!(
            "{what}: {}",
            windows::core::Error::from_win32().message()
        ))
    } else {
        Ok(Handle(handle))
    }
}

fn failed(what: &str, error: windows::core::Error) -> String {
    format!("{what}: {}", error.message())
}

pub struct Session {
    handle: Handle,
}

impl Session {
    pub fn new(agent: &str) -> Result<Self, String> {
        let handle = opened(
            unsafe {
                WinHttpOpen(
                    &HSTRING::from(agent),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    0,
                )
            },
            "нет доступа к сети",
        )?;
        unsafe {
            WinHttpSetTimeouts(
                handle.0,
                RESOLVE_TIMEOUT_MS,
                CONNECT_TIMEOUT_MS,
                TRANSFER_TIMEOUT_MS,
                TRANSFER_TIMEOUT_MS,
            )
        }
        .map_err(|error| failed("таймауты", error))?;
        Ok(Self { handle })
    }

    pub fn get(&self, host: &str, path: &str) -> Result<Response, String> {
        self.send("GET", host, path, "", &[], &|_| {})
    }

    pub fn post(
        &self,
        host: &str,
        path: &str,
        headers: &str,
        body: &[u8],
        progress: &dyn Fn(f32),
    ) -> Result<Response, String> {
        self.send("POST", host, path, headers, body, progress)
    }

    fn send(
        &self,
        method: &str,
        host: &str,
        path: &str,
        headers: &str,
        body: &[u8],
        progress: &dyn Fn(f32),
    ) -> Result<Response, String> {
        let connection = opened(
            unsafe {
                WinHttpConnect(
                    self.handle.0,
                    &HSTRING::from(host),
                    INTERNET_DEFAULT_HTTPS_PORT,
                    0,
                )
            },
            "не удалось подключиться",
        )?;
        let request = opened(
            unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    &HSTRING::from(method),
                    &HSTRING::from(path),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    WINHTTP_FLAG_SECURE,
                )
            },
            "не удалось создать запрос",
        )?;
        let header_text: Vec<u16> = headers.encode_utf16().collect();
        unsafe {
            WinHttpSendRequest(
                request.0,
                (!header_text.is_empty()).then_some(header_text.as_slice()),
                None,
                0,
                body.len() as u32,
                0,
            )
        }
        .map_err(|error| failed("сервер не принял запрос", error))?;
        let mut sent = 0usize;
        for chunk in body.chunks(CHUNK) {
            let mut written = 0u32;
            unsafe {
                WinHttpWriteData(
                    request.0,
                    Some(chunk.as_ptr().cast()),
                    chunk.len() as u32,
                    &mut written,
                )
            }
            .map_err(|error| failed("обрыв при отправке", error))?;
            sent += chunk.len();
            progress(sent as f32 / body.len().max(1) as f32);
        }
        unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
            .map_err(|error| failed("нет ответа от сервера", error))?;
        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut size,
                std::ptr::null_mut(),
            )
        }
        .map_err(|error| failed("ответ без кода", error))?;
        let mut bytes = Vec::new();
        loop {
            let mut available = 0u32;
            unsafe { WinHttpQueryDataAvailable(request.0, &mut available) }
                .map_err(|error| failed("обрыв при чтении ответа", error))?;
            if available == 0 {
                break;
            }
            let start = bytes.len();
            bytes.resize(start + available as usize, 0);
            let mut read = 0u32;
            unsafe {
                WinHttpReadData(
                    request.0,
                    bytes[start..].as_mut_ptr().cast(),
                    available,
                    &mut read,
                )
            }
            .map_err(|error| failed("обрыв при чтении ответа", error))?;
            bytes.truncate(start + read as usize);
            if read == 0 {
                break;
            }
        }
        Ok(Response {
            status,
            body: String::from_utf8_lossy(&bytes).into_owned(),
            bytes,
        })
    }
}
