// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// 検証専用の親ウィンドウとVST3コールバック。本体へリンクしない。
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"
#include <chrono>
#include <thread>
#include <algorithm>
#include <fstream>
#ifdef _WIN32
#include "gui-reentry.h"
#else
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/keysym.h>
#endif

static_assert(sizeof(ViewRect) == 16);

struct NativeParent {
#ifdef _WIN32
    HWND window, keyboard;
    inline static HWND lastKeyWindow = nullptr;
    inline static unsigned keyDowns = 0, keyUps = 0;
    static LRESULT CALLBACK procedure(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
        if (wparam == 'Z' || wparam == VK_SPACE) {
            if (message == WM_KEYDOWN) { ++keyDowns; lastKeyWindow = hwnd; }
            if (message == WM_KEYUP) { ++keyUps; lastKeyWindow = hwnd; }
        }
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    NativeParent() {
        WNDCLASSW cls {};
        cls.lpfnWndProc = procedure;
        cls.hInstance = GetModuleHandleW(nullptr);
        cls.lpszClassName = L"SampleVstiGuiTest";
        RegisterClassW(&cls);
        // 検証するクライアント領域全体を画面内に置く。
        window = CreateWindowExW(0, cls.lpszClassName, L"VST3 GUI test", WS_POPUP,
                                 0, 0, 960, 700, nullptr, nullptr, cls.hInstance, nullptr);
        assert(window); ShowWindow(window, SW_SHOW);
        // 埋め込み先とは別のホスト側コントロールをキー入力先にする。
        keyboard = CreateWindowExW(0, cls.lpszClassName, L"Host keyboard", WS_CHILD | WS_VISIBLE,
                                   0, 0, 1, 1, window, nullptr, cls.hInstance, nullptr);
        assert(keyboard);
    }
    ~NativeParent() { DestroyWindow(window); }
    HWND child() {
        for (HWND result = GetWindow(window, GW_CHILD); result; result = GetWindow(result, GW_HWNDNEXT))
            if (result != keyboard) return result;
        return nullptr;
    }
    void* handle() { return window; }
    void resize(int32 width, int32 height) { SetWindowPos(window, nullptr, 0, 0, width, height, SWP_NOMOVE | SWP_NOZORDER); }
    static void pump() {
        MSG msg;
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
    void pointer(int x, int y, bool down) {
        HWND child = this->child(); assert(child);
        // キャプチャ解除時のOSのマウス通知も、送信するクリックと同じ座標に揃える。
        POINT screen {x, y};
        assert(ClientToScreen(child, &screen));
        assert(SetCursorPos(screen.x, screen.y));
        pump();
        LPARAM position = MAKELPARAM(x, y);
        SendMessageW(child, WM_MOUSEMOVE, 0, position);
        SendMessageW(child, down ? WM_LBUTTONDOWN : WM_LBUTTONUP, down ? MK_LBUTTON : 0, position);
    }
    bool drawn(int width, int height) {
        HWND child = this->child();
        if (!child) return false;
        RECT rect; assert(GetClientRect(child, &rect));
        if (rect.right != width || rect.bottom != height) return false;
        HDC dc = GetDC(child); assert(dc);
        COLORREF pixel = GetPixel(dc, 20, height - 10);
        ReleaseDC(child, dc);
        return pixel != CLR_INVALID && pixel != RGB(0, 0, 0);
    }
    uint32 pixel(int x, int y) {
        HWND child = this->child(); assert(child);
        HDC dc = GetDC(child); assert(dc);
        COLORREF value = GetPixel(dc, x, y);
        ReleaseDC(child, dc);
        assert(value != CLR_INVALID);
        return value;
    }
    void screenshot(const char* path = "target/gui-smoke.ppm") {
        HWND child = this->child(); assert(child);
        RECT rect; assert(GetClientRect(child, &rect));
        HDC dc = GetDC(child); assert(dc);
        std::ofstream file(path, std::ios::binary);
        file << "P6\n" << rect.right << ' ' << rect.bottom << "\n255\n";
        for (int y = 0; y < rect.bottom; ++y) for (int x = 0; x < rect.right; ++x) {
            COLORREF pixel = GetPixel(dc, x, y);
            const char rgb[] = {static_cast<char>(GetRValue(pixel)),
                                static_cast<char>(GetGValue(pixel)), static_cast<char>(GetBValue(pixel))};
            file.write(rgb, 3);
        }
        ReleaseDC(child, dc);
    }
#else
    Display* display;
    ::Window window;
    NativeParent() {
        display = XOpenDisplay(nullptr); assert(display);
        window = XCreateSimpleWindow(display, DefaultRootWindow(display), 0, 0, 960, 700, 0, 0, 0);
        XMapWindow(display, window); XSync(display, False);
    }
    ~NativeParent() { XDestroyWindow(display, window); XCloseDisplay(display); }
    void* handle() { return reinterpret_cast<void*>(static_cast<uintptr_t>(window)); }
    void resize(int32 width, int32 height) { XResizeWindow(display, window, static_cast<unsigned>(width), static_cast<unsigned>(height)); XFlush(display); }
    void pump() { while (XPending(display)) { XEvent event; XNextEvent(display, &event); } }
    ::Window child() {
        ::Window root, parent, *children = nullptr; unsigned count = 0;
        assert(XQueryTree(display, window, &root, &parent, &children, &count));
        assert(count == 1); ::Window result = children[0]; XFree(children); return result;
    }
    void pointer(int x, int y, bool down) {
        const auto target = child();
        XEvent event {};
        event.xmotion = {MotionNotify, 0, True, display, target, DefaultRootWindow(display), 0,
                         CurrentTime, x, y, x, y, 0, NotifyNormal, True};
        XSendEvent(display, target, False, PointerMotionMask, &event);
        event.xbutton = {down ? ButtonPress : ButtonRelease, 0, True, display, target, DefaultRootWindow(display), 0,
                         CurrentTime, x, y, x, y, 0, Button1, True};
        XSendEvent(display, target, False, down ? ButtonPressMask : ButtonReleaseMask, &event);
        XFlush(display);
    }
    bool drawn(int width, int height) {
        auto target = child(); XWindowAttributes attributes;
        assert(XGetWindowAttributes(display, target, &attributes));
        if (attributes.width != width || attributes.height != height) return false;
        XImage* image = XGetImage(display, target, 20, height - 10, 1, 1, AllPlanes, ZPixmap);
        assert(image);
        bool result = (XGetPixel(image, 0, 0) & 0xffffff) != 0;
        XDestroyImage(image);
        return result;
    }
    uint32 pixel(int x, int y) {
        XImage* image = XGetImage(display, child(), x, y, 1, 1, AllPlanes, ZPixmap);
        assert(image);
        auto value = static_cast<uint32>(XGetPixel(image, 0, 0) & 0xffffff);
        XDestroyImage(image);
        return value;
    }
    void screenshot(const char* path = "target/gui-smoke.ppm") {
        const auto target = child(); XWindowAttributes attributes;
        assert(XGetWindowAttributes(display, target, &attributes));
        XImage* image = XGetImage(display, target, 0, 0, static_cast<unsigned>(attributes.width),
                                 static_cast<unsigned>(attributes.height), AllPlanes, ZPixmap);
        assert(image);
        std::ofstream file(path, std::ios::binary);
        file << "P6\n" << attributes.width << ' ' << attributes.height << "\n255\n";
        for (int y = 0; y < attributes.height; ++y) for (int x = 0; x < attributes.width; ++x) {
            unsigned long pixel = XGetPixel(image, x, y);
            const char rgb[] = {static_cast<char>(pixel >> 16), static_cast<char>(pixel >> 8), static_cast<char>(pixel)};
            file.write(rgb, 3);
        }
        XDestroyImage(image);
    }
#endif
    // OSの実ボタン状態と暗黙グラブも検証するため、ノブには実入力を注入する。
    void knobButton(bool down) {
#ifdef _WIN32
        INPUT input {}; input.type = INPUT_MOUSE;
        input.mi.dwFlags = down ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_LEFTUP;
        assert(SendInput(1, &input, sizeof(input)) == 1);
#else
        static void* xtst = dlopen("libXtst.so.6", RTLD_NOW | RTLD_LOCAL); assert(xtst);
        using Button = int (*)(Display*, unsigned int, Bool, unsigned long);
        static auto button = reinterpret_cast<Button>(dlsym(xtst, "XTestFakeButtonEvent")); assert(button);
        assert(button(display, 1, down ? True : False, 0)); XFlush(display);
#endif
    }
    void movePointer(int x, int y) {
#ifdef _WIN32
        POINT point {x, y}; assert(ClientToScreen(child(), &point));
        const auto absolute = [](LONG coordinate, int origin, int extent) {
            const auto offset = static_cast<long long>(coordinate) - origin;
            assert(extent > 0 && extent <= 65536 && offset >= 0 && offset < extent);
            // 画素の範囲内へ切り上げ、整数化で1px手前へ戻るのを防ぐ。
            return static_cast<LONG>((offset * 65536 + extent - 1) / extent);
        };
        INPUT input {}; input.type = INPUT_MOUSE;
        input.mi.dx = absolute(point.x, GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN));
        input.mi.dy = absolute(point.y, GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        // 座標のワープだけでなくマウス移動を送り、OSのタッチ入力によるカーソル抑制を解く。
        input.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        assert(SendInput(1, &input, sizeof(input)) == 1);
#else
        XWarpPointer(display, 0, child(), 0, 0, 0, 0, x, y); XFlush(display);
#endif
    }
    bool cursorVisible() {
#ifdef _WIN32
        CURSORINFO info {}; info.cbSize = sizeof(info); assert(GetCursorInfo(&info));
        return (info.flags & CURSOR_SHOWING) != 0;
#else
        struct CursorImage {
            short x, y; unsigned short width, height, xhot, yhot;
            unsigned long serial; unsigned long* pixels; unsigned long atom; const char* name;
        };
        static void* fixes = dlopen("libXfixes.so.3", RTLD_NOW | RTLD_LOCAL); assert(fixes);
        using GetImage = CursorImage* (*)(Display*);
        static auto getImage = reinterpret_cast<GetImage>(dlsym(fixes, "XFixesGetCursorImage")); assert(getImage);
        auto* image = getImage(display); assert(image);
        bool visible = false;
        for (unsigned i = 0; i < static_cast<unsigned>(image->width) * image->height; ++i)
            visible |= (image->pixels[i] & 0xff000000UL) != 0;
        XFree(image); return visible;
#endif
    }
    void unfocus() {
#ifdef _WIN32
        SetFocus(keyboard);
#else
        XSetInputFocus(display, window, RevertToParent, CurrentTime); XFlush(display);
#endif
    }
    bool focused() {
#ifdef _WIN32
        return GetFocus() == keyboard;
#else
        ::Window focus; int revert;
        XGetInputFocus(display, &focus, &revert);
        return focus == window;
#endif
    }
    bool pointerAt(int x, int y) {
#ifdef _WIN32
        POINT point; assert(GetCursorPos(&point)); assert(ScreenToClient(child(), &point));
        return point.x == x && point.y == y;
#else
        ::Window root, sub; int rx, ry, px, py; unsigned mask;
        assert(XQueryPointer(display, child(), &root, &sub, &rx, &ry, &px, &py, &mask));
        return px == x && py == y;
#endif
    }
    void click(int x, int y) { pointer(x, y, true); pointer(x, y, false); }

};

class GuiFrame final : public IPlugFrame
#ifndef _WIN32
    , public Linux::IRunLoop
#endif
{
public:
    uint32 refs = 1;
    NativeParent* parent = nullptr;
#ifndef _WIN32
    std::vector<Linux::ITimerHandler*> timers;
#endif
    tresult PLUGIN_API queryInterface(const TUID requested, void** out) override {
        *out = nullptr;
        if (std::memcmp(requested, IPlugFrame_iid, 16) == 0 || std::memcmp(requested, FUnknown_iid, 16) == 0)
            *out = static_cast<IPlugFrame*>(this);
#ifndef _WIN32
        if (std::memcmp(requested, Linux::IRunLoop_iid, 16) == 0) *out = static_cast<Linux::IRunLoop*>(this);
#endif
        if (!*out) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { assert(refs > 1); return --refs; }
    tresult PLUGIN_API resizeView(IPlugView* view, ViewRect* rect) override {
        if (parent) parent->resize(rect->getWidth(), rect->getHeight());
        return view->onSize(rect);
    }
#ifndef _WIN32
    tresult PLUGIN_API registerEventHandler(Linux::IEventHandler*, Linux::FileDescriptor) override { return kNotImplemented; }
    tresult PLUGIN_API unregisterEventHandler(Linux::IEventHandler*) override { return kNotImplemented; }
    tresult PLUGIN_API registerTimer(Linux::ITimerHandler* handler, Linux::TimerInterval milliseconds) override {
        assert(milliseconds > 0); handler->addRef(); timers.push_back(handler); return kResultOk;
    }
    tresult PLUGIN_API unregisterTimer(Linux::ITimerHandler* handler) override {
        auto it = std::find(timers.begin(), timers.end(), handler); assert(it != timers.end());
        timers.erase(it); handler->release(); return kResultOk;
    }
#endif
    void tick() {
#ifndef _WIN32
        for (auto* timer : timers) timer->onTimer();
#endif
    }
};

class GuiHandler final : public IComponentHandler {
public:
    uint32 refs = 1;
    std::vector<int> sequence;
    double value = -1.0;
    ParamID editedId = waveformId;
    std::thread::id uiThread = std::this_thread::get_id();
    IEditController* controller;
    explicit GuiHandler(IEditController* c) : controller(c) {}
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { assert(refs > 1); return --refs; }
    tresult PLUGIN_API beginEdit(ParamID id) override {
#ifdef _WIN32
        assert(!GuiReentry::insideDraw);
#endif
        assert(std::this_thread::get_id() == uiThread);
        assert(id == 0x4153454e || id == 0x4653454e || (id >= 0x43500100 && id <= 0x43500306) || id == 0x4c490000 || (id >= 0x50314346 && id <= 0x50335254) || (id >= 0x43314450 && id <= 0x4333544d) || (id >= 0x52314443 && id <= 0x5233535a) || (id >= 0x45510000 && id < 0x45510008) || (id >= 0x45463142 && id <= 0x45463354) || id == 0x43545950 || id == 0x43474149 || id == 0x434d4958 || id == 0x4f32464d || id == 0x4f33464d || id == 0x4f31564c || id == 0x4f325746 || id == 0x4f32544e || id == 0x4f32564c || id == 0x4f335746 || id == 0x4f33544e || id == 0x4f33564c || id == 0x57494454 || id == 0x4d50414e || std::find(cutoffEnvelopeIds.begin(), cutoffEnvelopeIds.end(), id) != cutoffEnvelopeIds.end() || id == volumeId || id == phaseId || id == waveformId || id == unisonId || id == detuneId || id == polyphonyId || std::find(filterIds.begin(), filterIds.end(), id) != filterIds.end() || std::find(envelopeIds.begin(), envelopeIds.end(), id) != envelopeIds.end());
        editedId = id;
        sequence.push_back(0); return kResultOk;
    }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue v) override {
#ifdef _WIN32
        assert(!GuiReentry::insideDraw);
#endif
        assert(std::this_thread::get_id() == uiThread && id == editedId);
        sequence.push_back(1); value = v;
        // ホストからControllerへ再入しても循環・借用違反を起こさない。
        assert(controller->setParamNormalized(id, v) == kResultOk);
        return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID id) override {
#ifdef _WIN32
        assert(!GuiReentry::insideDraw);
#endif
        assert(std::this_thread::get_id() == uiThread && id == editedId);
        sequence.push_back(2); return kResultOk;
    }
    tresult PLUGIN_API restartComponent(int32) override { assert(false); return kNotImplemented; }
};

void checkViewContract(IEditController* controller) {
    assert(controller->createView("unknown") == nullptr);
    auto* view = controller->createView("editor"); assert(view);
    assert(view->canResize() == kResultFalse);
    ViewRect rect;
    assert(view->getSize(&rect) == kResultOk && rect.getWidth() == 1240 && rect.getHeight() == 728);
    assert(view->isPlatformTypeSupported("unknown") == kResultFalse);
#ifdef _WIN32
    const auto platform = kPlatformTypeHWND;
#else
    const auto platform = kPlatformTypeX11EmbedWindowID;
#endif
    assert(view->isPlatformTypeSupported(platform) == kResultOk);
    assert(view->attached(nullptr, platform) == kInvalidArgument);
    void* result = nullptr;
    assert(view->queryInterface(IPlugViewContentScaleSupport_iid, &result) == kNoInterface && !result);
    rect.right = 800;
    assert(view->onSize(&rect) == kResultFalse);
    assert(view->checkSizeConstraint(&rect) == kResultOk && rect.getWidth() == 1240);
    assert(view->onKeyDown(u' ', 0, 0) == kResultFalse);
    assert(view->onKeyUp(u' ', 0, 0) == kResultFalse);
    assert(view->onWheel(1) == kResultFalse);
    assert(view->onFocus(true) == kResultOk);
    assert(view->removed() == kResultOk);
    assert(view->release() == 0);
}

#ifdef _WIN32
void checkGlContext(IEditController* controller) {
    NativeParent parent;
    GuiFrame frame; frame.parent = &parent;
    HDC dc = GetDC(parent.window); assert(dc);
    PIXELFORMATDESCRIPTOR format {};
    format.nSize = sizeof(format); format.nVersion = 1;
    format.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
    format.iPixelType = PFD_TYPE_RGBA; format.cColorBits = 24;
    const int index = ChoosePixelFormat(dc, &format); assert(index);
    assert(SetPixelFormat(dc, index, &format));
    HGLRC context = wglCreateContext(dc); assert(context);
    auto* view = controller->createView("editor"); assert(view);
    assert(view->setFrame(&frame) == kResultOk);
    unsigned draws = 0;
    for (int cycle = 0; cycle < 4; ++cycle) {
        const bool active = cycle % 2 == 0;
        const HGLRC expectedContext = active ? context : nullptr;
        const HDC expectedDc = active ? dc : nullptr;
        assert(wglMakeCurrent(expectedDc, expectedContext));
        auto restored = [&] {
            return wglGetCurrentContext() == expectedContext && wglGetCurrentDC() == expectedDc;
        };
        // Painter生成後のタイマー作成失敗から戻る経路も、同じ条件で検証する。
        GuiReentry::failTimer = true;
        assert(view->attached(parent.handle(), kPlatformTypeHWND) != kResultOk);
        assert(!GuiReentry::failTimer && restored());
        assert(view->attached(parent.handle(), kPlatformTypeHWND) == kResultOk);
        assert(restored());
        for (int i = 0; i < 100; ++i) {
            const auto before = GuiReentry::draws;
            InvalidateRect(parent.child(), nullptr, FALSE);
            UpdateWindow(parent.child());
            assert(GuiReentry::draws > before && restored());
            ++draws;
        }
        assert(view->removed() == kResultOk);
        assert(restored());
    }
    assert(view->setFrame(nullptr) == kResultOk);
    assert(view->release() == 0);
    assert(!wglGetCurrentContext() && !wglGetCurrentDC());
    assert(wglDeleteContext(context));
    ReleaseDC(parent.window, dc);
    std::cout << "GL context: " << draws << " redraws, 4 opens/closes and 4 creation failures preserved host context/DC\n";
}
#endif

void checkGui(IComponent* component, IAudioProcessor* processor, IEditController* controller) {
#ifdef _WIN32
    checkGlContext(controller);
#endif
    NativeParent parent;
    GuiFrame frame; frame.parent = &parent;
    GuiHandler handler(controller);
    assert(controller->setComponentHandler(&handler) == kResultOk && handler.refs == 2);
    auto* view = controller->createView("editor"); assert(view);
    ViewRect size; assert(view->getSize(&size) == kResultOk);
    parent.resize(size.getWidth(), size.getHeight());
    assert(view->setFrame(&frame) == kResultOk);
    auto pump = [&] {
        for (int i = 0; i < 10; ++i) {
            parent.pump(); frame.tick(); std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
    };
    auto waitFor = [&](auto condition, const char* description) {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        do {
            parent.pump(); frame.tick();
            if (condition()) return;
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        } while (std::chrono::steady_clock::now() < deadline);
        parent.screenshot();
        std::cerr << "GUI timeout: " << description << ", edit notifications=" << handler.sequence.size()
                  << ", parameter=" << handler.editedId << std::endl;
#ifdef _WIN32
        CURSORINFO cursor {}; cursor.cbSize = sizeof(cursor);
        POINT pointer {};
        const auto cursorOk = GetCursorInfo(&cursor), pointerOk = GetCursorPos(&pointer);
        std::cerr << "Windows cursor: query=" << cursorOk << ", flags=" << cursor.flags
                  << ", handle=" << cursor.hCursor << ", capture=" << GetCapture() << ", focus=" << GetFocus()
                  << ", pointer query=" << pointerOk << ", position=" << pointer.x << ',' << pointer.y
                  << ", child=" << parent.child() << ", keyboard=" << parent.keyboard << std::endl;
#endif
        assert(false);
    };
    // グループ内のノブ中心とドロップダウン。画面上の実入力で全設定を検証する。
    std::array<std::pair<int, int>, 132> controls {{
        {175, 124}, {292, 146}, {348, 146}, {404, 146}, {460, 146}, {516, 146},
        {44, 174}, {100, 174}, {960, 524}, {535, 242}, {306, 314}, {390, 314},
        {960, 133}, {960, 317}, {298, 409}, {365, 409}, {432, 409}, {499, 409},
        {566, 409}, {474, 314}, {156, 174}, {960, 420}, {212, 174}, {175, 245},
        {53, 295}, {128, 295}, {175, 367}, {53, 417}, {128, 417}, {203, 295},
        {203, 417}, {669, 150}, {765, 150}, {852, 150}, {535, 505}, {58, 555},
        {141, 555}, {225, 555}, {308, 555}, {391, 555}, {475, 555}, {558, 555},
        {745, 66}, {745, 251}, {745, 436}, {669, 335}, {765, 335}, {852, 335},
        {669, 520}, {765, 520}, {852, 520}, {765, 115}, {765, 186}, {852, 150},
        {765, 300}, {765, 371}, {852, 335}, {765, 485}, {765, 556}, {852, 520},
        {669, 150}, {669, 335}, {669, 520}, {716, 150}, {777, 150}, {655, 150},
        {852, 150}, {716, 335}, {777, 335}, {655, 335}, {852, 335}, {716, 520},
        {777, 520}, {655, 520}, {852, 520}, {655, 115}, {716, 115}, {670, 186},
        {762, 186}, {777, 115}, {852, 150}, {655, 300}, {716, 300}, {670, 371},
        {762, 371}, {777, 300}, {852, 335}, {655, 485}, {716, 485}, {670, 556},
        {762, 556}, {777, 485}, {852, 520}, {670, 115}, {762, 115}, {670, 186},
        {762, 186}, {852, 150}, {670, 300}, {762, 300}, {670, 371}, {762, 371},
        {852, 335}, {670, 485}, {762, 485}, {670, 556}, {762, 556}, {852, 520},
        {960, 236}, {655, 115}, {716, 115}, {655, 186}, {716, 186}, {777, 115},
        {777, 186}, {852, 150}, {655, 300}, {716, 300}, {655, 371}, {716, 371},
        {777, 300}, {777, 371}, {852, 335}, {655, 485}, {716, 485}, {655, 556},
        {716, 556}, {777, 485}, {777, 556}, {852, 520}, {572, 146}, {558, 314},
    }};
    auto uiPixel = [](int point) { return static_cast<int>(std::lround(point * 1.2)); };
    auto choose = [&](int index, int item) {
        const auto [x, y] = controls[index];
        parent.click(uiPixel(x), uiPixel(y)); pump(); assert(handler.sequence.empty());
        parent.screenshot(("target/gui-menu-" + std::to_string(index) + ".ppm").c_str());
        // 最下段の4択は画面内へ収まるよう上側に開く。
        const int firstItemY = index == 62 ? y - 112 : index == 44 ? y - 193 : y + 30;
        parent.click(uiPixel(x), uiPixel(firstItemY + item * 27));
        waitFor([&] { return handler.sequence.size() >= 3; }, "dropdown choice");
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        handler.sequence.clear(); pump();
    };
    for (int cycle = 0; cycle < 2; ++cycle) {
#ifdef _WIN32
        assert(view->attached(parent.handle(), kPlatformTypeHWND) == kResultOk);
#else
        assert(view->attached(parent.handle(), kPlatformTypeX11EmbedWindowID) == kResultOk);
#endif
        waitFor([&] { return parent.drawn(1240, 728); }, "first rendered frame");
        pump();
        if (cycle == 0) parent.screenshot("target/gui-panel.ppm");
        if (cycle == 0) {
            // 実際の音声処理からタイトルバーへ発音数を渡し、再描画・release・停止を検証する。
            StateStream original; assert(component->getState(&original) == kResultOk);
            ProcessSetup setup {kRealtime, kSample32, 64, 48000.0};
            assert(processor->setupProcessing(setup) == kResultOk);
            assert(component->setActive(true) == kResultOk);
            assert(processor->setProcessing(true) == kResultOk);
            Points poly; poly.id = polyphonyId; poly.values = {{0, 2.0 / 31.0}};
            assert(controller->setParamNormalized(polyphonyId, 2.0 / 31.0) == kResultOk);
            Points master; master.id = volumeId; master.values = {{0, 1.0}};
            assert(controller->setParamNormalized(volumeId, 1.0) == kResultOk);
            Changes changes; changes.queues = {&poly, &master};
            Events events; events.values = {note(true, 0, 100), note(true, 0, 101), note(true, 0, 102)};
            std::array<float, 64> left {}, right {};
            Sample32* channels[] {left.data(), right.data()};
            AudioBusBuffers bus {}; bus.numChannels = 2; bus.channelBuffers32 = channels;
            assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
            ProcessData audio {}; audio.numSamples = 1; audio.symbolicSampleSize = kSample32;
            audio.numOutputs = 1; audio.outputs = &bus;
            audio.inputParameterChanges = &changes; audio.inputEvents = &events;
#ifdef _WIN32
            const uint32 lit = RGB(0, 164, 102), unlit = RGB(224, 230, 224), over = RGB(232, 64, 48);
#else
            const uint32 lit = 0x00a466, unlit = 0xe0e6e0, over = 0xe84030;
#endif
            auto meter = [&](int count) {
                for (int i = 0; i < 32; ++i) {
                    if (parent.pixel(uiPixel(349 + i * 8), uiPixel(32)) != (i < count ? lit : unlit)) return false;
                }
                return true;
            };
            assert(processor->process(audio) == kResultOk);
            waitFor([&] { return meter(3); }, "three active voices");
            audio.inputParameterChanges = nullptr; audio.inputEvents = nullptr;
            audio.numSamples = 64;
            for (int i = 0; i < 16; ++i) assert(processor->process(audio) == kResultOk);
            waitFor([&] {
                assert(processor->process(audio) == kResultOk);
                return parent.pixel(uiPixel(432), uiPixel(20)) == lit && parent.pixel(uiPixel(598), uiPixel(20)) == over;
            }, "peak at +12 dBFS: green below zero, red above");
            parent.screenshot("target/gui-voices.ppm");
            audio.inputEvents = &events;
            events.values = {note(false, 0, 100), note(false, 0, 101), note(false, 0, 102)};
            assert(processor->process(audio) == kResultOk);
            pump(); assert(meter(3)); // release中も点灯する。
            audio.inputEvents = nullptr; audio.numSamples = 64;
            for (int i = 0; i < 80; ++i) assert(processor->process(audio) == kResultOk);
            waitFor([&] { return meter(0); }, "release finished");
            events.values = {note(true, 0, 100), note(true, 0, 101), note(true, 0, 102)};
            audio.inputEvents = &events;
            assert(processor->process(audio) == kResultOk);
            waitFor([&] { return meter(3); }, "voices before stop");
            assert(processor->setProcessing(false) == kResultOk);
            waitFor([&] { return meter(0); }, "voices after stop");
            waitFor([&] { return parent.pixel(uiPixel(349), uiPixel(20)) == unlit; }, "peak decays to silence");
            assert(component->setActive(false) == kResultOk);
            assert(component->setState(&original) == kResultOk);
        }
        assert(controller->setParamNormalized(waveformId, 0.0) == kResultOk);
        pump(); assert(handler.sequence.empty());
        for (int item = 1; item <= 4; ++item) {
            choose(0, item);
            assert(handler.editedId == waveformId && handler.value == item / 4.0);
        }
        choose(0, 0);
        assert(handler.editedId == waveformId && handler.value == 0.0);
        Points point; point.values = {{0, handler.value}};
        Changes changes; changes.queues = {&point};
        ProcessData data; data.inputParameterChanges = &changes;
        assert(processor->process(data) == kResultOk);
        StateStream saved; assert(component->getState(&saved) == kResultOk && saved.bytes[8] == 0);
        saved.bytes[8] = 2;
        assert(component->setState(&saved) == kResultOk);
        assert(controller->getParamNormalized(waveformId) == 0.5);
        pump(); assert(handler.sequence.empty());
        for (const int index : {23, 26}) {
            ParameterInfo info; assert(controller->getParameterInfo(index, info) == kResultOk);
            assert(controller->setParamNormalized(info.id, 0.0) == kResultOk);
            pump();
            for (int item = 1; item <= 4; ++item) {
                choose(index, item);
                assert(handler.editedId == info.id && handler.value == item / 4.0);
            }
            choose(index, 0);
            assert(handler.editedId == info.id && handler.value == 0.0);
        }
        choose(42, 4);
        assert(controller->setParamNormalized(0x43545950, 0) == kResultOk); pump();
        for (int item : {1, 0}) {
            choose(31, item);
            assert(handler.editedId == 0x43545950 && handler.value == item);
        }
        for (int index : {34, 108}) for (int item : {1, 0}) {
            choose(index, item);
            assert(handler.value == item);
        }
        for (int slot = 1; slot < 3; ++slot) {
            choose(42 + slot, 4);
            choose(slot == 1 ? 45 : 48, 1);
            choose(slot == 1 ? 45 : 48, 0);
        }
        parent.movePointer(4, 4); pump(); parent.screenshot("target/gui-distortion.ppm");
        for (int slot = 0; slot < 3; ++slot) {
            choose(42 + slot, 3);
            ParameterInfo info; assert(controller->getParameterInfo(60 + slot, info) == kResultOk);
            assert(info.stepCount == 3);
            assert(controller->setParamNormalized(info.id, 0) == kResultOk); pump();
            for (int item : {1, 2, 3, 0}) {
                choose(60 + slot, item);
                assert(handler.editedId == info.id && handler.value == item / 3.0);
            }
        }
        for (int slot=0; slot<3; ++slot) {
            choose(42+slot, 6);
            assert(handler.value==1.0);
        }
        parent.movePointer(4,4);pump();parent.screenshot("target/gui-reverb.ppm");
        for(int slot=0;slot<3;++slot) choose(42+slot, 1);
        parent.movePointer(4,4);pump();parent.screenshot("target/gui-chorus.ppm");
        for(int slot=0;slot<3;++slot) choose(42+slot, 5);
        parent.movePointer(4,4);pump();parent.screenshot("target/gui-phaser.ppm");
        for(int slot=0;slot<3;++slot) choose(42+slot, 2);
        parent.movePointer(4,4);pump();parent.screenshot("target/gui-compressor.ppm");
        for(int slot=0;slot<3;++slot) choose(42+slot, 3);
        // Timeは整数ノブ。音価を全14段階、上下両方向へ操作する。
        assert(controller->setParamNormalized(0x45463142, 0.0) == kResultOk);
        const int timeX = uiPixel(controls[51].first), timeY = uiPixel(controls[51].second);
        parent.movePointer(timeX, timeY); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence == (std::vector<int>{0}); }, "time knob begin");
        size_t edits = 1;
        for (int direction : {-1, 1}) for (int tick = 1; tick <= 13; ++tick) {
            const auto previous = handler.value;
            parent.movePointer(timeX, timeY + direction * 31); pump();
            assert(handler.sequence.size() == edits && handler.value == previous);
            parent.movePointer(timeX, timeY + direction);
            ++edits;
            waitFor([&] { return handler.sequence.size() == edits; }, "time knob step");
            const int expected = direction < 0 ? tick : 13 - tick;
            assert(handler.editedId == 0x45463142 && handler.value == expected / 13.0);
            if (direction < 0 && tick == 5) { pump(); parent.screenshot("target/gui-delay-time.ppm"); }
        }
        parent.knobButton(false);
        waitFor([&] { return handler.sequence.size() == edits + 1 && parent.cursorVisible(); }, "time knob end");
        handler.sequence.clear();
        assert(controller->setParamNormalized(0x45463142, 5.0 / 13) == kResultOk); pump();
        parent.screenshot("target/gui-delay.ppm");
        // ホストが種類を切り替えた場合、非表示になるTimeのドラッグを終了する。
        parent.movePointer(timeX, timeY); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence == (std::vector<int>{0}); }, "delay drag begin");
        assert(controller->setParamNormalized(0x45463154, 0.0) == kResultOk);
        waitFor([&] { return handler.sequence == (std::vector<int>{0, 2}); }, "hidden delay drag end");
        parent.knobButton(false);
        waitFor([&] { return parent.cursorVisible(); }, "hidden delay cursor restored");
        handler.sequence.clear(); pump();
        // 交換ボタンは種類と内部設定をまとめて隣へ移動し、標準の編集通知を送る。
        const std::array<int, 60> swappedIndices {42, 31, 32, 33, 51, 52, 53, 60, 63, 64, 65, 66, 75, 76, 77, 78, 79, 80, 93, 94, 95, 96, 97, 109, 110, 111, 112, 113, 114, 115, 43, 45, 46, 47, 54, 55, 56, 61, 67, 68, 69, 70, 81, 82, 83, 84, 85, 86, 98, 99, 100, 101, 102, 116, 117, 118, 119, 120, 121, 122};
        const std::array<double, 60> beforeSwap {4./6, 1, .25, .75, 0, .2, .3, 0, .1, .2, .3, .4, .1, .2, .3, .4, .5, .6, .1, .2, .3, .4, .5, .1, .2, .3, .4, .5, .6, .7, 1, 0, .5, .25, 1, .4, .6, 1, .5, .6, .7, .8, .6, .5, .4, .3, .2, .1, .9, .8, .7, .6, .4, .7, .6, .5, .3, .2, .1, .8};
        for (size_t i = 0; i < swappedIndices.size(); ++i) {
            ParameterInfo info; assert(controller->getParameterInfo(swappedIndices[i], info) == kResultOk);
            assert(controller->setParamNormalized(info.id, beforeSwap[i]) == kResultOk);
        }
        pump(); parent.click(uiPixel(860), uiPixel(66));
        waitFor([&] { return handler.sequence.size() == 180; }, "adjacent effect swap");
        for (size_t i = 0; i < swappedIndices.size(); ++i) {
            ParameterInfo info; assert(controller->getParameterInfo(swappedIndices[i], info) == kResultOk);
            assert(controller->getParamNormalized(info.id) == beforeSwap[(i + 30) % 60]);
            assert(handler.sequence[i*3] == 0 && handler.sequence[i*3+1] == 1 && handler.sequence[i*3+2] == 2);
        }
        handler.sequence.clear(); pump();
        // Offでは完成したヘッダ・ノブを背景へ50%合成し、選択欄は通常の色を維持する。
        for (const auto& check : std::array<std::array<int, 3>, 2>{{{9, 10, 266}, {34, 35, 18}}}) {
            const auto [choiceIndex, knobIndex, headerX] = check;
            ParameterInfo info; assert(controller->getParameterInfo(choiceIndex, info) == kResultOk);
            assert(controller->setParamNormalized(info.id, 0.0) == kResultOk);
            auto pixels = [&] {
                const auto [choiceX, choiceY] = controls[choiceIndex];
                const auto [knobX, knobY] = controls[knobIndex];
                return std::array<uint32, 3>{
                    parent.pixel(uiPixel(headerX), uiPixel(choiceY)),
                    parent.pixel(uiPixel(knobX), uiPixel(knobY)),
                    parent.pixel(uiPixel(choiceX + 40), uiPixel(choiceY - 8))};
            };
            parent.movePointer(4, 4); pump();
            const auto off = pixels();
            assert(off[2] == parent.pixel(uiPixel(controls[0].first + 40), uiPixel(controls[0].second - 8)));
            choose(choiceIndex, 1);
            parent.movePointer(4, 4); pump();
            const auto on = pixels();
            const auto background = parent.pixel(4, 4);
            for (int sample = 0; sample < 2; ++sample) {
                for (int shift : {0, 8, 16}) {
                    const int expected = (((on[sample] >> shift) & 0xff) + ((background >> shift) & 0xff) + 1) / 2;
                    const int actual = (off[sample] >> shift) & 0xff;
                    assert(std::abs(actual - expected) <= 2);
                }
            }
            assert(off[2] == on[2]);
            choose(choiceIndex, 0);
            parent.movePointer(4, 4); pump();
            assert(pixels() == off);
        }
        // Offは内部選択欄・ノブ・ラベルを描画せず、旧ノブ位置でも編集を始めない。
        for (int slot = 0; slot < 3; ++slot) {
            choose(42 + slot, 0);
            parent.movePointer(4, 4); pump();
            const int y = controls[53 + slot * 3].second;
            const auto empty = parent.pixel(uiPixel(634), uiPixel(y - 40));
            for (int x : {634, 656, 688, 720, 752, 784, 816, 848, 870}) {
                assert(parent.pixel(uiPixel(x), uiPixel(y)) == empty);
                assert(parent.pixel(uiPixel(x), uiPixel(y + 40)) == empty);
            }
            assert(parent.pixel(uiPixel(669), uiPixel(y)) == empty);
            parent.movePointer(uiPixel(720), uiPixel(y)); pump(); parent.knobButton(true); pump();
            assert(handler.sequence.empty() && parent.cursorVisible());
            parent.knobButton(false); pump();
        }
        parent.movePointer(4, 4); pump(); parent.screenshot("target/gui-effects-off.ppm");
        // Offのグループを含む全ノブを実マウス入力で操作する。1ドラッグはbegin/value/endの一組。
        parent.unfocus(); pump(); assert(parent.focused());
        for (int i = 1; i < 132; ++i) {
            if (i == 108 || i == 9 || i == 12 || i == 23 || i == 26 || i == 31 || i == 34 || (i >= 42 && i <= 45) || i == 48 || (i >= 60 && i <= 62)) continue;
            if ((i >= 51 && i < 130) || i == 32 || i == 33 || i == 46 || i == 47 || i == 49 || i == 50) {
                const int slot = i >= 109 ? (i - 109) / 7 : i >= 93 ? (i - 93) / 5 : i >= 75 ? (i - 75) / 6 : i >= 63 ? (i - 63) / 4 : i >= 51 ? (i - 51) / 3 : (i < 45 ? 0 : (i - 45) / 3 + 1);
                ParameterInfo kind; assert(controller->getParameterInfo(42 + slot, kind) == kResultOk);
                assert(controller->setParamNormalized(kind.id, i >= 109 ? 2./6 : i >= 93 ? 5./6 : i >= 75 ? 1./6 : i >= 63 ? 1.0 : i >= 51 ? 3./6 : 4./6) == kResultOk);
            }
            ParameterInfo info; assert(controller->getParameterInfo(i, info) == kResultOk);
            assert(controller->setParamNormalized(info.id, 0.0) == kResultOk);
            pump(); assert(handler.sequence.empty());
            const auto [logicalX, logicalY] = controls[i];
            const int x = uiPixel(logicalX), y = uiPixel(logicalY);
            parent.movePointer(x, y); pump();
#ifdef _WIN32
            GuiReentry::arm(parent.child());
#endif
            parent.knobButton(true);
            waitFor([&] { return handler.sequence.size() == 1; }, "knob begin");
            assert(handler.sequence == (std::vector<int>{0}));
            waitFor([&] { return !parent.cursorVisible(); }, "knob cursor hidden");
            // 全ノブで224×60論理ポイントの同じ枠が画面内に描画される。
            const int overlayX = std::clamp(logicalX - 112, 0, 809);
            const int overlayY = std::clamp(logicalY - 91, 0, 547);
#ifdef _WIN32
            const uint32 overlayColor = RGB(23, 43, 38);
#else
            const uint32 overlayColor = 0x172b26;
#endif
            waitFor([&] {
                return parent.pixel(uiPixel(overlayX + 4), uiPixel(overlayY + 4)) == overlayColor &&
                       parent.pixel(uiPixel(overlayX + 219), uiPixel(overlayY + 4)) == overlayColor &&
                       parent.pixel(uiPixel(overlayX + 4), uiPixel(overlayY + 55)) == overlayColor &&
                       parent.pixel(uiPixel(overlayX + 219), uiPixel(overlayY + 55)) == overlayColor;
            }, "fixed overlay bounds");
#ifdef _WIN32
            assert(parent.focused()); // 操作中にもホストの入力先を維持する。
#endif
            const int pixelsPerTick = i >= 130 ? 4 : i >= 109 ? std::array<int,7>{2,4,3,3,4,4,4}[(i-109)%7] : (i >= 93 && ((i-93)%5 == 0 || (i-93)%5 == 2)) ? 3 : (i == 76 || i == 82 || i == 88) ? 2 : (i == 77 || i == 83 || i == 89) ? 3 : (i == 6 || i == 51 || i == 54 || i == 57) ? 32 : i == 8 ? 8 :
                (i == 10 || i == 11 || i == 13 || i == 19 || i == 22 || i == 25 || i == 28) ? 2 :
                (i == 1 || i == 2 || i == 3 || i == 5 || i == 14 || i == 15 || i == 16 ||
                 i == 18 || i == 24 || i == 27 || i == 32 || i == 46 || i == 49) ? 3 : 4;
            parent.movePointer(x, y - (pixelsPerTick - 1)); pump(); // 開始値は吸着しない。
            assert(handler.sequence.size() == 1 && controller->getParamNormalized(info.id) == 0.0);
            assert(parent.pointerAt(x, y));
            parent.movePointer(x, y - 1);
            waitFor([&] { return handler.sequence.size() == 2; }, "knob integer tick");
            assert(handler.sequence == (std::vector<int>{0, 1}) && handler.editedId == info.id);
            const bool time = i == 1 || i == 2 || i == 3 || i == 5 || i == 14 || i == 15 || i == 16 || i == 18;
            const double plain = i >= 130 ? 1.0 : i >= 109 ? std::array<double,7>{-59.9,1.1,1,1,.1,.1,1}[(i-109)%7] : i >= 93 ? std::array<double,5>{100.*std::pow(40.,1./400.),10.,.06,-94.,1.}[(i-93)%5] : i >= 75 ? std::array<double,6>{.2, .01, .06, -.099, -94., 1.}[(i-75)%6] : i >= 63 ? std::array<double,4>{.1 * std::pow(100.,1./200.), 500. * std::pow(40.,1./300.), 51., 1.}[(i-63)%4] : (i == 35 || i == 37 || i == 40) ? 20. * std::pow(1000., 1./300.) : (i == 36 || i == 38 || i == 41) ? -17.9 : i == 39 ? .1 * std::pow(100., 1./200.) : time ? 1.0 : (i == 6 || i == 8) ? 2.0 : i == 7 ? 1.0 : i == 10 ? -2390.0 :
                                 i == 11 ? .51 : (i == 13 || i == 22 || i == 25 || i == 28) ? -59.9 : (i == 24 || i == 27) ? -2390.0 : i == 19 ? -4790.0 : i == 20 ? 1.0 : i == 21 ? -99.0 : (i >= 51 || i == 29 || i == 30 || i == 33 || i == 47 || i == 50) ? 1.0 : (i == 32 || i == 46 || i == 49) ? .1 : .01;
            assert(std::abs(controller->normalizedParamToPlain(info.id, handler.value) - plain) < 1e-9);
            assert(parent.pointerAt(x, y));
            point.id = info.id; point.values = {{0, handler.value}};
            assert(processor->process(data) == kResultOk);
            StateStream state; assert(component->getState(&state) == kResultOk);
            double stored = 0; std::memcpy(&stored, state.bytes.data() + 12 + (i - 1) * 8, 8);
            assert(std::abs(stored - plain) < 1e-9);
            parent.knobButton(false);
            waitFor([&] { return handler.sequence.size() == 3; }, "knob end");
            assert(handler.sequence == (std::vector<int>{0, 1, 2}));
            waitFor([&] { return parent.focused(); }, "host focus after knob");
#ifdef _WIN32
            // フォーカス先へ実キー入力を配送し、文字キーとスペースの両端を確認する。
            if (i == 13) {
                const auto downs = NativeParent::keyDowns, ups = NativeParent::keyUps;
                INPUT keys[4] {};
                for (int k = 0; k < 4; ++k) {
                    keys[k].type = INPUT_KEYBOARD;
                    keys[k].ki.wVk = k < 2 ? 'Z' : VK_SPACE;
                    keys[k].ki.dwFlags = (k % 2) ? KEYEVENTF_KEYUP : 0;
                }
                assert(SendInput(4, keys, sizeof(INPUT)) == 4);
                waitFor([&] { return NativeParent::keyDowns == downs + 2 &&
                                    NativeParent::keyUps == ups + 2; }, "host keys after volume");
                assert(NativeParent::lastKeyWindow == parent.keyboard);
            }
#endif
            assert(parent.pointerAt(x, y));
            parent.movePointer(20, 20); pump(); assert(parent.pointerAt(20, 20));
            waitFor([&] { return parent.cursorVisible(); }, "cursor restored after knob");
            handler.sequence.clear();
            if (i == 21) {
                // 吸着点から開始すると内部位置40。大きい側から再進入すると80。
                assert(controller->setParamNormalized(info.id, .5) == kResultOk);
                parent.movePointer(x, y); pump(); parent.knobButton(true);
                waitFor([&] { return handler.sequence.size() == 1; }, "pan center begin");
                parent.movePointer(x, y - 39); pump();
                assert(handler.sequence.size() == 1 && controller->getParamNormalized(info.id) == .5);
                parent.movePointer(x, y - 1);
                waitFor([&] { return handler.sequence.size() == 2; }, "pan center start");
                assert(std::abs(handler.value - .505) < 1e-9);
                parent.movePointer(x, y + 4);
                waitFor([&] { return handler.sequence.size() == 3; }, "pan center arrival");
                assert(handler.value == .5);
                for (int step : {32, 32, 15}) {
                    parent.movePointer(x, y + step); pump();
                }
                assert(handler.sequence.size() == 3 && handler.value == .5);
                parent.movePointer(x, y + 1);
                waitFor([&] { return handler.sequence.size() == 4; }, "pan lower detent exit");
                assert(std::abs(handler.value - .495) < 1e-9);
                // 小さい側から進入し、20px進んで20px戻ると小さい側へ抜ける。
                parent.movePointer(x, y - 4);
                waitFor([&] { return handler.sequence.size() == 5; }, "pan lower arrival");
                assert(handler.value == .5);
                parent.movePointer(x, y - 20); pump();
                parent.movePointer(x, y + 19); pump();
                assert(handler.sequence.size() == 5 && handler.value == .5);
                parent.movePointer(x, y + 1);
                waitFor([&] { return handler.sequence.size() == 6; }, "pan reversed detent exit");
                assert(std::abs(handler.value - .495) < 1e-9);
                parent.knobButton(false);
                waitFor([&] { return handler.sequence.size() == 7; }, "pan center end");
                handler.sequence.clear();
            }
            assert(controller->setParamNormalized(info.id, .123456789) == kResultOk);
            pump(); assert(handler.sequence.empty());
        }
        assert(controller->setParamNormalized(filterIds[0], 1.0 / 6.0) == kResultOk);
        pump();
        for (int item = 2; item <= 6; ++item) {
            choose(9, item);
            assert(handler.editedId == filterIds[0] && handler.value == item / 6.0);
        }
        choose(9, 0);
        assert(handler.editedId == filterIds[0] && handler.value == 0.0);
        assert(controller->setParamNormalized(phaseId, 0.0) == kResultOk);
        pump(); choose(12, 1);
        assert(handler.editedId == phaseId && handler.value == .5);
        choose(12, 2);
        assert(handler.editedId == phaseId && handler.value == 1.0);
        parent.movePointer(4, 4); pump();
        parent.screenshot("target/gui-choices.ppm");
        point.id = phaseId; point.values = {{0, handler.value}};
        assert(processor->process(data) == kResultOk);
        StateStream phaseState; assert(component->getState(&phaseState) == kResultOk);
        double phaseMode = -1; std::memcpy(&phaseMode, phaseState.bytes.data() + 100, 8);
        assert(phaseMode == 2.0);
#ifdef _WIN32
        parent.movePointer(uiPixel(controls[13].first), uiPixel(controls[13].second)); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence.size() == 1; }, "knob before escape");
        INPUT escape {}; escape.type = INPUT_KEYBOARD; escape.ki.wVk = VK_ESCAPE;
        assert(SendInput(1, &escape, sizeof(INPUT)) == 1);
        waitFor([&] { return handler.sequence.size() == 2; }, "escape without plug-in focus");
        waitFor([&] { return parent.focused() && parent.cursorVisible(); }, "host focus and cursor after escape");
        escape.ki.dwFlags = KEYEVENTF_KEYUP;
        assert(SendInput(1, &escape, sizeof(INPUT)) == 1);
        parent.knobButton(false); pump(); handler.sequence.clear();
#endif
        // 実際のフォーカス喪失で、ボタン解放を待たずに終了する。
        parent.movePointer(uiPixel(controls[6].first), uiPixel(controls[6].second)); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence.size() == 1; }, "knob before focus loss");
        {
            NativeParent other;
            other.unfocus();
            waitFor([&] { return handler.sequence.size() == 2; }, "knob focus loss");
            assert(other.focused()); // 別ウィンドウに移ったフォーカスを奪わない。
        }
        parent.unfocus();
        assert(handler.sequence == (std::vector<int>{0, 2}));
        waitFor([&] { return parent.cursorVisible(); }, "cursor restored after focus loss");
        parent.knobButton(false); pump(); handler.sequence.clear();
        // 押したまま画面を閉じても開始済み編集は終了する。
        parent.movePointer(uiPixel(controls[6].first), uiPixel(controls[6].second)); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence.size() == 1; }, "knob before close");
        pump(); parent.screenshot();
        assert(view->removed() == kResultOk);
        parent.knobButton(false); pump();
        assert(handler.sequence == (std::vector<int>{0, 2})); handler.sequence.clear();
    }
    assert(view->setFrame(nullptr) == kResultOk && frame.refs == 1);
    assert(view->release() == 0);
    assert(controller->setComponentHandler(nullptr) == kResultOk && handler.refs == 1);
#ifdef _WIN32
    assert(GuiReentry::injections == 228); // 114ノブ×2回の表示で再入と配送延期を検証。
#endif
    std::cout << "GUI: embedded editor, clicks, UI-thread gestures, restore and reopen passed\n";
}
