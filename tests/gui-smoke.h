// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// 検証専用の親ウィンドウとVST3コールバック。本体へリンクしない。
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"
#include <chrono>
#include <thread>
#include <algorithm>
#include <fstream>
#ifndef _WIN32
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
        // 1024×768の画面でも、検証するクライアント領域全体を画面内に置く。
        window = CreateWindowExW(0, cls.lpszClassName, L"VST3 GUI test", WS_POPUP,
                                 0, 0, 800, 600, nullptr, nullptr, cls.hInstance, nullptr);
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
        window = XCreateSimpleWindow(display, DefaultRootWindow(display), 0, 0, 800, 600, 0, 0, 0);
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
        assert(SetCursorPos(point.x, point.y));
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
        assert(std::this_thread::get_id() == uiThread);
        assert(id == 0x4f32464d || id == 0x4f33464d || id == 0x4f31564c || id == 0x4f325746 || id == 0x4f32544e || id == 0x4f32564c || id == 0x4f335746 || id == 0x4f33544e || id == 0x4f33564c || id == 0x57494454 || id == 0x4d50414e || std::find(cutoffEnvelopeIds.begin(), cutoffEnvelopeIds.end(), id) != cutoffEnvelopeIds.end() || id == volumeId || id == phaseId || id == waveformId || id == unisonId || id == detuneId || id == polyphonyId || std::find(filterIds.begin(), filterIds.end(), id) != filterIds.end() || std::find(envelopeIds.begin(), envelopeIds.end(), id) != envelopeIds.end());
        editedId = id;
        sequence.push_back(0); return kResultOk;
    }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue v) override {
        assert(std::this_thread::get_id() == uiThread && id == editedId);
        sequence.push_back(1); value = v;
        // ホストからControllerへ再入しても循環・借用違反を起こさない。
        assert(controller->setParamNormalized(id, v) == kResultOk);
        return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID id) override {
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
    assert(view->getSize(&rect) == kResultOk && rect.getWidth() == 856 && rect.getHeight() == 380);
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
    assert(view->checkSizeConstraint(&rect) == kResultOk && rect.getWidth() == 856);
    assert(view->onKeyDown(u' ', 0, 0) == kResultFalse);
    assert(view->onKeyUp(u' ', 0, 0) == kResultFalse);
    assert(view->onWheel(1) == kResultFalse);
    assert(view->onFocus(true) == kResultOk);
    assert(view->removed() == kResultOk);
    assert(view->release() == 0);
}

void checkGui(IComponent* component, IAudioProcessor* processor, IEditController* controller) {
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
        std::cerr << "GUI timeout: " << description << ", edit notifications=" << handler.sequence.size() << std::endl;
        assert(false);
    };
    // グループ内のノブ中心と選択リスト。画面上の実入力で全設定を検証する。
    const std::array<std::pair<int, int>, 31> controls {{
        {231, 28}, {353, 322}, {418, 322}, {484, 322}, {550, 322}, {615, 322},
        {51, 74}, {121, 74}, {756, 120}, {583, 28}, {375, 80}, {484, 80},
        {790, 70}, {756, 204}, {353, 184}, {418, 184}, {484, 184}, {550, 184},
        {615, 184}, {593, 80}, {191, 74}, {756, 288}, {261, 74},
        {231, 152}, {63, 198}, {156, 198}, {231, 276}, {63, 322}, {156, 322}, {249, 198}, {249, 322}
    }};
    auto choose = [&](int index, int item) {
        const auto [x, y] = controls[index];
        parent.click(x, y); pump(); assert(handler.sequence.empty());
        parent.screenshot(("target/gui-menu-" + std::to_string(index) + ".ppm").c_str());
        parent.click(x, (index == 26 ? y - 154 : y + 34) + item * 30);
        waitFor([&] { return handler.sequence.size() >= 3; }, "grouped choice");
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        handler.sequence.clear(); pump();
    };
    for (int cycle = 0; cycle < 2; ++cycle) {
#ifdef _WIN32
        assert(view->attached(parent.handle(), kPlatformTypeHWND) == kResultOk);
#else
        assert(view->attached(parent.handle(), kPlatformTypeX11EmbedWindowID) == kResultOk);
#endif
        waitFor([&] { return parent.drawn(856, 380); }, "first rendered frame");
        pump();
        if (cycle == 0) parent.screenshot("target/gui-panel.ppm");
        assert(controller->setParamNormalized(waveformId, 0.0) == kResultOk);
        pump(); assert(handler.sequence.empty());
        choose(0, 4);
        assert(handler.editedId == waveformId && handler.value == 1.0);
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
            pump(); choose(index, 4);
            assert(handler.editedId == info.id && handler.value == 1.0);
            choose(index, 0);
            assert(handler.editedId == info.id && handler.value == 0.0);
        }
        // 全26ノブを実マウス入力で操作する。1ドラッグはbegin/value/endの一組。
        parent.unfocus(); pump(); assert(parent.focused());
        for (int i = 1; i < 31; ++i) {
            if (i == 9 || i == 12 || i == 23 || i == 26) continue;
            ParameterInfo info; assert(controller->getParameterInfo(i, info) == kResultOk);
            assert(controller->setParamNormalized(info.id, 0.0) == kResultOk);
            pump(); assert(handler.sequence.empty());
            const auto [x, y] = controls[i];
            parent.movePointer(x, y); pump(); parent.knobButton(true);
            waitFor([&] { return handler.sequence.size() == 1; }, "knob begin");
            assert(handler.sequence == (std::vector<int>{0}));
            assert(!parent.cursorVisible());
            // 全ノブで224×60 pxの同じ枠が画面内に描画される。
            const int overlayX = std::clamp(x - 112, 0, 632);
            const int overlayY = std::clamp(y - 91, 0, 320);
#ifdef _WIN32
            const uint32 overlayColor = RGB(23, 43, 38);
#else
            const uint32 overlayColor = 0x172b26;
#endif
            waitFor([&] {
                return parent.pixel(overlayX + 4, overlayY + 4) == overlayColor &&
                       parent.pixel(overlayX + 219, overlayY + 4) == overlayColor &&
                       parent.pixel(overlayX + 4, overlayY + 55) == overlayColor &&
                       parent.pixel(overlayX + 219, overlayY + 55) == overlayColor;
            }, "fixed overlay bounds");
#ifdef _WIN32
            assert(parent.focused()); // 操作中にもホストの入力先を維持する。
#endif
            parent.movePointer(x, y - 3); pump(); // 4px未満は保持。
            assert(handler.sequence.size() == 1 && controller->getParamNormalized(info.id) == 0.0);
            assert(parent.pointerAt(x, y));
            parent.movePointer(x, y - 1);
            waitFor([&] { return handler.sequence.size() == 2; }, "knob integer tick");
            assert(handler.sequence == (std::vector<int>{0, 1}) && handler.editedId == info.id);
            const bool time = i == 1 || i == 2 || i == 3 || i == 5 || i == 14 || i == 15 || i == 16 || i == 18;
            const double plain = time ? 1.0 : (i == 6 || i == 8) ? 2.0 : i == 7 ? 1.0 : i == 10 ? -2390.0 :
                                 i == 11 ? .51 : (i == 13 || i == 22 || i == 25 || i == 28) ? -48.0 : (i == 24 || i == 27) ? -2390.0 : i == 19 ? -4790.0 : i == 20 ? 1.0 : i == 21 ? -99.0 : (i == 29 || i == 30) ? 1.0 : .01;
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
            assert(parent.cursorVisible());
            handler.sequence.clear();
            assert(controller->setParamNormalized(info.id, .123456789) == kResultOk);
            pump(); assert(handler.sequence.empty());
        }
        assert(controller->setParamNormalized(filterIds[0], 1.0 / 6.0) == kResultOk);
        pump(); choose(9, 6);
        assert(handler.editedId == filterIds[0] && handler.value == 1.0);
        choose(9, 0);
        assert(handler.editedId == filterIds[0] && handler.value == 0.0);
        assert(controller->setParamNormalized(phaseId, 0.0) == kResultOk);
        pump(); choose(12, 2);
        assert(handler.editedId == phaseId && handler.value == 1.0);
        point.id = phaseId; point.values = {{0, handler.value}};
        assert(processor->process(data) == kResultOk);
        StateStream phaseState; assert(component->getState(&phaseState) == kResultOk);
        double phaseMode = -1; std::memcpy(&phaseMode, phaseState.bytes.data() + 100, 8);
        assert(phaseMode == 2.0);
#ifdef _WIN32
        parent.movePointer(controls[13].first, controls[13].second); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence.size() == 1; }, "knob before escape");
        INPUT escape {}; escape.type = INPUT_KEYBOARD; escape.ki.wVk = VK_ESCAPE;
        assert(SendInput(1, &escape, sizeof(INPUT)) == 1);
        waitFor([&] { return handler.sequence.size() == 2; }, "escape without plug-in focus");
        assert(parent.focused() && parent.cursorVisible());
        escape.ki.dwFlags = KEYEVENTF_KEYUP;
        assert(SendInput(1, &escape, sizeof(INPUT)) == 1);
        parent.knobButton(false); pump(); handler.sequence.clear();
#endif
        // 実際のフォーカス喪失で、ボタン解放を待たずに終了する。
        parent.movePointer(controls[6].first, controls[6].second); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence.size() == 1; }, "knob before focus loss");
        {
            NativeParent other;
            other.unfocus();
            waitFor([&] { return handler.sequence.size() == 2; }, "knob focus loss");
            assert(other.focused()); // 別ウィンドウに移ったフォーカスを奪わない。
        }
        parent.unfocus();
        assert(handler.sequence == (std::vector<int>{0, 2}));
        assert(parent.cursorVisible());
        parent.knobButton(false); pump(); handler.sequence.clear();
        // 押したまま画面を閉じても開始済み編集は終了する。
        parent.movePointer(controls[6].first, controls[6].second); pump(); parent.knobButton(true);
        waitFor([&] { return handler.sequence.size() == 1; }, "knob before close");
        pump(); parent.screenshot();
        assert(view->removed() == kResultOk);
        parent.knobButton(false); pump();
        assert(handler.sequence == (std::vector<int>{0, 2})); handler.sequence.clear();
    }
    assert(view->setFrame(nullptr) == kResultOk && frame.refs == 1);
    assert(view->release() == 0);
    assert(controller->setComponentHandler(nullptr) == kResultOk && handler.refs == 1);
    std::cout << "GUI: embedded editor, clicks, UI-thread gestures, restore and reopen passed\n";
}
