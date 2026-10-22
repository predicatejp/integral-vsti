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
    HWND window;
    NativeParent() {
        WNDCLASSW cls {};
        cls.lpfnWndProc = DefWindowProcW;
        cls.hInstance = GetModuleHandleW(nullptr);
        cls.lpszClassName = L"SampleVstiGuiTest";
        RegisterClassW(&cls);
        // 1024×768の画面でも、検証するクライアント領域全体を画面内に置く。
        window = CreateWindowExW(0, cls.lpszClassName, L"VST3 GUI test", WS_POPUP,
                                 0, 0, 800, 600, nullptr, nullptr, cls.hInstance, nullptr);
        assert(window); ShowWindow(window, SW_SHOW);
    }
    ~NativeParent() { DestroyWindow(window); }
    void* handle() { return window; }
    void resize(int32 width, int32 height) { SetWindowPos(window, nullptr, 0, 0, width, height, SWP_NOMOVE | SWP_NOZORDER); }
    static void pump() {
        MSG msg;
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
    void pointer(int x, int y, bool down) {
        HWND child = GetWindow(window, GW_CHILD); assert(child);
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
        HWND child = GetWindow(window, GW_CHILD);
        if (!child) return false;
        RECT rect; assert(GetClientRect(child, &rect));
        if (rect.right != width || rect.bottom != height) return false;
        HDC dc = GetDC(child); assert(dc);
        COLORREF pixel = GetPixel(dc, 20, height - 10);
        ReleaseDC(child, dc);
        return pixel != CLR_INVALID && pixel != RGB(0, 0, 0);
    }
    uint32 pixel(int x, int y) {
        HWND child = GetWindow(window, GW_CHILD); assert(child);
        HDC dc = GetDC(child); assert(dc);
        COLORREF value = GetPixel(dc, x, y);
        ReleaseDC(child, dc);
        assert(value != CLR_INVALID);
        return value;
    }
    void screenshot() {
        HWND child = GetWindow(window, GW_CHILD); assert(child);
        RECT rect; assert(GetClientRect(child, &rect));
        HDC dc = GetDC(child); assert(dc);
        std::ofstream file("target/gui-smoke.ppm", std::ios::binary);
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
    void screenshot() {
        const auto target = child(); XWindowAttributes attributes;
        assert(XGetWindowAttributes(display, target, &attributes));
        XImage* image = XGetImage(display, target, 0, 0, static_cast<unsigned>(attributes.width),
                                 static_cast<unsigned>(attributes.height), AllPlanes, ZPixmap);
        assert(image);
        std::ofstream file("target/gui-smoke.ppm", std::ios::binary);
        file << "P6\n" << attributes.width << ' ' << attributes.height << "\n255\n";
        for (int y = 0; y < attributes.height; ++y) for (int x = 0; x < attributes.width; ++x) {
            unsigned long pixel = XGetPixel(image, x, y);
            const char rgb[] = {static_cast<char>(pixel >> 16), static_cast<char>(pixel >> 8), static_cast<char>(pixel)};
            file.write(rgb, 3);
        }
        XDestroyImage(image);
    }
#endif
    void click(int x, int y) { pointer(x, y, true); pointer(x, y, false); }
    // 実際の子ウィンドウへキー入力を送る。0は未確定、27はEsc、13はEnter。
    void replaceText(const char* text, int finish = 13) {
#ifdef _WIN32
        HWND child = GetWindow(window, GW_CHILD); assert(child);
        auto key = [&](UINT code) {
            const LPARAM scan = static_cast<LPARAM>(MapVirtualKeyW(code, MAPVK_VK_TO_VSC)) << 16;
            SendMessageW(child, WM_KEYDOWN, code, scan | 1);
            SendMessageW(child, WM_KEYUP, code, scan | 0xc0000001);
        };
        key(VK_HOME);
        for (int i = 0; i < 32; ++i) key(VK_DELETE);
        for (const char* c = text; *c; ++c) SendMessageW(child, WM_CHAR, static_cast<WPARAM>(*c), 1);
        if (finish) key(static_cast<UINT>(finish));
#else
        auto key = [&](KeySym symbol) {
            const auto code = XKeysymToKeycode(display, symbol); assert(code);
            for (int type : {KeyPress, KeyRelease}) {
                XEvent event {};
                event.xkey = {type, 0, True, display, child(), DefaultRootWindow(display), 0,
                              CurrentTime, 0, 0, 0, 0, 0, code, True};
                XSendEvent(display, event.xkey.window, False,
                           type == KeyPress ? KeyPressMask : KeyReleaseMask, &event);
            }
        };
        key(XK_Home);
        for (int i = 0; i < 32; ++i) key(XK_Delete);
        for (const char* c = text; *c; ++c) key(static_cast<KeySym>(*c));
        if (finish) key(finish == 27 ? XK_Escape : XK_Return);
        XFlush(display);
#endif
    }
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
        assert(std::find(cutoffEnvelopeIds.begin(), cutoffEnvelopeIds.end(), id) != cutoffEnvelopeIds.end() || id == volumeId || id == phaseId || id == waveformId || id == unisonId || id == detuneId || id == polyphonyId || std::find(filterIds.begin(), filterIds.end(), id) != filterIds.end() || std::find(envelopeIds.begin(), envelopeIds.end(), id) != envelopeIds.end());
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
    assert(view->getSize(&rect) == kResultOk && rect.getWidth() == 1024 && rect.getHeight() == 768);
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
    assert(view->checkSizeConstraint(&rect) == kResultOk && rect.getWidth() == 1024);
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
        for (int i = 0; i < 50; ++i) {
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
    // リストの文字・枠が描画されるまで待つ。固定時間の待機では、ソフトウェアGLの
    // 初回描画が終わる前に項目をクリックしてしまうことがある。
    auto menuVisible = [&] {
        const auto background = parent.pixel(20, 130);
        int border = 0;
        for (int y = 100; y < 190; ++y) {
            if (parent.pixel(192, y) != background) ++border;
        }
        return border > 70;
    };
    for (int cycle = 0; cycle < 2; ++cycle) {
#ifdef _WIN32
        assert(view->attached(parent.handle(), kPlatformTypeHWND) == kResultOk);
#else
        assert(view->attached(parent.handle(), kPlatformTypeX11EmbedWindowID) == kResultOk);
#endif
        waitFor([&] { return parent.drawn(1024, 768); }, "first rendered frame");
        pump();
        assert(controller->setParamNormalized(waveformId, 0.0) == kResultOk);
        pump(); assert(handler.sequence.empty());
        parent.click(253, 80); // ドロップダウンを開くだけでは編集を通知しない。
        waitFor(menuVisible, "open Waveform list");
        assert(handler.sequence.empty());
        parent.click(253, 185);
        waitFor([&] { return handler.sequence.size() >= 3; }, "Sawtooth selection");
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        assert(handler.editedId == waveformId && handler.value == 1.0 && controller->getParamNormalized(waveformId) == 1.0);
        handler.sequence.clear();
        waitFor([&] { return !menuVisible(); }, "close Waveform list");
        parent.click(253, 80);
        waitFor(menuVisible, "reopen Waveform list");
        assert(handler.sequence.empty());
        parent.click(253, 110);
        waitFor([&] { return handler.sequence.size() >= 3; }, "Sine selection");
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        assert(handler.editedId == waveformId && handler.value == 0.0);
        handler.sequence.clear();
        // GUIの編集値を標準の変更キューへ配送し、DSPが保存する値を確認。
        Points point; point.values = {{0, handler.value}};
        Changes changes; changes.queues = {&point};
        ProcessData data; data.inputParameterChanges = &changes;
        assert(processor->process(data) == kResultOk);
        StateStream saved; assert(component->getState(&saved) == kResultOk && saved.bytes[8] == 0);
        // Componentの復元だけで表示を同期し、開いているGUIから再通知しない。
        saved.bytes[8] = 2;
        assert(component->setState(&saved) == kResultOk);
        assert(controller->getParamNormalized(waveformId) == 2.0 / 3.0);
        pump(); assert(handler.sequence.empty());
        // スライダーは左端0、右端最大を選べる。ホストからの復元値を丸めて再通知しない。
        for (size_t i = 0; i < envelopeIds.size(); ++i) {
            for (int x : {577, 188}) {
                const int y = 113 + static_cast<int>(i) * 29;
                parent.pointer(x, y, true); pump(); parent.pointer(x, y, false);
                waitFor([&] { return handler.sequence.size() >= 3; }, "envelope slider edit");
                assert(handler.sequence == (std::vector<int>{0, 1, 2}));
                assert(handler.editedId == envelopeIds[i]);
                assert(handler.value == (x == 577 ? 1.0 : 0.0));
                handler.sequence.clear(); pump();
            }
            if (i != 3) {
                // 0ms端点の追加後も、対数軸の10msと1000msをマウスで選べる。
                for (int x : {297, 501}) {
                    parent.pointer(x, 113 + static_cast<int>(i) * 29, true);
                    pump(); parent.pointer(x, 113 + static_cast<int>(i) * 29, false);
                    waitFor([&] { return handler.sequence.size() >= 3; }, "exact envelope time");
                    assert(handler.sequence == (std::vector<int>{0, 1, 2}));
                    assert(handler.editedId == envelopeIds[i]);
                    const double expected = x == 297 ? 10.0 : 1000.0;
                    assert(std::abs(controller->normalizedParamToPlain(handler.editedId, handler.value)
                                    - expected) < 1e-9);
                    handler.sequence.clear(); pump();
                }
            }
            assert(controller->setParamNormalized(envelopeIds[i], .123456789) == kResultOk);
            pump(); assert(handler.sequence.empty());
        }
        if (cycle == 0) {
            // 全整数tickをマウスで選択し、実単位への換算とホストへの反映を確認。
            // 390pxの操作欄・18pxフォントでのハンドル移動範囲を整数ピクセルへ丸める。
            for (int tick = 0; tick <= 371; ++tick) {
                const int x = static_cast<int>(std::lround(194.20625 + 377.5875 * tick / 371.0));
                const double ms = tick == 0 ? 0.0 : std::min(5000.0, std::pow(10.0, (tick - 1) / 100.0));
                const double expected = controller->plainParamToNormalized(envelopeIds[0], ms);
                parent.pointer(x, 113, true);
                waitFor([&] {
                    return std::abs(controller->getParamNormalized(envelopeIds[0]) - expected) < 1e-12;
                }, ("envelope integer tick " + std::to_string(tick)).c_str());
                parent.pointer(x, 113, false);
                for (int step = 0; step < 3; ++step) {
                    parent.pump(); frame.tick(); std::this_thread::sleep_for(std::chrono::milliseconds(10));
                }
                handler.sequence.clear();
            }
        }
        // 追加3行は整数で操作し、ホスト値の表示更新では編集通知しない。
        for (int row = 0; row < 3; ++row) {
            const ParamID id = row == 0 ? unisonId : (row == 1 ? detuneId : polyphonyId);
            for (int x : {577, 188}) {
                parent.pointer(x, 258 + row * 29, true); pump();
                parent.pointer(x, 258 + row * 29, false);
                waitFor([&] { return handler.sequence.size() >= 3; }, "unison/detune/polyphony slider edit");
                assert(handler.editedId == id && handler.value == (x == 577 ? 1.0 : 0.0));
                assert(handler.sequence == (std::vector<int>{0, 1, 2}));
                handler.sequence.clear(); pump();
            }
            assert(controller->setParamNormalized(id, .5) == kResultOk);
            pump(); assert(handler.sequence.empty());
        }
        // フィルタの新規行も整数UIから規定の編集通知を送る。
        for (int row = 0; row < 2; ++row) {
            const ParamID id = filterIds[row + 1];
            for (int x : {577, 188}) {
                parent.pointer(x, 374 + row * 29, true); pump();
                parent.pointer(x, 374 + row * 29, false);
                waitFor([&] { return handler.sequence.size() >= 3; }, "filter slider edit");
                assert(handler.editedId == id && handler.value == (x == 577 ? 1.0 : 0.0));
                assert(handler.sequence == (std::vector<int>{0, 1, 2}));
                handler.sequence.clear(); pump();
            }
            assert(controller->setParamNormalized(id, .3) == kResultOk);
            pump(); assert(handler.sequence.empty());
        }
        assert(controller->setParamNormalized(filterIds[0], 1.0 / 6.0) == kResultOk);
        pump(); parent.click(253, 345); pump();
        // 左枠の連続部分から、下側へ開いたリストの先頭項目の位置を得る。
        const auto background = parent.pixel(20, 130);
        int run = 0, best = 0, top = 0;
        for (int y = 360; y < 600; ++y) {
            run = parent.pixel(188, y) != background ? run + 1 : 0;
            if (run > best) { best = run; top = y - run + 1; }
        }
        assert(best > 100);
        parent.click(253, top + 14);
        waitFor([&] { return handler.sequence.size() >= 3; }, "filter type selection");
        assert(handler.editedId == filterIds[0] && handler.value == 0.0);
        handler.sequence.clear(); pump();
        if (cycle == 0) {
            // 各数値欄で、スライダーの刻みの間にある値もホストへそのまま渡す。
            const std::array<const char*, 20> text {"", "12.345", "0.125", "123.456", "0.4567",
                "0.0625", "3", "25.125", "19", "", "-1234.125", "1.2345", "", "", "12.345", "0.125", "123.456", "0.4567", "0.0625", "-1234.125"};
            for (int i = 1; i < 20; ++i) {
                if (i == 9 || i == 12 || i == 13) continue;
                ParameterInfo info; assert(controller->getParameterInfo(i, info) == kResultOk);
                parent.click(643, 84 + 29 * i); pump();
                parent.replaceText(text[i]);
                waitFor([&] { return handler.sequence.size() >= 3; }, ("numeric entry " + std::to_string(i)).c_str());
                assert(handler.sequence == (std::vector<int>{0, 1, 2}));
                assert(handler.editedId == info.id);
                const double plain = std::stod(text[i]);
                assert(std::abs(controller->normalizedParamToPlain(info.id, handler.value) - plain) < 2e-10);
                // 標準キューを通した後も、保存データに丸め前の実単位が残る。
                point.id = info.id; point.values = {{0, handler.value}};
                assert(processor->process(data) == kResultOk);
                StateStream state; assert(component->getState(&state) == kResultOk);
                double stored = 0; std::memcpy(&stored, state.bytes.data() + 12 + (i - 1) * 8, 8);
                assert(std::abs(stored - plain) < 2e-10);
                handler.sequence.clear();
            }
            // 範囲外は元の値へ戻し、Escも編集を取り消す。
            for (bool cancel : {false, true}) {
                parent.click(643, 403); pump();
                parent.replaceText(cancel ? "3.25" : "0.1", cancel ? 27 : 13); pump();
                assert(handler.sequence.empty());
                assert(std::abs(controller->normalizedParamToPlain(filterIds[2],
                    controller->getParamNormalized(filterIds[2])) - 1.2345) < 1e-12);
            }
            parent.click(643, 403); pump(); parent.replaceText("2.3456", 0); pump();
            assert(handler.sequence.empty());
            parent.click(30, 40);
            waitFor([&] { return handler.sequence.size() >= 3; }, "numeric entry focus loss");
            assert(std::abs(controller->normalizedParamToPlain(filterIds[2], handler.value) - 2.3456) < 1e-12);
            handler.sequence.clear(); pump();
        }
        // PhaseリストからRandomを選択し、状態へ保存する。
        assert(controller->setParamNormalized(phaseId, 0.0) == kResultOk);
        pump(); parent.click(253, 435); pump();
        const auto phaseBackground = parent.pixel(20, 130);
        int phaseRun = 0, phaseBest = 0, phaseTop = 0;
        for (int y = 450; y < 600; ++y) {
            phaseRun = parent.pixel(188, y) != phaseBackground ? phaseRun + 1 : 0;
            if (phaseRun > phaseBest) { phaseBest = phaseRun; phaseTop = y - phaseRun + 1; }
        }
        assert(phaseBest > 60);
        parent.click(253, phaseTop + 64);
        waitFor([&] { return handler.sequence.size() >= 3; }, "Random phase selection");
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        assert(handler.editedId == phaseId && handler.value == 1.0);
        point.id = phaseId; point.values = {{0, handler.value}};
        assert(processor->process(data) == kResultOk);
        StateStream phaseState; assert(component->getState(&phaseState) == kResultOk);
        double phaseMode = -1;
        std::memcpy(&phaseMode, phaseState.bytes.data() + 100, 8);
        assert(phaseMode == 2.0);
        handler.sequence.clear(); pump();
        // Volumeの整数スライダー両端と直接入力を検証し、実単位で保存する。
        for (int x : {577, 188}) {
            parent.pointer(x, 464, true); pump(); parent.pointer(x, 464, false);
            waitFor([&] { return handler.sequence.size() >= 3; }, "volume slider edit");
            assert(handler.sequence == (std::vector<int>{0, 1, 2}));
            assert(handler.editedId == volumeId && handler.value == (x == 577 ? 1.0 : 0.0));
            handler.sequence.clear(); pump();
        }
        parent.click(643, 464); pump(); parent.replaceText("-12.345");
        waitFor([&] { return handler.sequence.size() >= 3; }, "volume numeric entry");
        assert(handler.sequence == (std::vector<int>{0, 1, 2}) && handler.editedId == volumeId);
        point.id = volumeId; point.values = {{0, handler.value}};
        assert(processor->process(data) == kResultOk);
        StateStream volumeState; assert(component->getState(&volumeState) == kResultOk);
        double volumeDb = 0; std::memcpy(&volumeDb, volumeState.bytes.data() + 108, 8);
        assert(std::abs(volumeDb + 12.345) < 1e-12);
        handler.sequence.clear(); pump();
        parent.screenshot();
        assert(view->removed() == kResultOk);
        pump(); assert(handler.sequence.empty());
    }
    assert(view->setFrame(nullptr) == kResultOk && frame.refs == 1);
    assert(view->release() == 0);
    assert(controller->setComponentHandler(nullptr) == kResultOk && handler.refs == 1);
    std::cout << "GUI: embedded editor, clicks, UI-thread gestures, restore and reopen passed\n";
}
