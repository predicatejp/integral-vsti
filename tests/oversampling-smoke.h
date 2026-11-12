// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

// Rateの編集・状態復元はUIスレッドで再構成し、停止中だけ資源を準備する。
class RateHandler final : public IComponentHandler {
public:
    IComponent* component;
    IAudioProcessor* processor;
    const std::thread::id thread = std::this_thread::get_id();
    uint32 refs = 1;
    unsigned restarts = 0;
    RateHandler(IComponent* c, IAudioProcessor* p) : component(c), processor(p) {}
    tresult PLUGIN_API queryInterface(const TUID, void** out) override { *out = nullptr; return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { return --refs; }
    tresult PLUGIN_API beginEdit(ParamID) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API performEdit(ParamID, ParamValue) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API endEdit(ParamID) override { assert(false); return kResultFalse; }
    tresult PLUGIN_API restartComponent(int32 flags) override {
        assert(thread == std::this_thread::get_id() && flags == kLatencyChanged);
        assert(processor->setProcessing(false) == kResultOk);
        assert(component->setActive(false) == kResultOk);
        assert(component->setActive(true) == kResultOk);
        assert(processor->setProcessing(true) == kResultOk);
        ++restarts;
        return kResultOk;
    }
};

template <typename Sample>
void checkOversampling(IComponent* component, IAudioProcessor* processor,
                       IEditController* controller, int32 precision) {
    ParameterInfo rate, program;
    assert(controller->getParameterInfo(132, rate) == kResultOk);
    assert(rate.id == 0x46585254 && rate.stepCount == 2 && rate.defaultNormalizedValue == 0);
    assert(rate.flags == ParameterInfo::kIsList);
    assert(controller->getParameterInfo(133, program) == kResultOk);
    Points preset; preset.id = program.id; preset.values = {{0, 0}};
    Changes changes; changes.queues = {&preset};
    ProcessData data {}; data.symbolicSampleSize = precision; data.inputParameterChanges = &changes;
    assert(processor->process(data) == kResultOk);
    ProcessSetup setup {kRealtime, precision, 257, 48000.};
    assert(processor->setupProcessing(setup) == kResultOk);
    assert(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    assert(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
    assert(component->setActive(true) == kResultOk && processor->setProcessing(true) == kResultOk);
    RateHandler handler(component, processor);
    assert(controller->setComponentHandler(&handler) == kResultOk);
    std::array<Sample, 257> left {}, right {};
    Sample* channels[] = {left.data(), right.data()};
    AudioBusBuffers bus {}; bus.numChannels = 2;
    if constexpr (std::is_same_v<Sample, float>) bus.channelBuffers32 = channels;
    else bus.channelBuffers64 = channels;
    data.numOutputs = 1; data.outputs = &bus; data.inputParameterChanges = nullptr;
    StateStream saved;
    constexpr std::array<uint32, 3> latency {243, 305, 308};
    for (int mode : {1, 2, 0}) {
        const auto before = handler.restarts;
        assert(controller->setParamNormalized(rate.id, mode / 2.) == kResultOk);
        assert(handler.restarts == before + 1 && processor->getLatencySamples() == latency[mode]);
        assert(controller->setParamNormalized(rate.id, mode / 2.) == kResultOk);
        assert(handler.restarts == before + 1);
        // コールバックにパラメーターキューが届く前から設定を保存できる。
        if (mode == 2) assert(component->getState(&saved) == kResultOk);
        Events events; events.values = {note(true, 0, 123, 69)};
        double peak = 0;
        for (int block = 0; block < 8; ++block) {
            data.numSamples = block % 2 ? 257 : 31;
            data.inputEvents = block == 0 ? &events : nullptr;
            assert(processor->process(data) == kResultOk);
            for (int i = 0; i < data.numSamples; ++i) {
                assert(std::isfinite(left[i]) && left[i] == right[i]);
                peak = std::max(peak, std::abs(double(left[i])));
            }
        }
        assert(peak > .5 && peak < .8);
    }
    const auto before = handler.restarts;
    assert(component->setState(&saved) == kResultOk);
    saved.position = 0;
    assert(controller->setComponentState(&saved) == kResultOk);
    assert(handler.restarts == before + 1 && processor->getLatencySamples() == 308);
    assert(controller->getParamNormalized(rate.id) == 1);
    assert(controller->setParamNormalized(rate.id, 0) == kResultOk);
    assert(processor->getLatencySamples() == 243);
    assert(controller->setComponentHandler(nullptr) == kResultOk && handler.refs == 1);
    assert(processor->setProcessing(false) == kResultOk && component->setActive(false) == kResultOk);
}
