use super::{AppEvent, TrayAction};
use anyhow::{bail, Result};
use eframe::{egui, CreationContext};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::mpsc::Sender;
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::UpdateWindow;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    EnableMenuItem, GetCursorPos, GetWindowLongPtrW, IsWindowVisible, LoadIconW, ModifyMenuW,
    PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    TrackPopupMenu, CREATESTRUCTW, GWLP_USERDATA, HWND_MESSAGE, IDI_APPLICATION, MF_BYCOMMAND,
    MF_ENABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, SW_RESTORE, SW_SHOWNOACTIVATE, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NCCREATE,
    WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 73;
const TRAY_ICON_ID: u32 = 1;
const STATUS_ID: u32 = 10_001;
const OPEN_ID: u32 = 10_002;
const CONNECTION_ID: u32 = 10_003;
const SETTINGS_ID: u32 = 10_004;
const EXIT_ID: u32 = 10_005;

struct TrayWindowState {
    sender: Sender<AppEvent>,
    ctx: egui::Context,
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    main_hwnd: HWND,
}

impl TrayWindowState {
    fn dispatch(&self, action: TrayAction) {
        unsafe {
            match action {
                TrayAction::Show | TrayAction::Settings => {
                    ShowWindow(self.main_hwnd, SW_RESTORE);
                    SetForegroundWindow(self.main_hwnd);
                    UpdateWindow(self.main_hwnd);
                }
                TrayAction::ToggleConnection { .. } | TrayAction::Exit => {
                    if IsWindowVisible(self.main_hwnd) == 0 {
                        ShowWindow(self.main_hwnd, SW_SHOWNOACTIVATE);
                        UpdateWindow(self.main_hwnd);
                    }
                }
            }
        }
        let _ = self.sender.send(AppEvent::Tray(action));
        self.ctx.request_repaint();
    }
}

pub struct AppTray {
    hwnd: HWND,
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    last_status: String,
    last_connection_text: String,
    last_connection_enabled: bool,
}

impl AppTray {
    pub fn new(sender: Sender<AppEvent>, ctx: egui::Context, main_hwnd: HWND) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(null());
            if instance.is_null() {
                bail!("не удалось получить дескриптор universe");
            }

            let class_name = wide("universe.NativeTray.Window");
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(tray_window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&window_class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                bail!("не удалось зарегистрировать окно системного трея");
            }

            let menu = CreatePopupMenu();
            if menu.is_null() {
                bail!("не удалось создать меню системного трея");
            }
            let initial_status = "universe · VPN отключён".to_owned();
            if !append_item(menu, STATUS_ID, &initial_status, MF_GRAYED)
                || AppendMenuW(menu, MF_SEPARATOR, 0, null()) == 0
                || !append_item(menu, OPEN_ID, "Открыть universe", MF_ENABLED)
                || !append_item(menu, CONNECTION_ID, "Подключить VPN", MF_ENABLED)
                || !append_item(menu, SETTINGS_ID, "Настройки", MF_ENABLED)
                || AppendMenuW(menu, MF_SEPARATOR, 0, null()) == 0
                || !append_item(menu, EXIT_ID, "Выйти", MF_ENABLED)
            {
                DestroyMenu(menu);
                bail!("не удалось заполнить меню системного трея");
            }

            let state = Box::new(TrayWindowState {
                sender,
                ctx,
                menu,
                main_hwnd,
            });
            let state_ptr = Box::into_raw(state);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance,
                state_ptr.cast(),
            );
            if hwnd.is_null() {
                DestroyMenu(menu);
                bail!("не удалось создать окно системного трея");
            }

            let mut icon = LoadIconW(instance, 1usize as _);
            if icon.is_null() {
                icon = LoadIconW(null_mut(), IDI_APPLICATION);
            }
            let mut notify = notify_data(hwnd);
            notify.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            notify.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
            notify.hIcon = icon;
            copy_wide_into(&mut notify.szTip, "universe");
            if Shell_NotifyIconW(NIM_ADD, &notify) == 0 {
                DestroyWindow(hwnd);
                DestroyMenu(menu);
                bail!("Windows не добавила значок universe в трей");
            }

            Ok(Self {
                hwnd,
                menu,
                last_status: initial_status,
                last_connection_text: "Подключить VPN".to_owned(),
                last_connection_enabled: true,
            })
        }
    }

    pub fn sync(&mut self, country: Option<&str>, busy: bool, can_connect: bool) {
        let status = country
            .map(|country| format!("universe · защищено · {country}"))
            .unwrap_or_else(|| "universe · VPN отключён".to_owned());
        let connection_text = if busy {
            "Операция выполняется…"
        } else if country.is_some() {
            "Отключить VPN"
        } else {
            "Подключить VPN"
        };
        let enabled = !busy && (country.is_some() || can_connect);

        unsafe {
            if status != self.last_status {
                modify_item(self.menu, STATUS_ID, &status, MF_GRAYED);
                self.last_status = status;
            }
            if connection_text != self.last_connection_text {
                modify_item(
                    self.menu,
                    CONNECTION_ID,
                    connection_text,
                    if enabled { MF_ENABLED } else { MF_GRAYED },
                );
                self.last_connection_text = connection_text.to_owned();
            }
            if enabled != self.last_connection_enabled {
                EnableMenuItem(
                    self.menu,
                    CONNECTION_ID,
                    MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
                );
                self.last_connection_enabled = enabled;
            }
        }
    }
}

pub fn main_window_handle(cc: &CreationContext<'_>) -> Option<HWND> {
    let handle = cc.window_handle().ok()?.as_raw();
    match handle {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

impl Drop for AppTray {
    fn drop(&mut self) {
        unsafe {
            let notify = notify_data(self.hwnd);
            Shell_NotifyIconW(NIM_DELETE, &notify);
            DestroyWindow(self.hwnd);
            DestroyMenu(self.menu);
        }
    }
}

unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        if create.is_null() {
            return 0;
        }
        let state = unsafe { (*create).lpCreateParams as *mut TrayWindowState };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize) };
        return 1;
    }

    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayWindowState };
    if message == TRAY_CALLBACK_MESSAGE && !state.is_null() {
        match lparam as u32 {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => unsafe {
                (*state).dispatch(TrayAction::Show);
            },
            WM_RBUTTONUP => unsafe {
                show_menu_and_dispatch(hwnd, &*state);
            },
            _ => {}
        }
        return 0;
    }

    if message == WM_NCDESTROY && !state.is_null() {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state));
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

unsafe fn show_menu_and_dispatch(hwnd: HWND, state: &TrayWindowState) {
    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return;
    }
    unsafe { SetForegroundWindow(hwnd) };
    let command = unsafe {
        TrackPopupMenu(
            state.menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            null(),
        ) as u32
    };
    let action = match command {
        OPEN_ID => Some(TrayAction::Show),
        CONNECTION_ID => Some(TrayAction::ToggleConnection {
            hide_after: unsafe { IsWindowVisible(state.main_hwnd) } == 0,
        }),
        SETTINGS_ID => Some(TrayAction::Settings),
        EXIT_ID => Some(TrayAction::Exit),
        _ => None,
    };
    if let Some(action) = action {
        state.dispatch(action);
    }
    unsafe { PostMessageW(hwnd, WM_NULL, 0, 0) };
}

unsafe fn append_item(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    id: u32,
    label: &str,
    state: u32,
) -> bool {
    let label = wide(label);
    unsafe { AppendMenuW(menu, MF_STRING | state, id as usize, label.as_ptr()) != 0 }
}

unsafe fn modify_item(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    id: u32,
    label: &str,
    state: u32,
) {
    let label = wide(label);
    unsafe {
        ModifyMenuW(
            menu,
            id,
            MF_BYCOMMAND | MF_STRING | state,
            id as usize,
            label.as_ptr(),
        );
    }
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn copy_wide_into<const N: usize>(target: &mut [u16; N], value: &str) {
    let encoded = value.encode_utf16().take(N.saturating_sub(1));
    for (slot, character) in target.iter_mut().zip(encoded) {
        *slot = character;
    }
}
