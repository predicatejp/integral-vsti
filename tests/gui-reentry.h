// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// Windowsの実描画中に再描画と編集配送を再入させる、継続実行用の回帰試験。
// 検証プロセス内のプラグインIATだけを差し替え、通常のOS呼び出しへ転送する。
namespace GuiReentry {
inline HWND target = nullptr, timerWindow = nullptr;
inline UINT_PTR timerId = 0;
inline WNDPROC windowProc = nullptr;
inline bool armed = false, insideDraw = false;
inline bool failTimer = false;
inline unsigned draws = 0;
inline unsigned injections = 0;
inline void (WINAPI* clear)(unsigned) = nullptr;
inline PROC (WINAPI* getGlProc)(LPCSTR) = nullptr;

void WINAPI clearWithReentry(unsigned mask) {
    ++draws;
    if (armed) {
        armed = false;
        insideDraw = true;
        assert(target && timerWindow && timerId);
        // 描画資源を借用中に同じ画面へ再入する。
        const auto context = wglGetCurrentContext();
        const auto dc = wglGetCurrentDC();
        InvalidateRect(target, nullptr, FALSE);
        UpdateWindow(target);
        assert(wglGetCurrentContext() == context && wglGetCurrentDC() == dc);
        // beginEditがキューに残っている状態でタイマーを再入させる。
        SendMessageW(timerWindow, WM_TIMER, timerId, 0);
        insideDraw = false;
        ++injections;
    }
    clear(mask);
}

FARPROC WINAPI resolve(HMODULE module, LPCSTR name) {
    auto result = GetProcAddress(module, name);
    if (reinterpret_cast<uintptr_t>(name) > 65535 && std::strcmp(name, "glClear") == 0) {
        clear = reinterpret_cast<decltype(clear)>(result);
        return reinterpret_cast<FARPROC>(clearWithReentry);
    }
    return result;
}

PROC WINAPI resolveGl(LPCSTR name) {
    auto result = getGlProc(name);
    if (result && std::strcmp(name, "glClear") == 0) {
        clear = reinterpret_cast<decltype(clear)>(result);
        return reinterpret_cast<PROC>(clearWithReentry);
    }
    return result;
}

UINT_PTR WINAPI setTimer(HWND hwnd, UINT_PTR id, UINT ms, TIMERPROC callback) {
    if (failTimer && hwnd && ms == 16) {
        failTimer = false;
        SetLastError(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    auto result = SetTimer(hwnd, id, ms, callback);
    // Editorの編集配送用16msタイマー。再表示時は新しいウィンドウへ更新する。
    if (result && hwnd && ms == 16) { timerWindow = hwnd; timerId = id; }
    return result;
}

LRESULT CALLBACK onMouseDown(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    auto result = CallWindowProcW(windowProc, hwnd, message, wparam, lparam);
    if (message == WM_LBUTTONDOWN) {
        SetWindowLongPtrW(hwnd, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(windowProc));
        // beginEditを処理する通常タイマーより先に、押下直後の描画を同期実行する。
        armed = true;
        InvalidateRect(hwnd, nullptr, FALSE);
        UpdateWindow(hwnd);
        assert(!armed);
    }
    return result;
}

void arm(HWND hwnd) {
    target = hwnd;
    windowProc = reinterpret_cast<WNDPROC>(
        SetWindowLongPtrW(hwnd, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(onMouseDown)));
    assert(windowProc);
}

void install(HMODULE module) {
    auto base = reinterpret_cast<unsigned char*>(module);
    auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
    auto nt = reinterpret_cast<IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
    auto imports = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(
        base + nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress);
    unsigned patched = 0;
    for (; imports->Name; ++imports) {
        auto names = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + imports->OriginalFirstThunk);
        auto slots = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + imports->FirstThunk);
        for (; names->u1.AddressOfData; ++names, ++slots) {
            if (IMAGE_SNAP_BY_ORDINAL64(names->u1.Ordinal)) continue;
            const auto name = reinterpret_cast<const char*>(
                reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + names->u1.AddressOfData)->Name);
            uintptr_t replacement = 0;
            if (std::strcmp(name, "GetProcAddress") == 0) replacement = reinterpret_cast<uintptr_t>(resolve);
            if (std::strcmp(name, "wglGetProcAddress") == 0) {
                getGlProc = reinterpret_cast<decltype(getGlProc)>(slots->u1.Function);
                replacement = reinterpret_cast<uintptr_t>(resolveGl);
            }
            if (std::strcmp(name, "SetTimer") == 0) replacement = reinterpret_cast<uintptr_t>(setTimer);
            if (!replacement) continue;
            DWORD old;
            assert(VirtualProtect(&slots->u1.Function, sizeof(uintptr_t), PAGE_READWRITE, &old));
            slots->u1.Function = replacement;
            DWORD ignored;
            assert(VirtualProtect(&slots->u1.Function, sizeof(uintptr_t), old, &ignored));
            ++patched;
        }
    }
    assert(patched == 3);
}
} // namespace GuiReentry
