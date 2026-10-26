//! Device identification for subscription panels with device limits
//! (Remnawave and others): the `x-hwid` family of headers sent by Happ/Incy.

use sha2::{Digest, Sha256};

/// What the client tells the subscription server about this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub hwid: String,
    pub os: String,
    pub os_version: String,
    pub model: String,
}

impl DeviceInfo {
    /// Information about this computer. `hwid` is empty when Windows does not
    /// expose a machine identifier; callers then use a stored random one.
    pub fn current() -> Self {
        Self {
            hwid: machine_guid()
                .map(|guid| hwid_from_machine_guid(&guid))
                .unwrap_or_default(),
            os: "Windows".to_owned(),
            os_version: os_version(),
            model: device_model().unwrap_or_else(|| "Windows PC".to_owned()),
        }
    }

    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            ("x-hwid", self.hwid.clone()),
            ("x-device-os", self.os.clone()),
        ];
        if !self.os_version.is_empty() {
            headers.push(("x-ver-os", self.os_version.clone()));
        }
        if !self.model.is_empty() {
            headers.push(("x-device-model", self.model.clone()));
        }
        headers
    }
}

/// Stable identifier derived from the Windows machine GUID. The GUID itself
/// never leaves the computer: only a salted SHA-256 prefix is sent.
pub fn hwid_from_machine_guid(guid: &str) -> String {
    let digest = Sha256::digest(format!("universe-hwid:{}", guid.trim().to_lowercase()));
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A fresh random identifier, used after the user resets the HWID.
pub fn random_hwid() -> String {
    let seed = format!("{:?}{}", std::time::SystemTime::now(), std::process::id());
    let digest = Sha256::digest(format!("universe-hwid-random:{seed}"));
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(target_os = "windows")]
fn registry_string(path: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
    };
    let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let path = wide(path);
    let name = wide(name);
    let mut buffer = [0_u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let length = (size as usize / 2).saturating_sub(1).min(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..length])
        .trim()
        .to_owned();
    (!value.is_empty()).then_some(value)
}

fn machine_guid() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        registry_string(r"SOFTWARE\Microsoft\Cryptography", "MachineGuid")
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::read_to_string("/etc/machine-id")
            .ok()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
    }
}

fn device_model() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        registry_string(r"HARDWARE\DESCRIPTION\System\BIOS", "SystemProductName")
            .filter(|model| !model.eq_ignore_ascii_case("To Be Filled By O.E.M."))
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
        let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if unsafe { GetVersionExW(&mut version) } != 0 {
            return format!(
                "{}.{}.{}",
                version.dwMajorVersion, version.dwMinorVersion, version.dwBuildNumber
            );
        }
        String::new()
    }
    #[cfg(not(target_os = "windows"))]
    {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwid_is_stable_hex_and_case_insensitive() {
        let hwid = hwid_from_machine_guid("4C4C4544-0000-1000-8000-B4C04F313233");
        assert_eq!(hwid.len(), 32);
        assert!(hwid.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            hwid,
            hwid_from_machine_guid(" 4c4c4544-0000-1000-8000-b4c04f313233 ")
        );
        assert_ne!(hwid, hwid_from_machine_guid("another-guid"));
    }

    #[test]
    fn random_hwids_differ() {
        let first = random_hwid();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_ne!(first, random_hwid());
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn headers_include_hwid_and_device() {
        let device = DeviceInfo {
            hwid: "abc".to_owned(),
            os: "Windows".to_owned(),
            os_version: "10.0.26200".to_owned(),
            model: "MS-7E56".to_owned(),
        };
        let headers = device.headers();
        assert!(headers.contains(&("x-hwid", "abc".to_owned())));
        assert!(headers.contains(&("x-ver-os", "10.0.26200".to_owned())));
        assert!(headers.contains(&("x-device-model", "MS-7E56".to_owned())));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reads_this_machine() {
        let device = DeviceInfo::current();
        assert_eq!(device.hwid.len(), 32);
        assert!(device.os_version.starts_with("10."));
    }
}
