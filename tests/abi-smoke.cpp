// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// 検証専用。SDKのC++インターフェースからRust共有ライブラリを直接呼び出す。
// このコードとSDKはプラグイン本体にリンクしない。
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/vstspeaker.h"

#include <array>
#include <cassert>
#include <cstddef>
#include <cstring>
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
    std::cout << "ABI smoke test passed (100 instances, float32/64, silence, lifecycle)\n";
}
