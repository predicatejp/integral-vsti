// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 検証専用。SDKのC++インターフェースからRust共有ライブラリを直接呼び出す。
// このコードとSDKはプラグイン本体にリンクしない。
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/vstspeaker.h"

#include <array>
#include <cassert>
#include <cstddef>
#include <cstring>
#include <cmath>
#include <vector>
#include <limits>
#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif
#include <string>
#include <iostream>
#include <type_traits>

using namespace Steinberg;
using namespace Steinberg::Vst;

// Rust側のlayoutテストと対にし、公式ヘッダのレイアウトを独立して確認する。
static_assert(sizeof(PFactoryInfo) == 452);
static_assert(sizeof(PClassInfo) == 116);
static_assert(sizeof(PClassInfo2) == 440);
static_assert(sizeof(BusInfo) == 276);
static_assert(sizeof(RoutingInfo) == 12);
static_assert(sizeof(ProcessSetup) == 24);
static_assert(offsetof(ProcessSetup, sampleRate) == 16);
static_assert(sizeof(AudioBusBuffers) == 24);
static_assert(offsetof(AudioBusBuffers, silenceFlags) == 8);
static_assert(offsetof(AudioBusBuffers, channelBuffers32) == 16);
static_assert(sizeof(ProcessData) == 80);
static_assert(alignof(ProcessData) == 8);
static_assert(offsetof(ProcessData, inputs) == 24);
static_assert(offsetof(ProcessData, processContext) == 72);

static_assert(sizeof(NoteOnEvent) == 20 && offsetof(NoteOnEvent, noteId) == 16);
static_assert(sizeof(NoteOffEvent) == 16 && offsetof(NoteOffEvent, noteId) == 8);
static_assert(sizeof(Event) == 48 && alignof(Event) == 8);
static_assert(offsetof(Event, type) == 18 && offsetof(Event, noteOn) == 24);
static_assert(sizeof(NoteExpressionTextEvent) == 24);

// ホスト側の独立したIEventList実装。借用中にretain/release/addEventされないことも検査。
class Events final : public IEventList {
public:
    std::vector<Event> values;
    int32 failedIndex = -1;
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { assert(false); return 1; }
    uint32 PLUGIN_API release() override { assert(false); return 1; }
    int32 PLUGIN_API getEventCount() override { return static_cast<int32>(values.size()); }
    tresult PLUGIN_API getEvent(int32 index, Event& event) override {
        if (index == failedIndex) return kResultFalse;
        event = values.at(static_cast<size_t>(index));
        return kResultOk;
    }
    tresult PLUGIN_API addEvent(Event&) override { assert(false); return kNotImplemented; }
};

Event note(bool on, int32 offset, int32 id, int16 pitch = 69, int16 channel = 0, float velocity = 1.0f) {
    Event event {};
    event.sampleOffset = offset;
    event.type = on ? Event::kNoteOnEvent : Event::kNoteOffEvent;
    if (on) event.noteOn = {channel, pitch, 0.0f, velocity, 0, id};
    else event.noteOff = {channel, pitch, 0.0f, id, 0.0f};
    return event;
}

template <typename Sample>
void checkNotes(IComponent* component, IAudioProcessor* processor, int32 precision, double rate, int32 mode,
                bool activateEvents) {
    ProcessSetup setup {mode, precision, 257, rate};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    // 有効化通知を省略するホストの経路でも、同じ波形・イベント時刻を検証する。
    if (activateEvents) assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    std::array<Sample, 259> left, right;
    Sample* channels[] = {left.data() + 1, right.data() + 1};
    AudioBusBuffers bus;
    bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    Events events;
    ProcessData data;
    data.processMode = mode;
    data.symbolicSampleSize = precision;
    data.numOutputs = 1;
    data.outputs = &bus;
    data.inputEvents = &events;
    double phase = 0.0;
    // 期待する音程・振幅の区間をテスト側で明示する。位相は音が出るサンプルだけ進む。
    auto check = [&](int32 length, auto expected) {
        data.numSamples = length;
        left.fill(Sample(0.75)); right.fill(Sample(0.75));
        assert(processor->process(data) == kResultOk);
        bool nonzero = false;
        for (int32 i = 0; i < length; ++i) {
            const auto [frequency, amplitude] = expected(i);
            double sample = 0.0;
            if (frequency > 0.0) {
                sample = amplitude * std::sin(2.0 * std::acos(-1.0) * phase);
                phase = std::fmod(phase + frequency / rate, 1.0);
            }
            const auto rounded = Sample(sample);
            nonzero |= rounded != Sample(0);
            for (auto* channel : channels) {
                assert(std::isfinite(channel[i]));
                assert(std::abs(channel[i] - rounded) < 1e-6);
            }
        }
        assert(bus.silenceFlags == (nonzero ? 0u : 3u));
        for (auto* channel : channels) { assert(channel[-1] == Sample(0.75)); assert(channel[length] == Sample(0.75)); }
    };
    using Tone = std::array<double, 2>;
    auto silence = [](int32) { return Tone {0.0, 0.0}; };
    auto high = [](int32) { return Tone {880.0, 0.25 * std::sqrt(2.0)}; };
    auto low = [](int32) { return Tone {440.0, 0.5 * std::sqrt(2.0)}; };
    events.values = {note(true, 0, 10), note(true, 3, 11, 81, 0, 0.5f), note(false, 5, 10),
                     note(false, 7, 11, 81), note(true, 10, 12), note(false, 15, 12),
                     note(true, 16, 13, 81, 0, 0.5f)};
    check(17, [](int32 i) {
        if (i < 3 || (i >= 10 && i < 15)) return Tone {440.0, 0.5 * std::sqrt(2.0)};
        if ((i >= 3 && i < 7) || i == 16) return Tone {880.0, 0.25 * std::sqrt(2.0)};
        return Tone {0.0, 0.0};
    });
    data.inputEvents = nullptr;
    for (int32 length : {1, 7, 256, 257}) check(length, high);
    channels[1] = channels[0]; // 発音時の左右エイリアスでも位相を二重に進めない。
    check(7, high);
    channels[1] = right.data() + 1;
    data.inputEvents = &events;
    // 出力バッファのないゼロサンプルでもノートオフを処理する。
    events.values = {note(false, 0, 13, 81)};
    data.numSamples = 0; data.numOutputs = 0; data.outputs = nullptr;
    assert(processor->process(data) == kResultOk);
    data.numOutputs = 1; data.outputs = &bus;
    events.values.clear(); check(7, silence);
    // 同時刻はリスト順。IDなしはチャンネルと音高で照合する。
    events.values = {note(true, 0, -1, 69, 2), note(false, 0, -1, 69, 3), note(false, 0, -1, 70, 2)};
    check(7, low);
    events.values = {note(false, 0, -1, 69, 2), note(true, 0, 20)};
    check(7, low);
    // 不正・未対応イベントは現在の発音を壊さない。getEvent失敗も読み出さない。
    events.values = {note(false, 0, 20), note(false, -1, 20), note(false, 7, 20),
                     note(true, 0, 30, 128), note(true, 0, 30, 69, 16),
                     note(true, 0, 30, 69, 0, std::numeric_limits<float>::quiet_NaN()),
                     note(false, 0, 20), note(false, 0, 20)};
    events.failedIndex = 0;
    events.values[6].busIndex = 1;
    events.values[7].type = Event::kDataEvent;
    check(7, low);
    events.failedIndex = -1;
    // 後戻りするoffsetは無視する。
    events.values = {note(true, 3, 20), note(false, 2, 20)};
    check(7, low);
    // 非アクティブ出力でもノートオフを落とさない。
    assert(component->activateBus(kAudio, kOutput, 0, false) == kResultOk);
    events.values = {note(false, 0, 20)};
    bus.channelBuffers32 = nullptr;
    assert(processor->process(data) == kResultOk);
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    events.values.clear(); check(7, silence);
    events.values = {note(true, 0, 21)}; check(7, low);
    // 処理停止でゲートを閉じる。再開しても勝手に鳴らず、再発音は位相を継承。
    assert(processor->setProcessing(false) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    events.values.clear(); check(7, silence);
    events.values = {note(true, 0, 22)}; check(7, low);
    assert(component->activateBus(kEvent, kInput, 0, false) == kResultOk);
    check(7, silence);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    // 明示的な無効化は再開しても維持される。受信済みのNote Onも復活させない。
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    check(7, silence);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);
    events.values.clear(); check(7, silence);
    events.values = {note(true, 0, 23)}; check(7, low);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
}

template <typename T>
T* query(FUnknown* object, const TUID iid) {
    void* result = nullptr;
    assert(object->queryInterface(iid, &result) == kResultOk);
    assert(result);
    return static_cast<T*>(result);
}

template <typename Sample>
void checkSilence(IComponent* component, IAudioProcessor* processor, int32 precision) {
    ProcessSetup setup {kRealtime, precision, 257, 48000.0};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk);
    assert(processor->setProcessing(true) == kResultOk);

    // 前後の番兵で範囲外書き込みを検知。両出力が同じ領域を指すケースも試す。
    for (bool alias : {false, true}) {
        for (int32 length : {1, 7, 256, 257}) {
            std::array<Sample, 259> left, right;
            left.fill(Sample(0.75));
            right.fill(Sample(0.75));
            Sample* channels[] = {left.data() + 1, alias ? left.data() + 1 : right.data() + 1};
            AudioBusBuffers bus;
            bus.numChannels = 2;
            if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
            else bus.channelBuffers64 = channels;
            ProcessData data;
            data.symbolicSampleSize = precision;
            data.numSamples = length;
            data.numOutputs = 1;
            data.outputs = &bus;
            assert(processor->process(data) == kResultOk);
            assert(bus.silenceFlags == 3);
            for (auto* channel : channels) {
                assert(channel[-1] == Sample(0.75));
                for (int32 i = 0; i < length; ++i) assert(channel[i] == Sample(0));
                assert(channel[length] == Sample(0.75));
            }
            data.numSamples = 258; // 最大長を超える要求は書き込み前に拒否。
            assert(processor->process(data) == kInvalidArgument);
        }
    }

    // 非アクティブバスのnullバッファを参照しない。
    assert(component->activateBus(kAudio, kOutput, 0, false) == kResultOk);
    AudioBusBuffers inactive;
    inactive.numChannels = 2;
    ProcessData data;
    data.symbolicSampleSize = precision;
    data.numSamples = 7;
    data.numOutputs = 1;
    data.outputs = &inactive;
    assert(processor->process(data) == kResultOk);
    assert(processor->setProcessing(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    assert(component->setActive(false) == kResultOk);
    // 停止中のゼロサンプルフラッシュ。
    ProcessData flush;
    flush.symbolicSampleSize = precision;
    assert(processor->process(flush) == kResultOk);
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    assert(argc == 3);
    HMODULE module = LoadLibraryW(argv[1]);
    if (!module) { std::cerr << "LoadLibrary failed: " << GetLastError() << '\n'; return 1; }
    const auto entry = reinterpret_cast<bool (PLUGIN_API*)()>(GetProcAddress(module, "InitDll"));
    const auto exit = reinterpret_cast<bool (PLUGIN_API*)()>(GetProcAddress(module, "ExitDll"));
    const auto getFactory = reinterpret_cast<IPluginFactory* (PLUGIN_API*)()>(GetProcAddress(module, "GetPluginFactory"));
    assert(entry && exit && getFactory && entry());
    const std::wstring wideVersion(argv[2]);
    const std::string expectedVersion(wideVersion.begin(), wideVersion.end());
#else
int main(int argc, char** argv) {
    assert(argc == 3);
    void* module = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!module) { std::cerr << dlerror() << '\n'; return 1; }
    const auto entry = reinterpret_cast<bool (*)(void*)>(dlsym(module, "ModuleEntry"));
    const auto exit = reinterpret_cast<bool (*)()>(dlsym(module, "ModuleExit"));
    const auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(dlsym(module, "GetPluginFactory"));
    assert(entry && exit && getFactory && entry(module));
    const std::string expectedVersion(argv[2]);
#endif
    auto* factory = getFactory();
    auto* factory2 = query<IPluginFactory2>(factory, IPluginFactory2_iid);
    auto* factoryIdentity = query<FUnknown>(factory2, FUnknown_iid);
    assert(factoryIdentity == factory);
    factoryIdentity->release();
    PFactoryInfo vendor;
    assert(factory->getFactoryInfo(&vendor) == kResultOk);
    assert(std::strcmp(vendor.vendor, "sample-vsti") == 0);
    assert(factory->countClasses() == 1);
    PClassInfo2 info;
    assert(factory2->getClassInfo2(0, &info) == kResultOk);
    assert(std::strcmp(info.subCategories, "Instrument|Synth") == 0);
    assert(std::strcmp(info.version, expectedVersion.c_str()) == 0);
    assert(std::strcmp(info.sdkVersion, kVstVersionString) == 0);
    const TUID expectedClassId = INLINE_UID(0x9ad1ae8c, 0xb51b4aac, 0xa13c85c0, 0xb023ab4d);
    assert(std::memcmp(info.cid, expectedClassId, sizeof(TUID)) == 0);
    void* invalid = factory;
    const TUID unknown = {};
    assert(factory->queryInterface(unknown, &invalid) == kNoInterface && invalid == nullptr);
    assert(factory2->getClassInfo2(1, &info) == kInvalidArgument);
    factory2->release();

    for (int iteration = 0; iteration < 100; ++iteration) {
        void* result = nullptr;
        assert(factory->createInstance(info.cid, IComponent_iid, &result) == kResultOk);
        auto* component = static_cast<IComponent*>(result);
        auto* processor = query<IAudioProcessor>(component, IAudioProcessor_iid);
        auto* controller = query<IEditController>(component, IEditController_iid);
        auto* requirements = query<IProcessContextRequirements>(processor, IProcessContextRequirements_iid);
        assert(requirements->getProcessContextRequirements() == 0);
        requirements->release();
        auto* identity = query<FUnknown>(controller, FUnknown_iid);
        assert(identity == component);
        identity->release();
        // このプラグインはホストのサービスを使わないため、有効なFUnknownだけを渡す。
        assert(component->initialize(factory) == kResultOk);
        assert(component->getBusCount(kAudio, kInput) == 0);
        assert(component->getBusCount(kAudio, kOutput) == 1);
        assert(component->getBusCount(kEvent, kInput) == 1);
        BusInfo bus;
        assert(component->getBusInfo(kAudio, kOutput, 0, bus) == kResultOk);
        assert(bus.channelCount == 2 && bus.name[0] == u'S');
        SpeakerArrangement output = SpeakerArr::kStereo;
        assert(processor->setBusArrangements(nullptr, 0, &output, 1) == kResultOk);
        assert(controller->getParameterCount() == 0);
        assert(controller->createView("editor") == nullptr);
        assert(controller->setComponentHandler(nullptr) == kResultOk);
        assert(processor->getLatencySamples() == 0 && processor->getTailSamples() == 0);
        checkSilence<float>(component, processor, kSample32);
        checkSilence<double>(component, processor, kSample64);
        if (iteration < 12) {
            const double rate = std::array<double, 3> {44100.0, 48000.0, 96000.0}[iteration % 3];
            if (iteration % 2 == 0) checkNotes<float>(component, processor, kSample32, rate, iteration % 3, iteration < 6);
            else checkNotes<double>(component, processor, kSample64, rate, iteration % 3, iteration < 6);
        }
        assert(component->terminate() == kResultOk);
        component->release();
        controller->release();
        assert(processor->release() == 0);
    }
    assert(factory->release() == 0);
    assert(exit());
#ifdef _WIN32
    assert(FreeLibrary(module));
#else
    assert(dlclose(module) == 0);
#endif
    std::cout << "Verified plug-in version: " << expectedVersion << '\n';
    std::cout << "ABI smoke test passed (100 instances, float32/64, notes, phase continuity, silence, lifecycle)\n";
}
