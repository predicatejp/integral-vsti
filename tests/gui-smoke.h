// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// 検証専用の親ウィンドウとVST3コールバック。本体へリンクしない。
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"
#include <chrono>
#include <thread>
#include <algorithm>
#ifndef _WIN32
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <fstream>
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
        window = CreateWindowExW(0, cls.lpszClassName, L"VST3 GUI test", WS_OVERLAPPEDWINDOW,
                                 50, 50, 600, 400, nullptr, nullptr, cls.hInstance, nullptr);
        assert(window); ShowWindow(window, SW_SHOW);
    }
    ~NativeParent() { DestroyWindow(window); }
    void* handle() { return window; }
    void resize(int32 width, int32 height) { SetWindowPos(window, nullptr, 0, 0, width + 32, height + 64, SWP_NOMOVE | SWP_NOZORDER); }
    static void pump() {
        MSG msg;
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
    void click(int x, int y) {
        HWND child = GetWindow(window, GW_CHILD); assert(child);
        LPARAM position = MAKELPARAM(x, y);
        SendMessageW(child, WM_MOUSEMOVE, 0, position);
        SendMessageW(child, WM_LBUTTONDOWN, MK_LBUTTON, position);
        SendMessageW(child, WM_LBUTTONUP, 0, position);
    }
    void screenshot() {}
#else
    Display* display;
    ::Window window;
    NativeParent() {
        display = XOpenDisplay(nullptr); assert(display);
        window = XCreateSimpleWindow(display, DefaultRootWindow(display), 0, 0, 600, 400, 0, 0, 0);
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
    void click(int x, int y) {
        const auto target = child();
        XEvent event {};
        event.xmotion = {MotionNotify, 0, True, display, target, DefaultRootWindow(display), 0,
                         CurrentTime, x, y, x, y, 0, NotifyNormal, True};
        XSendEvent(display, target, False, PointerMotionMask, &event);
        event.xbutton = {ButtonPress, 0, True, display, target, DefaultRootWindow(display), 0,
                         CurrentTime, x, y, x, y, 0, Button1, True};
        XSendEvent(display, target, False, ButtonPressMask, &event);
        event.xbutton.type = ButtonRelease;
        XSendEvent(display, target, False, ButtonReleaseMask, &event);
        XFlush(display);
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
    tresult PLUGIN_API queryInterface(const TUID iid, void** out) override {
        *out = nullptr;
        if (std::memcmp(iid, IPlugFrame_iid, 16) == 0 || std::memcmp(iid, FUnknown_iid, 16) == 0)
            *out = static_cast<IPlugFrame*>(this);
#ifndef _WIN32
        if (std::memcmp(iid, Linux::IRunLoop_iid, 16) == 0) *out = static_cast<Linux::IRunLoop*>(this);
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
    std::thread::id uiThread = std::this_thread::get_id();
    IEditController* controller;
    explicit GuiHandler(IEditController* c) : controller(c) {}
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { assert(refs > 1); return --refs; }
    tresult PLUGIN_API beginEdit(ParamID id) override {
        assert(std::this_thread::get_id() == uiThread && id == waveformId);
        sequence.push_back(0); return kResultOk;
    }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue v) override {
        assert(std::this_thread::get_id() == uiThread && id == waveformId);
        sequence.push_back(1); value = v;
        // ホストからControllerへ再入しても循環・借用違反を起こさない。
        assert(controller->setParamNormalized(id, v) == kResultOk);
        return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID id) override {
        assert(std::this_thread::get_id() == uiThread && id == waveformId);
        sequence.push_back(2); return kResultOk;
    }
    tresult PLUGIN_API restartComponent(int32) override { assert(false); return kNotImplemented; }
};

void checkViewContract(IEditController* controller) {
    assert(controller->createView("unknown") == nullptr);
    auto* view = controller->createView("editor"); assert(view);
    assert(view->canResize() == kResultFalse);
    ViewRect rect;
    assert(view->getSize(&rect) == kResultOk && rect.getWidth() == 360 && rect.getHeight() == 180);
    assert(view->isPlatformTypeSupported("unknown") == kResultFalse);
#ifdef _WIN32
    const auto platform = kPlatformTypeHWND;
#else
    const auto platform = kPlatformTypeX11EmbedWindowID;
#endif
    assert(view->isPlatformTypeSupported(platform) == kResultOk);
    assert(view->attached(nullptr, platform) == kInvalidArgument);
    void* result = nullptr;
    assert(view->queryInterface(IPlugViewContentScaleSupport_iid, &result) == kResultOk);
    auto* scale = static_cast<IPlugViewContentScaleSupport*>(result);
    assert(scale->setContentScaleFactor(1.5f) == kResultOk);
    assert(view->getSize(&rect) == kResultOk && rect.getWidth() == 540 && rect.getHeight() == 270);
    assert(scale->setContentScaleFactor(std::numeric_limits<float>::quiet_NaN()) == kInvalidArgument);
    assert(view->onKeyDown(u' ', 0, 0) == kResultFalse);
    assert(view->onKeyUp(u' ', 0, 0) == kResultFalse);
    assert(view->onWheel(1) == kResultFalse);
    assert(view->onFocus(true) == kResultOk);
    assert(view->removed() == kResultOk);
    assert(scale->release() == 1);
    assert(view->release() == 0);
}

void checkGui(IComponent* component, IAudioProcessor* processor, IEditController* controller) {
    NativeParent parent;
    GuiFrame frame; frame.parent = &parent;
    GuiHandler handler(controller);
    assert(controller->setComponentHandler(&handler) == kResultOk && handler.refs == 2);
    auto* view = controller->createView("editor"); assert(view);
    void* result = nullptr;
    assert(view->queryInterface(IPlugViewContentScaleSupport_iid, &result) == kResultOk);
    auto* scale = static_cast<IPlugViewContentScaleSupport*>(result);
    assert(view->setFrame(&frame) == kResultOk);
    auto pump = [&] {
        for (int i = 0; i < 50; ++i) {
            parent.pump(); frame.tick(); std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
    };
    for (int cycle = 0; cycle < 2; ++cycle) {
        assert(scale->setContentScaleFactor(1.0f) == kResultOk);
#ifdef _WIN32
        assert(view->attached(parent.handle(), kPlatformTypeHWND) == kResultOk);
#else
        assert(view->attached(parent.handle(), kPlatformTypeX11EmbedWindowID) == kResultOk);
#endif
        pump();
        assert(controller->setParamNormalized(waveformId, 0.0) == kResultOk);
        pump(); assert(handler.sequence.empty());
        parent.click(300, 100); pump();
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        assert(handler.value == 1.0 && controller->getParamNormalized(waveformId) == 1.0);
        handler.sequence.clear();
        assert(scale->setContentScaleFactor(1.5f) == kResultOk);
        pump(); parent.click(70, 150); pump();
        assert(handler.sequence == (std::vector<int>{0, 1, 2}));
        assert(handler.value == 0.0);
        handler.sequence.clear();
        // GUIの編集値を標準の変更キューへ配送し、DSPが保存する値を確認。
        Points point; point.values = {{0, handler.value}};
        Changes changes; changes.queues = {&point};
        ProcessData data; data.inputParameterChanges = &changes;
        assert(processor->process(data) == kResultOk);
        StateStream saved; assert(component->getState(&saved) == kResultOk && saved.bytes[8] == 0);
        // ホストによる復元は通知を発生させない。
        saved.bytes[8] = 2;
        assert(component->setState(&saved) == kResultOk);
        saved.position = 0;
        assert(controller->setComponentState(&saved) == kResultOk);
        pump(); assert(handler.sequence.empty());
        parent.screenshot();
        assert(view->removed() == kResultOk);
        pump(); assert(handler.sequence.empty());
    }
    assert(view->setFrame(nullptr) == kResultOk && frame.refs == 1);
    scale->release(); assert(view->release() == 0);
    assert(controller->setComponentHandler(nullptr) == kResultOk && handler.refs == 1);
    std::cout << "GUI: embedded editor, clicks, UI-thread gestures, restore, scaling and reopen passed\n";
}
