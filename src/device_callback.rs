use std::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
    sync::mpsc::{Sender, channel},
};
use winapi::shared::ntdef::LPCWSTR;
use winapi::um::combaseapi::{CLSCTX_ALL, CoInitializeEx, CoUninitialize};
use winapi::{
    shared::{
        guiddef::REFIID,
        minwindef::{DWORD, LPVOID, ULONG},
        winerror::{E_NOINTERFACE, S_OK},
        wtypes::PROPERTYKEY,
    },
    um::{
        combaseapi::CoCreateInstance,
        mmdeviceapi::{
            CLSID_MMDeviceEnumerator, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
            IMMNotificationClientVtbl,
        },
        objbase::COINIT_APARTMENTTHREADED,
        unknwnbase::{IUnknown, IUnknownVtbl},
    },
};

winapi::DEFINE_GUID! {IID_IUNKNOWN, 0x00000000, 0x0000, 0x0000, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46 }

winapi::DEFINE_GUID! {IID_IMMNOTIFICATION_CLIENT, 0x7991eec9, 0x7e89, 0x4d85, 0x83, 0x90, 0x6c, 0x70, 0x3c, 0xec, 0x60, 0xc0 }

winapi::DEFINE_GUID! { IID_IMMDEVICE_ENUMERATOR, 0xA95664D2, 0x9614, 0x4F35, 0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6 }

/// Static vtable for our COM object
static AUDIO_DEVICE_CALLBACK_VTBL: IMMNotificationClientVtbl = IMMNotificationClientVtbl {
    parent: IUnknownVtbl {
        QueryInterface: adc_query_interface,
        AddRef: adc_add_ref,
        Release: adc_release,
    },
    OnDeviceStateChanged: adc_on_device_state_changed,
    OnDeviceAdded: adc_on_device_added,
    OnDeviceRemoved: adc_on_device_removed,
    OnDefaultDeviceChanged: adc_on_default_device_changed,
    OnPropertyValueChanged: adc_on_property_value_changed,
};

#[repr(C)]
struct AudioDeviceCallback {
    vtable: *const IMMNotificationClientVtbl,
    ref_count: AtomicU32,
    tx: Sender<()>,
}

/// Blocks the current thread until *any* audio device change notification happens.
pub(crate) fn wait_for_audio_device_change() {
    unsafe {
        // Initialize COM apartment
        let hr = CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED);
        if hresult_failed(hr) {
            panic!("CoInitializeEx failed: 0x{:08x}", hr);
        }

        // Create IMMDeviceEnumerator
        let mut enumerator_ptr: *mut IMMDeviceEnumerator = ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_MMDeviceEnumerator,
            ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMMDEVICE_ENUMERATOR,
            &mut enumerator_ptr as *mut _ as *mut LPVOID,
        );
        if hresult_failed(hr) || enumerator_ptr.is_null() {
            CoUninitialize();
            panic!(
                "CoCreateInstance(CLSID_MMDeviceEnumerator) failed: 0x{:08x}",
                hr
            );
        }

        let enumerator = &mut *enumerator_ptr;

        // Create channel for notifications
        let (tx, rx) = channel::<()>();

        // Create our callback COM object
        let callback_ptr = create_audio_device_callback(tx);

        // Register callback
        let hr = (*enumerator).RegisterEndpointNotificationCallback(callback_ptr);
        if hresult_failed(hr) {
            // Drop our own ref
            adc_release(callback_ptr.cast());
            (*enumerator).Release();
            CoUninitialize();
            panic!("RegisterEndpointNotificationCallback failed: 0x{:08x}", hr);
        }

        // This is your "pause my app's thread until devices change"
        let _ = rx.recv();

        // Unregister callback
        let hr = (*enumerator).UnregisterEndpointNotificationCallback(callback_ptr);
        if hresult_failed(hr) {
            eprintln!(
                "UnregisterEndpointNotificationCallback failed: 0x{:08x}",
                hr
            );
        }

        // Release our own reference to the callback (COM has released its own)
        adc_release(callback_ptr.cast());

        // Release enumerator & COM
        (*enumerator).Release();
        CoUninitialize();
    }
}

unsafe fn create_audio_device_callback(tx: Sender<()>) -> *mut IMMNotificationClient {
    let obj = Box::new(AudioDeviceCallback {
        vtable: &AUDIO_DEVICE_CALLBACK_VTBL,
        ref_count: AtomicU32::new(1), // our own ref
        tx,
    });

    Box::into_raw(obj) as *mut IMMNotificationClient
}

/// Helper: simple HRESULT check
fn hresult_failed(hr: i32) -> bool {
    hr < 0
}

/// Cast from interface pointer to our struct
fn from_this<'a>(this: *mut IMMNotificationClient) -> &'a mut AudioDeviceCallback {
    unsafe { &mut *(this as *mut AudioDeviceCallback) }
}

/// IUnknown::QueryInterface
unsafe extern "system" fn adc_query_interface(
    this: *mut IUnknown,
    riid: REFIID,
    ppv: *mut *mut winapi::ctypes::c_void,
) -> i32 {
    if ppv.is_null() {
        return E_NOINTERFACE;
    }

    unsafe {
        *ppv = ptr::null_mut();
    }

    if riid == &IID_IUNKNOWN || riid == &IID_IMMNOTIFICATION_CLIENT {
        let adc = this as *mut winapi::ctypes::c_void;
        unsafe {
            *ppv = adc;
            adc_add_ref(this);
        }
        return S_OK;
    }

    E_NOINTERFACE
}

/// IUnknown::AddRef
unsafe extern "system" fn adc_add_ref(this: *mut IUnknown) -> ULONG {
    let adc = from_this(this.cast());
    adc.ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

/// IUnknown::Release
unsafe extern "system" fn adc_release(this: *mut IUnknown) -> ULONG {
    let adc = from_this(this.cast());
    let old = adc.ref_count.fetch_sub(1, Ordering::SeqCst);
    if old == 1 {
        // ref count is going to 0, free the object
        unsafe {
            drop(Box::from_raw(this as *mut AudioDeviceCallback));
        }
        0
    } else {
        old - 1
    }
}

/// IMMNotificationClient::OnDeviceStateChanged
unsafe extern "system" fn adc_on_device_state_changed(
    this: *mut IMMNotificationClient,
    _id: LPCWSTR,
    _new_state: DWORD,
) -> i32 {
    let adc = from_this(this);
    let _ = adc.tx.send(());
    S_OK
}

/// IMMNotificationClient::OnDeviceAdded
unsafe extern "system" fn adc_on_device_added(
    this: *mut IMMNotificationClient,
    _id: LPCWSTR,
) -> i32 {
    let adc = from_this(this);
    let _ = adc.tx.send(());
    S_OK
}

/// IMMNotificationClient::OnDeviceRemoved
unsafe extern "system" fn adc_on_device_removed(
    this: *mut IMMNotificationClient,
    _id: LPCWSTR,
) -> i32 {
    let adc = from_this(this);
    let _ = adc.tx.send(());
    S_OK
}

/// IMMNotificationClient::OnDefaultDeviceChanged
unsafe extern "system" fn adc_on_default_device_changed(
    this: *mut IMMNotificationClient,
    _flow: EDataFlow,
    _role: ERole,
    _id: LPCWSTR,
) -> i32 {
    let adc = from_this(this);
    let _ = adc.tx.send(());
    S_OK
}

/// IMMNotificationClient::OnPropertyValueChanged
unsafe extern "system" fn adc_on_property_value_changed(
    _this: *mut IMMNotificationClient,
    _id: LPCWSTR,
    _key: PROPERTYKEY,
) -> i32 {
    // ignore property changes
    S_OK
}
