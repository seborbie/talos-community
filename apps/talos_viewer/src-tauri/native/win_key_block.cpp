// Windows key suppression via RIDEV_NOHOTKEYS raw input registration.
//
// The low-level keyboard hook has been removed.  RIDEV_NOHOTKEYS is the sole
// mechanism for suppressing the Win key while the viewport window has focus.
// It is inherently focus-scoped: registered on WM_SETFOCUS, deregistered on
// WM_KILLFOCUS.
//
// See: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerrawinputdevices

#include <windows.h>

extern "C" {

// ---------------------------------------------------------------------------
// FFI stubs — kept for backward compatibility with Rust callers.
// The LL hook has been removed; these are intentional no-ops.
// ---------------------------------------------------------------------------
int win_key_block_init(void) { return 0; }
void win_key_block_set_enabled(int /*enabled*/) {}
void win_key_block_shutdown(void) {}

// ---------------------------------------------------------------------------
// RIDEV_NOHOTKEYS — the sole mechanism for suppressing the Win key.
// Register on WM_SETFOCUS, deregister on WM_KILLFOCUS.
// ---------------------------------------------------------------------------

int win_key_register_nohotkeys(HWND hwnd) {
    RAWINPUTDEVICE rid = {};
    rid.usUsagePage = 0x01;       // HID_USAGE_PAGE_GENERIC
    rid.usUsage     = 0x06;       // HID_USAGE_GENERIC_KEYBOARD
    rid.dwFlags     = RIDEV_NOHOTKEYS;
    rid.hwndTarget  = hwnd;
    BOOL ok = RegisterRawInputDevices(&rid, 1, sizeof(RAWINPUTDEVICE));
    return ok ? 0 : -1;
}

int win_key_deregister_nohotkeys(void) {
    RAWINPUTDEVICE rid = {};
    rid.usUsagePage = 0x01;
    rid.usUsage     = 0x06;
    rid.dwFlags     = RIDEV_REMOVE;
    rid.hwndTarget  = nullptr;    // Must be NULL for RIDEV_REMOVE
    BOOL ok = RegisterRawInputDevices(&rid, 1, sizeof(RAWINPUTDEVICE));
    return ok ? 0 : -1;
}

}  // extern "C"
